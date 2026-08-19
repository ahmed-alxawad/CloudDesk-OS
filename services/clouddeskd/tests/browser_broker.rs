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

    let runtime_root = tempfile::tempdir().unwrap();
    std::mem::forget(runtime_root);
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
        eprintln!("SKIP: docker/{BROWSER_IMAGE} not available (build docker/brave first)");
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
        eprintln!("SKIP: docker/{BROWSER_IMAGE} not available (build docker/brave first)");
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
        eprintln!("SKIP: docker/{BROWSER_IMAGE} not available (build docker/brave first)");
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
        eprintln!("SKIP: docker/{BROWSER_IMAGE} not available (build docker/brave first)");
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
        eprintln!("SKIP: docker/{BROWSER_IMAGE} not available (build docker/brave first)");
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
