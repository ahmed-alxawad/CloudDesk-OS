//! Phase 9 Pass 3A-3 (see `PHASE9_BROWSER_EVIDENCE.md`): real cookie
//! persistence evidence through the actual `CloudDesk` Browser product
//! path -- not `localStorage`, and not a raw CDP injection. A
//! controlled fixture site sends a real `Set-Cookie` response header;
//! the browser must send it back on a subsequent real HTTP request,
//! and that must still be true after a real stop/restart of the same
//! persistent profile.

use axum::extract::State;
use axum::http::Method;
use axum::response::IntoResponse;
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
        SecretCipher::new(&[127_u8; 32]).unwrap(),
        AuthPolicy::default(),
    )
    .unwrap();
    let directory = tempfile::tempdir().unwrap();
    let secret_path = directory.path().join("bootstrap.secret");
    std::fs::write(&secret_path, "browser-cookies-test-secret\n").unwrap();

    let runtime_root = tempfile::tempdir().unwrap();
    std::mem::forget(runtime_root);
    let runtime_manager = std::sync::Arc::new(
        clouddesk_orchestrator::RuntimeManager::new(
            clouddesk_orchestrator::store::RuntimeStore::new(pool.clone()),
            std::env::temp_dir().join(format!(
                "clouddesk-browser-cookies-test-{}",
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
            "secret": "browser-cookies-test-secret",
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
        Some(&json!({"username": username, "display_name": username, "password": "user horse battery staple", "role_ids": [role_id]})),
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

/// A fixture that sets a real, genuinely persistent `Set-Cookie`
/// (`Max-Age`, not a session cookie) on every request, and records the
/// `Cookie` header it received on each one -- proving a subsequent
/// request from the same real browser profile actually sent the cookie
/// back, which is the only way to prove real HTTP cookie persistence
/// through the product path (not a CDP-level injection, not
/// `localStorage`).
#[derive(Default)]
struct CookieFixtureLog {
    received_cookie_headers: Vec<Option<String>>,
}

async fn fixture_root(
    State(log): State<Arc<TokioMutex<CookieFixtureLog>>>,
    headers: axum::http::HeaderMap,
) -> impl IntoResponse {
    let received = headers
        .get(axum::http::header::COOKIE)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    log.lock().await.received_cookie_headers.push(received);
    (
        [(
            axum::http::header::SET_COOKIE,
            "sentinel=cookie-persist-value; Max-Age=86400; Path=/",
        )],
        axum::response::Html("<!doctype html><html><body>cookie fixture</body></html>"),
    )
}

async fn spawn_cookie_fixture_site() -> (String, Arc<TokioMutex<CookieFixtureLog>>) {
    let log = Arc::new(TokioMutex::new(CookieFixtureLog::default()));
    let router = axum::Router::new()
        .route("/", axum::routing::get(fixture_root))
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

async fn navigate_and_wait(
    tx: &mut futures_util::stream::SplitSink<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
        WsMessage,
    >,
    rx: &mut futures_util::stream::SplitStream<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
    >,
    url: &str,
) {
    tx.send(WsMessage::Text(
        json!({"type": "navigate", "url": url}).to_string(),
    ))
    .await
    .unwrap();
    let _ = recv_json_matching(
        rx,
        |v| v["type"] == "page_state" && v.get("url").is_some(),
        std::time::Duration::from_secs(15),
    )
    .await;
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
}

/// Tasks 1/4/5/6 (Phase 9 Pass 3A-3): the full, real cookie-persistence
/// live matrix through the actual product path. A real `Set-Cookie`
/// from a controlled fixture; a real second visit proving the browser
/// sent it back; a real `stop`/`restart` of the same persistent
/// profile (exercising the real `graceful_stop` CDP `Browser.close`
/// fix); a real revisit after restart proving the cookie survived;
/// cross-user isolation (User B never sees User A's cookie); and Guest
/// ephemeral cleanup (the cookie disappears once the Guest's ephemeral
/// profile is torn down and a fresh Guest session starts).
#[tokio::test]
#[allow(clippy::too_many_lines, clippy::similar_names)]
async fn task_1_4_5_6_cookie_persistence_live_matrix() {
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
    let user_a_cookie = create_user(&base, &admin_cookie, "cookieuserA", "user").await;
    let user_b_cookie = create_user(&base, &admin_cookie, "cookieuserB", "user").await;
    let guest_cookie = create_user(&base, &admin_cookie, "cookieguest", "guest").await;

    let (fixture_url_template, fixture_log) = spawn_cookie_fixture_site().await;
    let gateway = bridge_gateway_ip().await;
    clouddeskd::browser_egress_proxy::set_test_allowlist([gateway.parse().unwrap()]);
    let fixture_url = fixture_url_template.replace("REPLACE_WITH_GATEWAY", &gateway);

    // -- User A: set, verify sent-back, stop, restart, verify survives --
    let instance_a = open_browser_instance(&base, &user_a_cookie).await;
    {
        let (mut tx, mut rx) = connect_browser_ws(&base, &user_a_cookie, &instance_a)
            .await
            .expect("A must connect");
        let _ = recv_json_matching(
            &mut rx,
            |v| v["type"] == "connected",
            std::time::Duration::from_secs(10),
        )
        .await;
        navigate_and_wait(&mut tx, &mut rx, &fixture_url).await;
        navigate_and_wait(&mut tx, &mut rx, &fixture_url).await;
        let _ = tx.close().await;
    }
    {
        let log = fixture_log.lock().await;
        let sent_back = log.received_cookie_headers.iter().any(|c| {
            c.as_deref()
                .is_some_and(|c| c.contains("sentinel=cookie-persist-value"))
        });
        assert!(sent_back, "the real Brave browser must send the real Set-Cookie value back on a subsequent request, got {:?}", log.received_cookie_headers);
    }

    let stop = http(
        &base,
        Method::POST,
        &format!("/api/v1/runtime-instances/browser/{instance_a}/stop"),
        Some(&user_a_cookie),
        None,
    )
    .await;
    assert_eq!(stop.status(), reqwest::StatusCode::NO_CONTENT);
    let mut stopped = false;
    for _ in 0..30 {
        let status = http(
            &base,
            Method::GET,
            &format!("/api/v1/runtime-instances/browser/{instance_a}"),
            Some(&user_a_cookie),
            None,
        )
        .await;
        let body: Value = status.json().await.unwrap();
        if matches!(body["state"].as_str(), Some("stopped" | "failed")) {
            stopped = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    }
    assert!(
        stopped,
        "instance must settle into a terminal state after stop"
    );

    let restart = http(
        &base,
        Method::POST,
        &format!("/api/v1/runtime-instances/browser/{instance_a}/restart"),
        Some(&user_a_cookie),
        None,
    )
    .await;
    assert_eq!(restart.status(), reqwest::StatusCode::OK);
    assert!(wait_for_running(&base, &user_a_cookie, &instance_a).await);

    {
        let mut log = fixture_log.lock().await;
        log.received_cookie_headers.clear();
        drop(log);
        let (mut tx, mut rx) = connect_browser_ws(&base, &user_a_cookie, &instance_a)
            .await
            .expect("A must reconnect after restart");
        let _ = recv_json_matching(
            &mut rx,
            |v| v["type"] == "connected",
            std::time::Duration::from_secs(10),
        )
        .await;
        navigate_and_wait(&mut tx, &mut rx, &fixture_url).await;
        let _ = tx.close().await;
        let log = fixture_log.lock().await;
        let survived = log.received_cookie_headers.iter().any(|c| {
            c.as_deref()
                .is_some_and(|c| c.contains("sentinel=cookie-persist-value"))
        });
        assert!(survived, "COOKIE PERSISTENCE: the real cookie must survive a real stop/restart of the same persistent profile, got {:?}", log.received_cookie_headers);
    }

    // -- User B: cross-user isolation --
    {
        let mut log = fixture_log.lock().await;
        log.received_cookie_headers.clear();
        drop(log);
        let instance_b = open_browser_instance(&base, &user_b_cookie).await;
        let (mut tx, mut rx) = connect_browser_ws(&base, &user_b_cookie, &instance_b)
            .await
            .expect("B must connect");
        let _ = recv_json_matching(
            &mut rx,
            |v| v["type"] == "connected",
            std::time::Duration::from_secs(10),
        )
        .await;
        navigate_and_wait(&mut tx, &mut rx, &fixture_url).await;
        let _ = tx.close().await;
        let log = fixture_log.lock().await;
        let leaked = log.received_cookie_headers.iter().any(|c| {
            c.as_deref()
                .is_some_and(|c| c.contains("sentinel=cookie-persist-value"))
        });
        assert!(
            !leaked,
            "CROSS-USER COOKIE ISOLATION: User B must never send User A's real cookie, got {:?}",
            log.received_cookie_headers
        );
    }

    // -- Guest: ephemeral -- set, restart same instance (documented
    // instance-reuse gap, see browser_runtime.rs's own tests), verify
    // absent.
    {
        let mut log = fixture_log.lock().await;
        log.received_cookie_headers.clear();
        drop(log);
        let instance_guest = open_browser_instance(&base, &guest_cookie).await;
        {
            let (mut tx, mut rx) = connect_browser_ws(&base, &guest_cookie, &instance_guest)
                .await
                .expect("Guest must connect");
            let _ = recv_json_matching(
                &mut rx,
                |v| v["type"] == "connected",
                std::time::Duration::from_secs(10),
            )
            .await;
            navigate_and_wait(&mut tx, &mut rx, &fixture_url).await;
            let _ = tx.close().await;
        }
        let stop = http(
            &base,
            Method::POST,
            &format!("/api/v1/runtime-instances/browser/{instance_guest}/stop"),
            Some(&guest_cookie),
            None,
        )
        .await;
        assert_eq!(stop.status(), reqwest::StatusCode::NO_CONTENT);
        let mut stopped = false;
        for _ in 0..30 {
            let status = http(
                &base,
                Method::GET,
                &format!("/api/v1/runtime-instances/browser/{instance_guest}"),
                Some(&guest_cookie),
                None,
            )
            .await;
            let body: Value = status.json().await.unwrap();
            if matches!(body["state"].as_str(), Some("stopped" | "failed")) {
                stopped = true;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        }
        assert!(stopped);
        let restart = http(
            &base,
            Method::POST,
            &format!("/api/v1/runtime-instances/browser/{instance_guest}/restart"),
            Some(&guest_cookie),
            None,
        )
        .await;
        assert_eq!(restart.status(), reqwest::StatusCode::OK);
        assert!(wait_for_running(&base, &guest_cookie, &instance_guest).await);

        let mut log = fixture_log.lock().await;
        log.received_cookie_headers.clear();
        drop(log);
        let (mut tx, mut rx) = connect_browser_ws(&base, &guest_cookie, &instance_guest)
            .await
            .expect("Guest must reconnect");
        let _ = recv_json_matching(
            &mut rx,
            |v| v["type"] == "connected",
            std::time::Duration::from_secs(10),
        )
        .await;
        navigate_and_wait(&mut tx, &mut rx, &fixture_url).await;
        let _ = tx.close().await;
        let log = fixture_log.lock().await;
        let survived = log.received_cookie_headers.iter().any(|c| {
            c.as_deref()
                .is_some_and(|c| c.contains("sentinel=cookie-persist-value"))
        });
        assert!(!survived, "GUEST COOKIE CLEANUP: a Guest's ephemeral profile must never retain a cookie across restart, got {:?}", log.received_cookie_headers);
    }
}
