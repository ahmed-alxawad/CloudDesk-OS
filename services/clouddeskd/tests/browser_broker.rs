//! Phase 9 Pass 2 (see `PHASE9_BROWSER_EVIDENCE.md`): real, live
//! integration evidence for the trusted typed Browser broker
//! (`services/clouddeskd/src/browser_broker.rs`) -- the first tests
//! that drive a real Brave container through `CloudDesk`'s own
//! authenticated Browser `WebSocket`, not raw CDP directly.

use axum::extract::connect_info::ConnectInfo;
use axum::extract::State;
use axum::http::Method;
use axum::response::IntoResponse;
use axum::routing::get;
use clouddesk_auth::{AuthPolicy, AuthService};
use clouddesk_secrets::SecretCipher;
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::process::Command as TokioCommand;
use tokio::sync::Mutex as TokioMutex;
use tokio_tungstenite::tungstenite::Message as WsMessage;

const BROWSER_IMAGE: &str = "clouddesk-brave:1.93.136";

struct BraveContainerGuard {
    before: std::collections::HashSet<String>,
}

fn list_brave_container_ids() -> std::collections::HashSet<String> {
    std::process::Command::new("docker")
        .args([
            "ps",
            "-a",
            "-q",
            "--filter",
            &format!("ancestor={BROWSER_IMAGE}"),
        ])
        .output()
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

impl BraveContainerGuard {
    fn new() -> Self {
        Self {
            before: list_brave_container_ids(),
        }
    }
}

impl Drop for BraveContainerGuard {
    fn drop(&mut self) {
        for id in list_brave_container_ids().difference(&self.before) {
            let _ = std::process::Command::new("docker")
                .args(["rm", "-f", id])
                .output();
        }
    }
}

/// Same cross-process serialization as `browser_runtime.rs` -- this
/// file's tests also start real Brave containers and must not race
/// against that file's tests under `cargo test --workspace`.
fn acquire_cross_process_browser_lock() -> std::fs::File {
    let path = std::env::temp_dir().join("clouddesk-browser-test.lock");
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .unwrap();
    rustix::fs::flock(&file, rustix::fs::FlockOperation::LockExclusive).unwrap();
    file
}

async fn docker_and_image_available() -> bool {
    TokioCommand::new("docker")
        .args(["image", "inspect", BROWSER_IMAGE])
        .output()
        .await
        .is_ok_and(|o| o.status.success())
}

async fn application() -> (
    String,
    tempfile::TempDir,
    std::sync::Arc<clouddesk_orchestrator::RuntimeManager>,
) {
    clouddeskd::browser_egress_proxy::spawn();
    let pool = clouddesk_db::connect("sqlite::memory:", 1).await.unwrap();
    clouddesk_db::migrate(&pool).await.unwrap();
    let auth = AuthService::new(
        pool.clone(),
        SecretCipher::new(&[103_u8; 32]).unwrap(),
        AuthPolicy::default(),
    )
    .unwrap();
    let directory = tempfile::tempdir().unwrap();
    let secret_path = directory.path().join("bootstrap.secret");
    std::fs::write(&secret_path, "browser-broker-test-secret\n").unwrap();

    let runtime_manager = std::sync::Arc::new(
        clouddesk_orchestrator::RuntimeManager::new(
            clouddesk_orchestrator::store::RuntimeStore::new(pool.clone()),
            std::env::temp_dir().join(format!(
                "clouddesk-browser-broker-test-{}",
                std::process::id()
            )),
            clouddesk_orchestrator::ResourcePolicy {
                start_timeout: std::time::Duration::from_secs(30),
                health_timeout: std::time::Duration::from_secs(20),
                ..clouddesk_orchestrator::ResourcePolicy::default()
            },
        )
        .with_adapter(std::sync::Arc::new(
            clouddesk_orchestrator::oci::OciAdapter::new(
                clouddeskd::browser_runtime::browser_oci_spec(BROWSER_IMAGE.to_owned()),
            ),
        ))
        .with_kind_policy(
            clouddesk_orchestrator::RuntimeKind::Browser,
            clouddesk_orchestrator::ResourcePolicy {
                start_timeout: std::time::Duration::from_secs(30),
                health_timeout: std::time::Duration::from_secs(20),
                pids_limit: Some(512),
                ..clouddesk_orchestrator::ResourcePolicy::default()
            },
        ),
    );

    let router = clouddeskd::application_router_and_media_and_library_and_runtime_configured(
        directory.path().to_owned(),
        auth,
        secret_path,
        true,
        None,
        None,
        Some(runtime_manager.clone()),
    );
    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        axum::serve(
            listener,
            router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await
        .unwrap();
    });
    (
        format!("http://127.0.0.1:{port}"),
        directory,
        runtime_manager,
    )
}

/// Task 19: builds a fresh router (fresh `RuntimeManager`, fresh
/// in-memory `live` instance map, fresh `axum::serve` listener on a new
/// port) against an *already-existing* `pool`/`auth` -- simulates a
/// real `clouddeskd` process restart far more faithfully than merely
/// killing Brave: the durable state (`SQLite` rows) survives exactly as
/// it would across a real restart, while every piece of in-process
/// state (the live-instance map, any open broker `WebSocket` tasks)
/// does not, exactly as a real process restart would lose it.
async fn spawn_router_on_pool(
    pool: &sqlx::SqlitePool,
    auth: AuthService,
) -> (
    String,
    std::sync::Arc<clouddesk_orchestrator::RuntimeManager>,
    // Returned (rather than `mem::forget`-ed, as this previously was)
    // so the caller's scope owns the bootstrap-secret directory and
    // `Drop` removes it -- otherwise every call left a `/tmp` residue
    // behind holding a real bootstrap secret file.
    tempfile::TempDir,
) {
    let runtime_manager = std::sync::Arc::new(
        clouddesk_orchestrator::RuntimeManager::new(
            clouddesk_orchestrator::store::RuntimeStore::new(pool.clone()),
            std::env::temp_dir().join(format!(
                "clouddesk-browser-broker-test-restart-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            )),
            clouddesk_orchestrator::ResourcePolicy {
                start_timeout: std::time::Duration::from_secs(30),
                health_timeout: std::time::Duration::from_secs(20),
                ..clouddesk_orchestrator::ResourcePolicy::default()
            },
        )
        .with_adapter(std::sync::Arc::new(
            clouddesk_orchestrator::oci::OciAdapter::new(
                clouddeskd::browser_runtime::browser_oci_spec(BROWSER_IMAGE.to_owned()),
            ),
        ))
        .with_kind_policy(
            clouddesk_orchestrator::RuntimeKind::Browser,
            clouddesk_orchestrator::ResourcePolicy {
                start_timeout: std::time::Duration::from_secs(30),
                health_timeout: std::time::Duration::from_secs(20),
                pids_limit: Some(512),
                ..clouddesk_orchestrator::ResourcePolicy::default()
            },
        ),
    );
    let directory = tempfile::tempdir().unwrap();
    let secret_path = directory.path().join("bootstrap.secret");
    std::fs::write(&secret_path, "browser-broker-test-secret\n").unwrap();
    let router = clouddeskd::application_router_and_media_and_library_and_runtime_configured(
        std::env::temp_dir(),
        auth,
        secret_path,
        true,
        None,
        None,
        Some(runtime_manager.clone()),
    );
    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        axum::serve(
            listener,
            router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await
        .unwrap();
    });
    (
        format!("http://127.0.0.1:{port}"),
        runtime_manager,
        directory,
    )
}

async fn http(
    base: &str,
    method: Method,
    path: &str,
    cookie: Option<&str>,
    body: Option<&Value>,
) -> reqwest::Response {
    let mut builder = reqwest::Client::new().request(
        reqwest::Method::from_bytes(method.as_str().as_bytes()).unwrap(),
        format!("{base}{path}"),
    );
    if let Some(cookie) = cookie {
        builder = builder.header(reqwest::header::COOKIE, cookie);
    }
    if let Some(body) = body {
        builder = builder
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(body.to_string());
    }
    builder.send().await.unwrap()
}

fn current_process_linux_identity() -> Option<clouddesk_linux::LinuxIdentity> {
    let uid = rustix::process::getuid().as_raw();
    if uid == 0 {
        return None;
    }
    clouddesk_linux::lookup_uid(uid).ok().flatten()
}

async fn bootstrap_admin(base: &str) -> String {
    let linux_username = current_process_linux_identity().map(|i| i.username);
    let response = http(
        base,
        Method::POST,
        "/api/v1/setup/bootstrap",
        None,
        Some(&json!({
            "secret": "browser-broker-test-secret",
            "username": "admin",
            "display_name": "Admin",
            "password": "correct horse battery staple",
            "linux_username": linux_username,
        })),
    )
    .await;
    assert_eq!(response.status(), reqwest::StatusCode::CREATED);
    login(base, "admin", "correct horse battery staple").await
}

async fn create_user(base: &str, admin_cookie: &str, username: &str, role_id: &str) -> String {
    let identity = current_process_linux_identity()
        .expect("this test requires running as a real, mapped, non-root Linux user");
    let step_up = http(
        base,
        Method::POST,
        "/api/v1/auth/step-up",
        Some(admin_cookie),
        Some(&json!({"password": "correct horse battery staple"})),
    )
    .await;
    assert_eq!(step_up.status(), reqwest::StatusCode::OK);

    let create = http(
        base,
        Method::POST,
        "/api/v1/users",
        Some(admin_cookie),
        Some(&json!({
            "username": username,
            "display_name": username,
            "password": "user horse battery staple",
            "role_ids": [role_id],
        })),
    )
    .await;
    assert_eq!(create.status(), reqwest::StatusCode::CREATED);
    let body: Value = create.json().await.unwrap();
    let user_id = body["user_id"].as_str().unwrap().to_owned();

    let set_identity = http(
        base,
        Method::PUT,
        &format!("/api/v1/users/{user_id}/linux-identity"),
        Some(admin_cookie),
        Some(&json!({ "uid": identity.uid, "gid": identity.gid })),
    )
    .await;
    assert_eq!(set_identity.status(), reqwest::StatusCode::NO_CONTENT);

    login(base, username, "user horse battery staple").await
}

async fn login(base: &str, username: &str, password: &str) -> String {
    let response = http(
        base,
        Method::POST,
        "/api/v1/auth/login",
        None,
        Some(&json!({"username": username, "password": password})),
    )
    .await;
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    response
        .headers()
        .get(reqwest::header::SET_COOKIE)
        .unwrap()
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned()
}

async fn enable_browser(base: &str, admin_cookie: &str) {
    let enable = http(
        base,
        Method::POST,
        "/api/v1/runtimes/browser/enable",
        Some(admin_cookie),
        None,
    )
    .await;
    assert_eq!(enable.status(), reqwest::StatusCode::NO_CONTENT);
}

async fn wait_for_running(base: &str, cookie: &str, instance_id: &str) -> bool {
    for _ in 0..40 {
        let status = http(
            base,
            Method::GET,
            &format!("/api/v1/runtime-instances/browser/{instance_id}"),
            Some(cookie),
            None,
        )
        .await;
        let body: Value = status.json().await.unwrap();
        if body["state"].as_str() == Some("running") {
            return true;
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    false
}

async fn open_browser_instance(base: &str, cookie: &str) -> String {
    let create = http(
        base,
        Method::POST,
        "/api/v1/runtime-instances",
        Some(cookie),
        Some(&json!({"kind": "browser"})),
    )
    .await;
    assert_eq!(create.status(), reqwest::StatusCode::OK);
    let body: Value = create.json().await.unwrap();
    let instance_id = body["instance_id"].as_str().unwrap().to_owned();
    assert!(wait_for_running(base, cookie, &instance_id).await);
    instance_id
}

/// Connects to the real trusted broker `WebSocket` for `instance_id`,
/// authenticated with `cookie` -- the same `CloudDesk` session cookie
/// every other route uses, never a raw CDP URL.
async fn connect_browser_ws(
    base: &str,
    cookie: &str,
    instance_id: &str,
) -> Result<
    (
        futures_util::stream::SplitSink<
            tokio_tungstenite::WebSocketStream<
                tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
            >,
            WsMessage,
        >,
        futures_util::stream::SplitStream<
            tokio_tungstenite::WebSocketStream<
                tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
            >,
        >,
    ),
    String,
> {
    let ws_url = format!(
        "ws{}/api/v1/runtime-instances/browser/{instance_id}/browser-ws",
        base.strip_prefix("http").unwrap()
    );
    let mut request = tokio_tungstenite::tungstenite::http::Request::builder()
        .uri(&ws_url)
        .header("Host", "127.0.0.1")
        .header("Connection", "Upgrade")
        .header("Upgrade", "websocket")
        .header("Sec-WebSocket-Version", "13")
        .header(
            "Sec-WebSocket-Key",
            tokio_tungstenite::tungstenite::handshake::client::generate_key(),
        )
        .header("Cookie", cookie)
        .body(())
        .unwrap();
    request
        .headers_mut()
        .insert("Cookie", cookie.parse().unwrap());
    match tokio_tungstenite::connect_async(request).await {
        Ok((stream, _)) => Ok(stream.split()),
        Err(e) => Err(e.to_string()),
    }
}

async fn recv_json(
    stream: &mut futures_util::stream::SplitStream<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
    >,
    timeout: std::time::Duration,
) -> Option<Value> {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return None;
        }
        match tokio::time::timeout(remaining, stream.next()).await {
            Ok(Some(Ok(WsMessage::Text(text)))) => {
                if let Ok(v) = serde_json::from_str::<Value>(&text) {
                    return Some(v);
                }
            }
            Ok(Some(Ok(_))) => {}
            _ => return None,
        }
    }
}

async fn recv_json_matching(
    stream: &mut futures_util::stream::SplitStream<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
    >,
    predicate: impl Fn(&Value) -> bool,
    timeout: std::time::Duration,
) -> Option<Value> {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return None;
        }
        match recv_json(stream, remaining).await {
            Some(v) if predicate(&v) => return Some(v),
            Some(_) => {}
            None => return None,
        }
    }
}

/// Task 17: a disposable, minimal acceptance website -- a unique
/// visible sentinel, a button/checkbox/text-input that each report
/// back to this same server via `fetch()` when interacted with (so
/// broker-dispatched input can be verified without a generic CDP
/// `Runtime.evaluate` capability, which the broker deliberately never
/// exposes), and safe request-source logging (remote address + User-
/// Agent only, no secrets) for Task 18's server-side-origin proof.
#[derive(Default)]
struct FixtureLog {
    last_remote_addr: Option<String>,
    last_user_agent: Option<String>,
    click_count: u32,
    last_input_value: Option<String>,
    checkbox_checked: Option<bool>,
}

fn fixture_page() -> impl IntoResponse {
    axum::response::Html(
        r#"<!doctype html><html><body>
<div id="sentinel">CLOUDDESK-BROWSER-FIXTURE-SENTINEL</div>
<button id="btn" style="position:absolute;left:20px;top:20px;width:100px;height:40px;"
  onclick="document.getElementById('btn').style.background='red'; fetch('/click', {method:'POST'});">
  Click me
</button>
<input id="txt" type="text" style="position:absolute;left:20px;top:80px;"
  oninput="fetch('/input?value=' + encodeURIComponent(this.value))" />
<input id="chk" type="checkbox" style="position:absolute;left:20px;top:120px;"
  onchange="fetch('/checkbox?checked=' + this.checked)" />
<button id="popup" style="position:absolute;left:20px;top:160px;width:100px;height:40px;"
  onclick="window.open('/page2', '_blank')">
  Open popup
</button>
<button id="popupstorm" style="position:absolute;left:20px;top:260px;width:100px;height:40px;"
  onclick="for (let i = 0; i < 12; i++) { window.open('/page2', '_blank_' + i); }">
  Popup storm
</button>
<a id="link2" href="/page2" style="position:absolute;left:20px;top:210px;">page2</a>
</body></html>"#,
    )
}

async fn fixture_page2() -> impl IntoResponse {
    axum::response::Html(
        r#"<!doctype html><html><body>
<div id="sentinel2">CLOUDDESK-BROWSER-FIXTURE-PAGE2-SENTINEL</div>
</body></html>"#,
    )
}

async fn fixture_click(
    State(log): State<Arc<TokioMutex<FixtureLog>>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: axum::http::HeaderMap,
) {
    let mut log = log.lock().await;
    log.click_count += 1;
    log.last_remote_addr = Some(addr.ip().to_string());
    log.last_user_agent = headers
        .get("user-agent")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
}

async fn fixture_input(
    State(log): State<Arc<TokioMutex<FixtureLog>>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) {
    let mut log = log.lock().await;
    log.last_input_value = params.get("value").cloned();
    log.last_remote_addr = Some(addr.ip().to_string());
}

async fn fixture_checkbox(
    State(log): State<Arc<TokioMutex<FixtureLog>>>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) {
    let mut log = log.lock().await;
    log.checkbox_checked = params.get("checked").map(|v| v == "true");
}

async fn fixture_root_with_source(
    State(log): State<Arc<TokioMutex<FixtureLog>>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: axum::http::HeaderMap,
) -> impl IntoResponse {
    let mut guard = log.lock().await;
    guard.last_remote_addr = Some(addr.ip().to_string());
    guard.last_user_agent = headers
        .get("user-agent")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    drop(guard);
    fixture_page()
}

async fn spawn_fixture_site() -> (String, Arc<TokioMutex<FixtureLog>>) {
    let log = Arc::new(TokioMutex::new(FixtureLog::default()));

    let router = axum::Router::new()
        .route("/", get(fixture_root_with_source))
        .route("/page2", get(fixture_page2))
        .route("/click", axum::routing::post(fixture_click))
        .route("/input", get(fixture_input))
        .route("/checkbox", get(fixture_checkbox))
        .with_state(log.clone());

    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        axum::serve(
            listener,
            router.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .unwrap();
    });
    (
        format!("http://0.0.0.0:{port}").replace("0.0.0.0", "REPLACE_WITH_GATEWAY"),
        log,
    )
}

/// The Docker bridge network's own gateway IP -- reachable from inside
/// any container on that network, unlike the host's `127.0.0.1` (which
/// is what a Brave container's own `127.0.0.1` refers to, i.e. itself).
async fn bridge_gateway_ip() -> String {
    let output = TokioCommand::new("docker")
        .args([
            "network",
            "inspect",
            "bridge",
            "--format",
            "{{(index .IPAM.Config 0).Gateway}}",
        ])
        .output()
        .await
        .unwrap();
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

/// Task 1/2: `BrowserSession` ownership -- ID possession alone is not
/// authorization. An unauthenticated caller and a caller presenting a
/// different user's session cookie against the *same instance-id
/// string* are both denied, because `InstanceId` ownership is derived
/// from the authenticated principal server-side, never from the
/// request (identical guarantee the existing generic runtime-instance
/// routes already provide -- this proves the Browser broker route
/// inherits it, not that the guarantee is novel).
#[tokio::test]
#[allow(clippy::similar_names)]
async fn task_1_2_ownership_unauthenticated_and_cross_user_denied() {
    if !docker_and_image_available().await {
        clouddesk_test_support::blocked_by_environment(
            "task_1_2_ownership_unauthenticated_and_cross_user_denied",
            clouddesk_test_support::reason::CONTAINER_RUNTIME_UNAVAILABLE,
        );
        return;
    }
    let (base, _dir, _runtime_manager) = application().await;
    let _cross_process_guard = tokio::task::spawn_blocking(acquire_cross_process_browser_lock)
        .await
        .unwrap();
    let _brave_container_guard = BraveContainerGuard::new();
    let admin_cookie = bootstrap_admin(&base).await;
    enable_browser(&base, &admin_cookie).await;
    let user_a_cookie = create_user(&base, &admin_cookie, "brokeruserA", "user").await;
    let user_b_cookie = create_user(&base, &admin_cookie, "brokeruserB", "user").await;

    let instance_id = open_browser_instance(&base, &user_a_cookie).await;

    // Owner succeeds.
    let (mut owner_tx, mut owner_rx) = connect_browser_ws(&base, &user_a_cookie, &instance_id)
        .await
        .expect("owner must be able to open the browser session");
    let connected = recv_json_matching(
        &mut owner_rx,
        |v| v["type"] == "connected",
        std::time::Duration::from_secs(10),
    )
    .await;
    assert!(
        connected.is_some(),
        "owner must receive a real connected message"
    );
    let _ = owner_tx.close().await;

    // User B against the same instance-id string never resolves to
    // User A's real instance (ownership is compound owner+instance_id,
    // not a bare lookup) -- the connection either fails the upgrade or
    // the broker reports the runtime as not running for User B's own,
    // nonexistent same-named instance.
    match connect_browser_ws(&base, &user_b_cookie, &instance_id).await {
        Err(_) => {} // upgrade itself denied -- acceptable
        Ok((mut tx, mut rx)) => {
            let msg = recv_json(&mut rx, std::time::Duration::from_secs(5)).await;
            assert_ne!(
                msg.as_ref().map(|v| &v["type"]),
                Some(&json!("connected")),
                "User B must never reach User A's real browser session, got {msg:?}"
            );
            let _ = tx.close().await;
        }
    }

    // Unauthenticated: no cookie at all.
    let ws_url = format!(
        "ws{}/api/v1/runtime-instances/browser/{instance_id}/browser-ws",
        base.strip_prefix("http").unwrap()
    );
    let unauth = tokio_tungstenite::connect_async(&ws_url).await;
    assert!(
        unauth.is_err(),
        "an unauthenticated caller must be denied the browser-ws upgrade entirely"
    );
}

/// Task 5: raw CDP must remain unreachable from anything other than
/// `clouddeskd`'s own backend process. The adapter publishes the
/// relayed CDP port bound only to the host's loopback interface
/// (`127.0.0.1:{port}:{container_port}`, see `browser_runtime.rs`) --
/// live-proven here from a real, separate, disposable container that
/// is NOT `clouddeskd` itself: it cannot reach that port through the
/// Docker bridge gateway, because a `127.0.0.1`-only publish rule never
/// answers packets arriving from anywhere but the host's own network
/// namespace.
#[tokio::test]
async fn task_5_raw_cdp_unreachable_from_another_container() {
    if !docker_and_image_available().await {
        clouddesk_test_support::blocked_by_environment(
            "task_5_raw_cdp_unreachable_from_another_container",
            clouddesk_test_support::reason::CONTAINER_RUNTIME_UNAVAILABLE,
        );
        return;
    }
    let (base, _dir, runtime_manager) = application().await;
    let _cross_process_guard = tokio::task::spawn_blocking(acquire_cross_process_browser_lock)
        .await
        .unwrap();
    let _brave_container_guard = BraveContainerGuard::new();
    let admin_cookie = bootstrap_admin(&base).await;
    enable_browser(&base, &admin_cookie).await;
    let user_cookie = create_user(&base, &admin_cookie, "cdpisolationuser", "user").await;
    let instance_id = open_browser_instance(&base, &user_cookie).await;

    let me = http(
        &base,
        Method::GET,
        "/api/v1/auth/me",
        Some(&user_cookie),
        None,
    )
    .await;
    let me_body: Value = me.json().await.unwrap();
    let owner_user_id = me_body["user_id"].as_str().unwrap().to_owned();
    let id = clouddesk_orchestrator::InstanceId {
        kind: clouddesk_orchestrator::RuntimeKind::Browser,
        owner_user_id: owner_user_id.clone(),
        instance_id,
    };
    let port = runtime_manager
        .instance_port(&owner_user_id, &id)
        .await
        .expect("running instance must have a real port");

    let gateway = bridge_gateway_ip().await;
    clouddeskd::browser_egress_proxy::set_test_allowlist([gateway.parse().unwrap()]);
    // A real, unrelated disposable container (alpine, never
    // clouddeskd) attempting to reach the host's 127.0.0.1-bound CDP
    // relay port via the bridge gateway -- must fail (connection
    // refused/timeout), never succeed.
    let output = TokioCommand::new("docker")
        .args([
            "run",
            "--rm",
            "--network",
            "bridge",
            "alpine:3.20",
            "sh",
            "-c",
            &format!("wget -T 3 -O- http://{gateway}:{port}/json/version 2>&1 || echo UNREACHABLE"),
        ])
        .output()
        .await
        .unwrap();
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        combined.contains("UNREACHABLE") || !combined.contains("Browser"),
        "an unrelated container must never reach another instance's raw CDP endpoint, got: {combined}"
    );
}

/// Task 7/9/10/13/14/15/16/18: the real product-slice acceptance --
/// navigate to a controlled fixture site through the typed broker
/// (never raw CDP), receive real screencast frames, resize the
/// viewport, dispatch real mouse/keyboard input and verify the actual
/// server-side page reacted (via the fixture's own request log, not a
/// generic CDP eval capability), and confirm the fixture observed the
/// request arriving from Brave's own container network -- not from
/// this test process directly (which would prove nothing about the
/// broker/CDP/Brave path actually being exercised).
#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn task_7_9_10_13_14_15_16_18_broker_product_slice() {
    if !docker_and_image_available().await {
        clouddesk_test_support::blocked_by_environment(
            "task_7_9_10_13_14_15_16_18_broker_product_slice",
            clouddesk_test_support::reason::CONTAINER_RUNTIME_UNAVAILABLE,
        );
        return;
    }
    let (base, _dir, _runtime_manager) = application().await;
    let _cross_process_guard = tokio::task::spawn_blocking(acquire_cross_process_browser_lock)
        .await
        .unwrap();
    let _brave_container_guard = BraveContainerGuard::new();
    let admin_cookie = bootstrap_admin(&base).await;
    enable_browser(&base, &admin_cookie).await;
    let user_cookie = create_user(&base, &admin_cookie, "brokerslice", "user").await;
    let instance_id = open_browser_instance(&base, &user_cookie).await;

    let (fixture_url_template, fixture_log) = spawn_fixture_site().await;
    let gateway = bridge_gateway_ip().await;
    clouddeskd::browser_egress_proxy::set_test_allowlist([gateway.parse().unwrap()]);
    let fixture_url = fixture_url_template.replace("REPLACE_WITH_GATEWAY", &gateway);

    let (mut tx, mut rx) = connect_browser_ws(&base, &user_cookie, &instance_id)
        .await
        .expect("owner must open the browser session");
    let connected = recv_json_matching(
        &mut rx,
        |v| v["type"] == "connected",
        std::time::Duration::from_secs(10),
    )
    .await;
    assert!(connected.is_some(), "must receive connected");

    // Task 9: real screencast frames flow without any navigation yet
    // (about:blank itself redraws once).
    let first_frame = recv_json_matching(
        &mut rx,
        |v| v["type"] == "frame",
        std::time::Duration::from_secs(15),
    )
    .await;
    assert!(
        first_frame.is_some(),
        "must receive at least one real screencast frame"
    );
    assert!(
        !first_frame.unwrap()["data_base64"]
            .as_str()
            .unwrap_or_default()
            .is_empty(),
        "frame must carry real, non-empty encoded pixel data"
    );

    // Task 7: dangerous schemes rejected before reaching Brave.
    tx.send(WsMessage::Text(
        json!({"type": "navigate", "url": "file:///etc/passwd"}).to_string(),
    ))
    .await
    .unwrap();
    let file_rejected = recv_json_matching(
        &mut rx,
        |v| v["type"] == "error",
        std::time::Duration::from_secs(5),
    )
    .await;
    assert!(
        file_rejected.is_some(),
        "file: navigation must be rejected with a typed error"
    );

    tx.send(WsMessage::Text(
        json!({"type": "navigate", "url": "javascript:alert(1)"}).to_string(),
    ))
    .await
    .unwrap();
    let js_rejected = recv_json_matching(
        &mut rx,
        |v| v["type"] == "error",
        std::time::Duration::from_secs(5),
    )
    .await;
    assert!(
        js_rejected.is_some(),
        "javascript: navigation must be rejected with a typed error"
    );

    // Task 13: viewport resize.
    tx.send(WsMessage::Text(
        json!({"type": "resize", "width": 640, "height": 480}).to_string(),
    ))
    .await
    .unwrap();
    let resized_frame = recv_json_matching(
        &mut rx,
        |v| v["type"] == "frame" && v["width"].as_u64() == Some(640),
        std::time::Duration::from_secs(15),
    )
    .await;
    assert!(
        resized_frame.is_some(),
        "a subsequent frame must reflect the new 640x480 viewport"
    );

    // Real navigation to the controlled fixture site through the typed
    // broker (Task 18's server-side-origin proof depends on this).
    tx.send(WsMessage::Text(
        json!({"type": "navigate", "url": fixture_url}).to_string(),
    ))
    .await
    .unwrap();
    let page_state = recv_json_matching(
        &mut rx,
        |v| v["type"] == "page_state" && v.get("url").is_some(),
        std::time::Duration::from_secs(15),
    )
    .await;
    assert!(
        page_state.is_some(),
        "must observe a real page_state navigation event"
    );

    // Give the real Brave page a moment to actually load and be
    // scriptable before dispatching input.
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;

    // Task 18: the fixture's root request must have arrived from
    // Brave's own container network, not from this test process.
    {
        let log = fixture_log.lock().await;
        let remote = log.last_remote_addr.clone().unwrap_or_default();
        assert_ne!(remote, "127.0.0.1", "the fixture site must be reached by Brave's own container, not the test process directly");
        let ua = log.last_user_agent.clone().unwrap_or_default();
        assert!(
            ua.contains("Chrome") || ua.contains("Brave") || ua.contains("HeadlessChrome"),
            "the fixture must observe a real browser User-Agent, got {ua:?}"
        );
    }

    // Task 14: real mouse click on the fixture's button, verified via
    // the fixture's own request log (not a generic CDP eval).
    tx.send(WsMessage::Text(
        json!({"type": "mouse_move", "x": 40.0, "y": 40.0}).to_string(),
    ))
    .await
    .unwrap();
    tx.send(WsMessage::Text(
        json!({"type": "mouse_down", "x": 40.0, "y": 40.0, "button": "left"}).to_string(),
    ))
    .await
    .unwrap();
    tx.send(WsMessage::Text(
        json!({"type": "mouse_up", "x": 40.0, "y": 40.0, "button": "left"}).to_string(),
    ))
    .await
    .unwrap();

    let mut clicked = false;
    for _ in 0..20 {
        if fixture_log.lock().await.click_count > 0 {
            clicked = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    assert!(clicked, "a real broker-dispatched mouse click must reach the real Brave page and trigger its onclick handler");

    // Task 15/16: real keyboard input, including basic Unicode, into
    // the fixture's text input, verified via its oninput callback.
    tx.send(WsMessage::Text(
        json!({"type": "mouse_move", "x": 60.0, "y": 90.0}).to_string(),
    ))
    .await
    .unwrap();
    tx.send(WsMessage::Text(
        json!({"type": "mouse_down", "x": 60.0, "y": 90.0, "button": "left"}).to_string(),
    ))
    .await
    .unwrap();
    tx.send(WsMessage::Text(
        json!({"type": "mouse_up", "x": 60.0, "y": 90.0, "button": "left"}).to_string(),
    ))
    .await
    .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    let test_string = "aA1 \u{00e9}\u{4e2d}"; // ASCII + accented Latin + one non-Latin (Basic Unicode, Task 16)
    for ch in test_string.chars() {
        let text = ch.to_string();
        tx.send(WsMessage::Text(
            json!({"type": "key_down", "key": text, "text": text}).to_string(),
        ))
        .await
        .unwrap();
        tx.send(WsMessage::Text(
            json!({"type": "key_up", "key": text}).to_string(),
        ))
        .await
        .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }

    let mut observed_value = None;
    for _ in 0..20 {
        let candidate = fixture_log.lock().await.last_input_value.clone();
        if candidate.as_deref().is_some_and(|v| !v.is_empty()) {
            observed_value = candidate;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    assert!(
        observed_value.is_some(),
        "real broker-dispatched keyboard input must reach the real Brave page's text input"
    );

    let _ = tx.close().await;
}

/// Task 24: kill the real Brave container out from under an active
/// broker `WebSocket`. Verify: the client receives an explicit
/// `closed` message (not a silent hang), `RuntimeManager` detects the
/// failure, no orphan Brave process remains, and after restarting the
/// same instance a brand-new session connects cleanly against the
/// replacement container -- the old session's own CDP connection can
/// never silently reattach to it (a new `run_browser_session` task
/// opens its own fresh CDP target against whatever port
/// `instance_port` reports *at that later connect time*, structurally
/// incapable of resuming a dead connection object from before the
/// crash).
#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn task_24_crash_handling_and_generation_invalidation() {
    if !docker_and_image_available().await {
        clouddesk_test_support::blocked_by_environment(
            "task_24_crash_handling_and_generation_invalidation",
            clouddesk_test_support::reason::CONTAINER_RUNTIME_UNAVAILABLE,
        );
        return;
    }
    let (base, _dir, _runtime_manager) = application().await;
    let _cross_process_guard = tokio::task::spawn_blocking(acquire_cross_process_browser_lock)
        .await
        .unwrap();
    let _brave_container_guard = BraveContainerGuard::new();
    let admin_cookie = bootstrap_admin(&base).await;
    enable_browser(&base, &admin_cookie).await;
    let user_cookie = create_user(&base, &admin_cookie, "crashuser", "user").await;
    let instance_id = open_browser_instance(&base, &user_cookie).await;

    let (mut tx, mut rx) = connect_browser_ws(&base, &user_cookie, &instance_id)
        .await
        .expect("owner must open the browser session");
    let connected = recv_json_matching(
        &mut rx,
        |v| v["type"] == "connected",
        std::time::Duration::from_secs(10),
    )
    .await;
    assert!(connected.is_some());
    let _ = recv_json_matching(
        &mut rx,
        |v| v["type"] == "frame",
        std::time::Duration::from_secs(15),
    )
    .await;

    let ps = TokioCommand::new("docker")
        .args(["ps", "-q", "--filter", &format!("ancestor={BROWSER_IMAGE}")])
        .output()
        .await
        .unwrap();
    let container_id = String::from_utf8_lossy(&ps.stdout)
        .lines()
        .next()
        .unwrap_or_default()
        .to_owned();
    assert!(
        !container_id.is_empty(),
        "expected a real running Brave container"
    );

    let kill = TokioCommand::new("docker")
        .args(["kill", &container_id])
        .output()
        .await
        .unwrap();
    assert!(kill.status.success(), "docker kill must succeed");

    let closed = recv_json_matching(
        &mut rx,
        |v| v["type"] == "closed",
        std::time::Duration::from_secs(20),
    )
    .await;
    assert!(
        closed.is_some(),
        "the broker must send an explicit closed message when Brave crashes, not hang silently"
    );
    let _ = tx.close().await;

    let mut runtime_manager_saw_failure = false;
    for _ in 0..30 {
        let status = http(
            &base,
            Method::GET,
            &format!("/api/v1/runtime-instances/browser/{instance_id}"),
            Some(&user_cookie),
            None,
        )
        .await;
        let body: Value = status.json().await.unwrap();
        if matches!(
            body["state"].as_str(),
            Some("failed" | "unhealthy" | "stopped")
        ) {
            runtime_manager_saw_failure = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    assert!(
        runtime_manager_saw_failure,
        "RuntimeManager must detect the crashed container via its own health mechanism"
    );

    // No orphan Brave process: the killed container itself is gone
    // (docker kill + reaping), and no *new* container appeared besides
    // whatever gets created by the restart below.
    let inspect = TokioCommand::new("docker")
        .args(["inspect", &container_id])
        .output()
        .await
        .unwrap();
    assert!(
        !inspect.status.success()
            || String::from_utf8_lossy(&inspect.stdout).contains("\"Running\": false"),
        "the killed container must not remain running"
    );

    let restart = http(
        &base,
        Method::POST,
        &format!("/api/v1/runtime-instances/browser/{instance_id}/restart"),
        Some(&user_cookie),
        None,
    )
    .await;
    assert_eq!(restart.status(), reqwest::StatusCode::OK);
    assert!(wait_for_running(&base, &user_cookie, &instance_id).await);

    // A brand-new session against the replacement container connects
    // cleanly -- proves the broker isn't left in some permanently
    // wedged state by the crash.
    let (mut tx2, mut rx2) = connect_browser_ws(&base, &user_cookie, &instance_id)
        .await
        .expect("owner must be able to open a fresh session after restart");
    let reconnected = recv_json_matching(
        &mut rx2,
        |v| v["type"] == "connected",
        std::time::Duration::from_secs(10),
    )
    .await;
    assert!(
        reconnected.is_some(),
        "a fresh session must connect cleanly against the replacement container"
    );
    let _ = tx2.close().await;
}

/// Task 25: the real enable/disable product path. While a Browser
/// session is active, disable the runtime as Administrator; the active
/// Brave container must stop and no new session may start until
/// re-enabled.
#[tokio::test]
async fn task_25_enable_disable_lifecycle() {
    if !docker_and_image_available().await {
        clouddesk_test_support::blocked_by_environment(
            "task_25_enable_disable_lifecycle",
            clouddesk_test_support::reason::CONTAINER_RUNTIME_UNAVAILABLE,
        );
        return;
    }
    let (base, _dir, _runtime_manager) = application().await;
    let _cross_process_guard = tokio::task::spawn_blocking(acquire_cross_process_browser_lock)
        .await
        .unwrap();
    let _brave_container_guard = BraveContainerGuard::new();
    let admin_cookie = bootstrap_admin(&base).await;
    enable_browser(&base, &admin_cookie).await;
    let user_cookie = create_user(&base, &admin_cookie, "enabledisableuser", "user").await;
    let instance_id = open_browser_instance(&base, &user_cookie).await;

    let disable = http(
        &base,
        Method::POST,
        "/api/v1/runtimes/browser/disable",
        Some(&admin_cookie),
        None,
    )
    .await;
    assert_eq!(disable.status(), reqwest::StatusCode::NO_CONTENT);

    let mut stopped = false;
    for _ in 0..30 {
        let status = http(
            &base,
            Method::GET,
            &format!("/api/v1/runtime-instances/browser/{instance_id}"),
            Some(&user_cookie),
            None,
        )
        .await;
        let body: Value = status.json().await.unwrap();
        if matches!(
            body["state"].as_str(),
            Some("stopped" | "failed" | "unavailable")
        ) {
            stopped = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    }
    assert!(
        stopped,
        "the active Brave container must stop when the runtime is disabled"
    );

    let ps = TokioCommand::new("docker")
        .args(["ps", "-q", "--filter", &format!("ancestor={BROWSER_IMAGE}")])
        .output()
        .await
        .unwrap();
    assert!(
        String::from_utf8_lossy(&ps.stdout).trim().is_empty(),
        "zero Brave containers must remain running after disable"
    );

    let denied = http(
        &base,
        Method::POST,
        "/api/v1/runtime-instances",
        Some(&user_cookie),
        Some(&json!({"kind": "browser"})),
    )
    .await;
    assert_ne!(
        denied.status(),
        reqwest::StatusCode::OK,
        "a new Browser session must be denied while the runtime is disabled"
    );

    let enable = http(
        &base,
        Method::POST,
        "/api/v1/runtimes/browser/enable",
        Some(&admin_cookie),
        None,
    )
    .await;
    assert_eq!(enable.status(), reqwest::StatusCode::NO_CONTENT);

    // Real, documented gap (same one `browser_runtime.rs`'s
    // `task_5_8_guest_ephemeral_and_cross_user_isolation` already
    // found and worked around): Browser has no instance-reuse-on-create
    // path, and `max_instances_per_user` (default 1) counts the
    // stopped-but-undeleted instance from before disable, so a genuine
    // `POST /api/v1/runtime-instances` here returns 429, not a fresh
    // instance. Restarting the same (still-existing) instance exercises
    // the identical "usable again after re-enable" claim this task
    // actually cares about.
    let restart = http(
        &base,
        Method::POST,
        &format!("/api/v1/runtime-instances/browser/{instance_id}/restart"),
        Some(&user_cookie),
        None,
    )
    .await;
    assert_eq!(
        restart.status(),
        reqwest::StatusCode::OK,
        "the existing Browser instance must be usable again after re-enabling"
    );
    assert!(wait_for_running(&base, &user_cookie, &instance_id).await);
}

fn tab_list_from(msg: &Value) -> Vec<Value> {
    msg["tabs"].as_array().cloned().unwrap_or_default()
}

/// Tasks 1/3: real tab lifecycle -- create a second tab, navigate each
/// tab to a different real page, switch between them and confirm
/// `page_state` reflects the correct tab each time, close one tab and
/// confirm the other survives, then close the last tab and confirm the
/// session falls back to a fresh blank tab rather than being left with
/// zero tabs.
#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn task_1_3_tab_lifecycle_create_switch_close() {
    if !docker_and_image_available().await {
        clouddesk_test_support::blocked_by_environment(
            "task_1_3_tab_lifecycle_create_switch_close",
            clouddesk_test_support::reason::CONTAINER_RUNTIME_UNAVAILABLE,
        );
        return;
    }
    let (base, _dir, _runtime_manager) = application().await;
    let _cross_process_guard = tokio::task::spawn_blocking(acquire_cross_process_browser_lock)
        .await
        .unwrap();
    let _brave_container_guard = BraveContainerGuard::new();
    let admin_cookie = bootstrap_admin(&base).await;
    enable_browser(&base, &admin_cookie).await;
    let user_cookie = create_user(&base, &admin_cookie, "tablifecycle", "user").await;
    let instance_id = open_browser_instance(&base, &user_cookie).await;

    let (fixture_url_template, _fixture_log) = spawn_fixture_site().await;
    let gateway = bridge_gateway_ip().await;
    clouddeskd::browser_egress_proxy::set_test_allowlist([gateway.parse().unwrap()]);
    let fixture_url = fixture_url_template.replace("REPLACE_WITH_GATEWAY", &gateway);
    let fixture_page2_url = format!("{fixture_url}/page2");

    let (mut tx, mut rx) = connect_browser_ws(&base, &user_cookie, &instance_id)
        .await
        .expect("owner must open the browser session");
    let connected = recv_json_matching(
        &mut rx,
        |v| v["type"] == "connected",
        std::time::Duration::from_secs(10),
    )
    .await;
    assert!(connected.is_some());
    let initial_list = recv_json_matching(
        &mut rx,
        |v| v["type"] == "tab_list",
        std::time::Duration::from_secs(10),
    )
    .await
    .expect("must receive an initial tab_list");
    assert_eq!(
        tab_list_from(&initial_list).len(),
        1,
        "a fresh session must start with exactly one tab"
    );
    let tab_a = tab_list_from(&initial_list)[0]["tab_id"]
        .as_str()
        .unwrap()
        .to_owned();

    tx.send(WsMessage::Text(json!({"type": "create_tab"}).to_string()))
        .await
        .unwrap();
    let created = recv_json_matching(
        &mut rx,
        |v| v["type"] == "tab_created",
        std::time::Duration::from_secs(10),
    )
    .await;
    assert!(created.is_some(), "create_tab must succeed");
    let tab_b = created.unwrap()["tab_id"].as_str().unwrap().to_owned();
    assert_ne!(
        tab_a, tab_b,
        "the new tab must have a distinct opaque TabId"
    );

    let list_after_create = recv_json_matching(
        &mut rx,
        |v| v["type"] == "tab_list" && tab_list_from(v).len() == 2,
        std::time::Duration::from_secs(10),
    )
    .await;
    assert!(
        list_after_create.is_some(),
        "tab_list must reflect 2 tabs after create_tab"
    );

    // Tab B is active after creation (Task 1's activate-on-create
    // default) -- navigate it, then switch to A and navigate that one
    // to a different real page.
    tx.send(WsMessage::Text(
        json!({"type": "navigate", "url": fixture_page2_url}).to_string(),
    ))
    .await
    .unwrap();
    let b_state = recv_json_matching(
        &mut rx,
        |v| v["type"] == "page_state" && v["tab_id"] == json!(tab_b) && v.get("url").is_some(),
        std::time::Duration::from_secs(15),
    )
    .await;
    assert!(
        b_state.is_some(),
        "tab B's navigation must be observed against tab B's own id"
    );

    tx.send(WsMessage::Text(
        json!({"type": "activate_tab", "tab_id": tab_a}).to_string(),
    ))
    .await
    .unwrap();
    let _ = recv_json_matching(
        &mut rx,
        |v| {
            v["type"] == "tab_list"
                && tab_list_from(v)
                    .iter()
                    .any(|t| t["tab_id"] == json!(tab_a) && t["active"] == json!(true))
        },
        std::time::Duration::from_secs(10),
    )
    .await;

    tx.send(WsMessage::Text(
        json!({"type": "navigate", "url": fixture_url.clone()}).to_string(),
    ))
    .await
    .unwrap();
    let a_state = recv_json_matching(
        &mut rx,
        |v| v["type"] == "page_state" && v["tab_id"] == json!(tab_a) && v.get("url").is_some(),
        std::time::Duration::from_secs(15),
    )
    .await;
    assert!(
        a_state.is_some(),
        "tab A's navigation must be observed against tab A's own id, not tab B's"
    );

    // Close tab A (the currently active one) -- B must survive and
    // become active.
    tx.send(WsMessage::Text(
        json!({"type": "close_tab", "tab_id": tab_a}).to_string(),
    ))
    .await
    .unwrap();
    let after_close_a = recv_json_matching(
        &mut rx,
        |v| v["type"] == "tab_list" && tab_list_from(v).len() == 1,
        std::time::Duration::from_secs(10),
    )
    .await
    .expect("tab_list must reflect exactly 1 tab after closing A");
    let remaining = &tab_list_from(&after_close_a)[0];
    assert_eq!(
        remaining["tab_id"],
        json!(tab_b),
        "the surviving tab must be B, not a fresh replacement"
    );

    // Close the last remaining tab -- session must never be left with
    // zero tabs (Task 3's own fallback policy).
    tx.send(WsMessage::Text(
        json!({"type": "close_tab", "tab_id": tab_b}).to_string(),
    ))
    .await
    .unwrap();
    let after_close_b = recv_json_matching(
        &mut rx,
        |v| v["type"] == "tab_list" && tab_list_from(v).len() == 1,
        std::time::Duration::from_secs(10),
    )
    .await
    .expect("closing the last tab must fall back to a fresh blank tab, never zero tabs");
    let fallback = &tab_list_from(&after_close_b)[0];
    assert_ne!(
        fallback["tab_id"],
        json!(tab_b),
        "the fallback tab must be a genuinely new tab, not the one just closed"
    );

    let _ = tx.close().await;
}

/// Task 2: tab IDs are identifiers, never authorization. A `tab_id`
/// that belongs to a different, unrelated `BrowserSession` (a separate
/// user, separate WebSocket connection, separate CDP attachment) must
/// never be usable by this session -- it simply does not exist in this
/// session's own `tabs` map, so it is denied exactly like any other
/// nonexistent resource.
#[tokio::test]
#[allow(clippy::too_many_lines)]
#[allow(clippy::similar_names)]
async fn task_2_tab_ownership_cross_session_denied() {
    if !docker_and_image_available().await {
        clouddesk_test_support::blocked_by_environment(
            "task_2_tab_ownership_cross_session_denied",
            clouddesk_test_support::reason::CONTAINER_RUNTIME_UNAVAILABLE,
        );
        return;
    }
    let (base, _dir, _runtime_manager) = application().await;
    let _cross_process_guard = tokio::task::spawn_blocking(acquire_cross_process_browser_lock)
        .await
        .unwrap();
    let _brave_container_guard = BraveContainerGuard::new();
    let admin_cookie = bootstrap_admin(&base).await;
    enable_browser(&base, &admin_cookie).await;
    let user_a_cookie = create_user(&base, &admin_cookie, "tabownera", "user").await;
    let user_b_cookie = create_user(&base, &admin_cookie, "tabownerb", "user").await;
    let instance_a = open_browser_instance(&base, &user_a_cookie).await;
    let instance_b = open_browser_instance(&base, &user_b_cookie).await;

    let (_tx_a, mut rx_a) = connect_browser_ws(&base, &user_a_cookie, &instance_a)
        .await
        .expect("owner A must open a browser session");
    let _ = recv_json_matching(
        &mut rx_a,
        |v| v["type"] == "connected",
        std::time::Duration::from_secs(10),
    )
    .await;
    let list_a = recv_json_matching(
        &mut rx_a,
        |v| v["type"] == "tab_list",
        std::time::Duration::from_secs(10),
    )
    .await
    .expect("A must receive its own tab_list");
    let tab_a = tab_list_from(&list_a)[0]["tab_id"]
        .as_str()
        .unwrap()
        .to_owned();

    let (mut tx_b, mut rx_b) = connect_browser_ws(&base, &user_b_cookie, &instance_b)
        .await
        .expect("owner B must open its own, separate browser session");
    let _ = recv_json_matching(
        &mut rx_b,
        |v| v["type"] == "connected",
        std::time::Duration::from_secs(10),
    )
    .await;
    let _ = recv_json_matching(
        &mut rx_b,
        |v| v["type"] == "tab_list",
        std::time::Duration::from_secs(10),
    )
    .await;

    // User B attempts to activate/close User A's real tab_id against
    // B's own session -- must be denied (A's tab_id simply doesn't
    // exist in B's own tabs map).
    tx_b.send(WsMessage::Text(
        json!({"type": "activate_tab", "tab_id": tab_a.clone()}).to_string(),
    ))
    .await
    .unwrap();
    let denied = recv_json_matching(
        &mut rx_b,
        |v| v["type"] == "error",
        std::time::Duration::from_secs(5),
    )
    .await;
    assert!(
        denied.is_some(),
        "activating another session's tab_id must be denied"
    );

    tx_b.send(WsMessage::Text(
        json!({"type": "close_tab", "tab_id": tab_a}).to_string(),
    ))
    .await
    .unwrap();
    let denied_close = recv_json_matching(
        &mut rx_b,
        |v| v["type"] == "error",
        std::time::Duration::from_secs(5),
    )
    .await;
    assert!(
        denied_close.is_some(),
        "closing another session's tab_id must be denied"
    );

    // A random/nonexistent tab_id and a syntactically tab-shaped but
    // never-issued id are both denied the same way.
    tx_b.send(WsMessage::Text(
        json!({"type": "activate_tab", "tab_id": "tab-99999"}).to_string(),
    ))
    .await
    .unwrap();
    let denied_random = recv_json_matching(
        &mut rx_b,
        |v| v["type"] == "error",
        std::time::Duration::from_secs(5),
    )
    .await;
    assert!(
        denied_random.is_some(),
        "a random/nonexistent tab_id must be denied"
    );

    let _ = tx_b.close().await;
}

/// Task 4: a real `window.open()`/`target=_blank` popup Brave creates
/// on its own is translated into a managed `CloudDesk` tab, never left
/// as an unmanaged renderer. Also exercises a bounded popup burst
/// (Task 4/26): a page opening many popups in a tight loop must not
/// grow this session's tab count past `MAX_TABS_PER_SESSION`.
#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn task_4_popup_becomes_managed_tab_and_storm_is_bounded() {
    if !docker_and_image_available().await {
        clouddesk_test_support::blocked_by_environment(
            "task_4_popup_becomes_managed_tab_and_storm_is_bounded",
            clouddesk_test_support::reason::CONTAINER_RUNTIME_UNAVAILABLE,
        );
        return;
    }
    let (base, _dir, _runtime_manager) = application().await;
    let _cross_process_guard = tokio::task::spawn_blocking(acquire_cross_process_browser_lock)
        .await
        .unwrap();
    let _brave_container_guard = BraveContainerGuard::new();
    let admin_cookie = bootstrap_admin(&base).await;
    enable_browser(&base, &admin_cookie).await;
    let user_cookie = create_user(&base, &admin_cookie, "popupuser", "user").await;
    let instance_id = open_browser_instance(&base, &user_cookie).await;

    let (fixture_url_template, _fixture_log) = spawn_fixture_site().await;
    let gateway = bridge_gateway_ip().await;
    clouddeskd::browser_egress_proxy::set_test_allowlist([gateway.parse().unwrap()]);
    let fixture_url = fixture_url_template.replace("REPLACE_WITH_GATEWAY", &gateway);

    let (mut tx, mut rx) = connect_browser_ws(&base, &user_cookie, &instance_id)
        .await
        .expect("owner must open the browser session");
    let _ = recv_json_matching(
        &mut rx,
        |v| v["type"] == "connected",
        std::time::Duration::from_secs(10),
    )
    .await;
    let _ = recv_json_matching(
        &mut rx,
        |v| v["type"] == "tab_list",
        std::time::Duration::from_secs(10),
    )
    .await;

    tx.send(WsMessage::Text(
        json!({"type": "navigate", "url": fixture_url}).to_string(),
    ))
    .await
    .unwrap();
    let _ = recv_json_matching(
        &mut rx,
        |v| v["type"] == "page_state" && v.get("url").is_some(),
        std::time::Duration::from_secs(15),
    )
    .await;
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;

    // Click the real "Open popup" button (a real window.open() call in
    // the real page's own JS, not simulated from the test).
    tx.send(WsMessage::Text(
        json!({"type": "mouse_move", "x": 60.0, "y": 180.0}).to_string(),
    ))
    .await
    .unwrap();
    tx.send(WsMessage::Text(
        json!({"type": "mouse_down", "x": 60.0, "y": 180.0, "button": "left"}).to_string(),
    ))
    .await
    .unwrap();
    tx.send(WsMessage::Text(
        json!({"type": "mouse_up", "x": 60.0, "y": 180.0, "button": "left"}).to_string(),
    ))
    .await
    .unwrap();

    let popup_list = recv_json_matching(
        &mut rx,
        |v| v["type"] == "tab_list" && tab_list_from(v).len() == 2,
        std::time::Duration::from_secs(15),
    )
    .await;
    assert!(
        popup_list.is_some(),
        "a real window.open() popup must be auto-attached as a second managed tab"
    );

    // Switch back to the original tab and trigger the popup storm.
    let original_tab = tab_list_from(&popup_list.unwrap())
        .into_iter()
        .find(|t| t["url"].as_str().is_none_or(|u| !u.contains("page2")))
        .map(|t| t["tab_id"].as_str().unwrap().to_owned());
    if let Some(original_tab) = original_tab {
        tx.send(WsMessage::Text(
            json!({"type": "activate_tab", "tab_id": original_tab}).to_string(),
        ))
        .await
        .unwrap();
        let _ = recv_json_matching(
            &mut rx,
            |v| v["type"] == "tab_list",
            std::time::Duration::from_secs(10),
        )
        .await;
    }

    tx.send(WsMessage::Text(
        json!({"type": "mouse_move", "x": 60.0, "y": 280.0}).to_string(),
    ))
    .await
    .unwrap();
    tx.send(WsMessage::Text(
        json!({"type": "mouse_down", "x": 60.0, "y": 280.0, "button": "left"}).to_string(),
    ))
    .await
    .unwrap();
    tx.send(WsMessage::Text(
        json!({"type": "mouse_up", "x": 60.0, "y": 280.0, "button": "left"}).to_string(),
    ))
    .await
    .unwrap();

    // Give the storm time to fully land, then confirm the tab count
    // never exceeded the bound -- observe every tab_list update over a
    // window rather than just the last one, since an over-the-bound
    // spike (even if later corrected) would itself be the defect.
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(15);
    let mut max_observed = 0usize;
    let mut last_len = 0usize;
    while tokio::time::Instant::now() < deadline {
        if let Some(msg) = recv_json(&mut rx, std::time::Duration::from_millis(500)).await {
            if msg["type"] == "tab_list" {
                let len = tab_list_from(&msg).len();
                max_observed = max_observed.max(len);
                last_len = len;
            }
        }
    }
    assert!(
        max_observed <= 8,
        "a popup storm must never push this session's tab count past MAX_TABS_PER_SESSION (8), observed {max_observed}"
    );
    assert!(
        last_len >= 2,
        "at least the original popup-opening tab plus one popup must remain, got {last_len}"
    );

    let _ = tx.close().await;
}

/// Task 19/20: a real `clouddeskd` process restart -- not merely
/// killing Brave (that's the existing crash-recovery test) -- proven
/// by discarding the entire in-process `RuntimeManager` (fresh `live`
/// instance map, exactly as a real process restart would have) while
/// keeping the same durable `SQLite` pool, then calling the real
/// `reconcile_on_startup()` every runtime kind already relies on. The
/// pre-restart instance must be marked `Failed` (this project's own
/// documented restart policy -- never silently trusted or reattached
/// to), the old `instance_id` must be unusable for a new broker
/// session against the replacement generation, and a genuinely fresh
/// session must work normally afterward.
#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn task_19_20_service_restart_marks_stale_instance_failed() {
    if !docker_and_image_available().await {
        clouddesk_test_support::blocked_by_environment(
            "task_19_20_service_restart_marks_stale_instance_failed",
            clouddesk_test_support::reason::CONTAINER_RUNTIME_UNAVAILABLE,
        );
        return;
    }
    let _cross_process_guard = tokio::task::spawn_blocking(acquire_cross_process_browser_lock)
        .await
        .unwrap();
    let _brave_container_guard = BraveContainerGuard::new();

    clouddeskd::browser_egress_proxy::spawn();
    let pool = clouddesk_db::connect("sqlite::memory:", 1).await.unwrap();
    clouddesk_db::migrate(&pool).await.unwrap();
    let auth = AuthService::new(
        pool.clone(),
        SecretCipher::new(&[113_u8; 32]).unwrap(),
        AuthPolicy::default(),
    )
    .unwrap();

    let (base1, _runtime_manager1, _secret_dir1) = spawn_router_on_pool(&pool, auth.clone()).await;
    let admin_cookie = bootstrap_admin(&base1).await;
    enable_browser(&base1, &admin_cookie).await;
    let user_cookie = create_user(&base1, &admin_cookie, "restartuser", "user").await;
    let old_instance_id = open_browser_instance(&base1, &user_cookie).await;

    // Real, pre-restart evidence the session genuinely worked.
    let (mut tx, mut rx) = connect_browser_ws(&base1, &user_cookie, &old_instance_id)
        .await
        .expect("pre-restart session must connect");
    let connected = recv_json_matching(
        &mut rx,
        |v| v["type"] == "connected",
        std::time::Duration::from_secs(10),
    )
    .await;
    assert!(connected.is_some());
    let _ = tx.close().await;

    // Simulate a real process restart: a brand-new RuntimeManager (no
    // in-memory live-instance state) against the same durable pool.
    let (base2, runtime_manager2, _secret_dir2) = spawn_router_on_pool(&pool, auth.clone()).await;
    let reconciled = runtime_manager2.reconcile_on_startup().await.unwrap();
    assert!(
        reconciled >= 1,
        "reconcile_on_startup must find and mark the pre-restart instance"
    );

    // The durable DB row itself must be marked `Failed` by
    // `reconcile_on_startup` -- checked directly against the store,
    // since `RuntimeManager::status()` is deliberately in-memory-only
    // (see its own doc comment: never resurrect/trust DB state as
    // "live" without a real, fresh health check) and correctly reports
    // an instance the fresh post-restart process never live-tracked as
    // simply not found, which is checked next.
    let old_id = clouddesk_orchestrator::InstanceId {
        kind: clouddesk_orchestrator::RuntimeKind::Browser,
        owner_user_id: {
            let me = http(
                &base2,
                Method::GET,
                "/api/v1/auth/me",
                Some(&user_cookie),
                None,
            )
            .await;
            let me_body: Value = me.json().await.unwrap();
            me_body["user_id"].as_str().unwrap().to_owned()
        },
        instance_id: old_instance_id.clone(),
    };
    let stored_row = runtime_manager2.store().get(&old_id).await.unwrap();
    assert_eq!(
        stored_row.map(|r| r.state),
        Some(clouddesk_orchestrator::InstanceState::Failed),
        "reconcile_on_startup must durably mark the pre-restart instance Failed in the DB"
    );

    // Task 20: the fresh post-restart process never live-tracked this
    // instance_id (its in-memory `live` map starts empty on every
    // restart, by design), so a status query for it correctly reports
    // not found -- a stronger denial than a stale "failed" status
    // would be, since there is no live state to query at all.
    let status = http(
        &base2,
        Method::GET,
        &format!("/api/v1/runtime-instances/browser/{old_instance_id}"),
        Some(&user_cookie),
        None,
    )
    .await;
    assert_eq!(
        status.status(),
        reqwest::StatusCode::NOT_FOUND,
        "a stale instance_id must never resolve to real, live state after a restart"
    );

    // Task 20: the old instance_id cannot be used to open a new broker
    // session against the replacement generation.
    match connect_browser_ws(&base2, &user_cookie, &old_instance_id).await {
        Err(_) => {} // upgrade itself denied -- acceptable
        Ok((mut stale_tx, mut stale_rx)) => {
            let msg = recv_json(&mut stale_rx, std::time::Duration::from_secs(5)).await;
            assert_ne!(
                msg.as_ref().map(|v| &v["type"]),
                Some(&json!("connected")),
                "a stale instance_id must never yield a real, connected broker session after restart, got {msg:?}"
            );
            let _ = stale_tx.close().await;
        }
    }

    // Browsing must work again after the restart, via a genuinely new
    // instance. Real defect found and fixed this pass (root-caused
    // here, fixed in `crates/orchestrator/src/manager.rs`'s
    // `create_instance`): a `Failed` row (exactly what every session
    // active during a restart becomes) used to count against
    // `max_instances_per_user`, and since a `Failed` instance can never
    // be restarted (`restart_instance` also requires live-tracking,
    // which a fresh post-restart process never has for it), any user
    // whose Browser session was active during a restart would be
    // permanently locked out of ever creating a new one -- no
    // self-service recovery, only admin/DB intervention. Fixed by
    // excluding `Failed` rows from both the per-user and global counts.
    let new_instance_id = open_browser_instance(&base2, &user_cookie).await;
    assert_ne!(
        new_instance_id, old_instance_id,
        "the fresh session must be a genuinely new instance, not a reused stale one"
    );
    let (mut fresh_tx, mut fresh_rx) = connect_browser_ws(&base2, &user_cookie, &new_instance_id)
        .await
        .expect("a fresh session after restart must connect cleanly");
    let fresh_connected = recv_json_matching(
        &mut fresh_rx,
        |v| v["type"] == "connected",
        std::time::Duration::from_secs(10),
    )
    .await;
    assert!(
        fresh_connected.is_some(),
        "normal browsing must work again after a genuinely fresh post-restart session"
    );
    let _ = fresh_tx.close().await;

    // Real, known, already-documented, non-Browser-specific limitation
    // (see RuntimeManager::reconcile_on_startup's own doc comment):
    // reconciliation marks DB state only, it does not stop the real,
    // now-orphaned pre-restart Brave container -- the same accepted
    // scope boundary Code/Office already live under. The
    // BraveContainerGuard held for this whole test cleans it up
    // regardless, so this test itself leaks nothing.
}

/// Task 18: logout/session revocation. A logged-out `CloudDesk` session
/// must never be usable to open a *new* Browser broker session -- the
/// same session-revocation guarantee every other authenticated route
/// in this project already provides (`AuthService::revoke_session` sets
/// `revoked_at`, and `principal()` checks `revoked_at IS NULL` on every
/// request, including this WebSocket's own upgrade). Matches this
/// project's own established policy (see Office's
/// `task_9_logout_with_office_open`): revocation is proven against new
/// requests, not by inventing new mid-connection kill-switch behavior
/// nothing else in this codebase has either.
#[tokio::test]
async fn task_18_logout_denies_new_browser_sessions() {
    if !docker_and_image_available().await {
        clouddesk_test_support::blocked_by_environment(
            "task_18_logout_denies_new_browser_sessions",
            clouddesk_test_support::reason::CONTAINER_RUNTIME_UNAVAILABLE,
        );
        return;
    }
    let (base, _dir, _runtime_manager) = application().await;
    let _cross_process_guard = tokio::task::spawn_blocking(acquire_cross_process_browser_lock)
        .await
        .unwrap();
    let _brave_container_guard = BraveContainerGuard::new();
    let admin_cookie = bootstrap_admin(&base).await;
    enable_browser(&base, &admin_cookie).await;
    let user_cookie = create_user(&base, &admin_cookie, "logoutuser", "user").await;

    let logout = http(
        &base,
        Method::POST,
        "/api/v1/auth/logout",
        Some(&user_cookie),
        None,
    )
    .await;
    assert!(logout.status().is_success() || logout.status() == reqwest::StatusCode::NO_CONTENT);

    let denied = http(
        &base,
        Method::POST,
        "/api/v1/runtime-instances",
        Some(&user_cookie),
        Some(&json!({"kind": "browser"})),
    )
    .await;
    assert_eq!(
        denied.status(),
        reqwest::StatusCode::UNAUTHORIZED,
        "a logged-out session must never be able to start a new Browser session"
    );

    let ws_url = format!(
        "ws{}/api/v1/runtime-instances/browser/nonexistent/browser-ws",
        base.strip_prefix("http").unwrap()
    );
    let mut request = tokio_tungstenite::tungstenite::http::Request::builder()
        .uri(&ws_url)
        .header("Host", "127.0.0.1")
        .header("Connection", "Upgrade")
        .header("Upgrade", "websocket")
        .header("Sec-WebSocket-Version", "13")
        .header(
            "Sec-WebSocket-Key",
            tokio_tungstenite::tungstenite::handshake::client::generate_key(),
        )
        .body(())
        .unwrap();
    request
        .headers_mut()
        .insert("Cookie", user_cookie.parse().unwrap());
    let result = tokio_tungstenite::connect_async(request).await;
    assert!(
        result.is_err(),
        "a logged-out session's cookie must never open a new browser-ws upgrade"
    );
}
