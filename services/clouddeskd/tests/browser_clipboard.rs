//! Phase 9 Pass 3B: real, live Browser clipboard evidence through the
//! actual product path. Task 14's chosen v1 policy: `clipboard_write`
//! delivers text into the active tab's currently focused element via
//! CDP `Input.insertText` (a real paste, session-scoped, never a host
//! clipboard write); `clipboard_read` returns the active tab's current
//! page selection (`window.getSelection()`), never a real OS
//! clipboard read -- this sidesteps the Web Clipboard API's secure-
//! context/user-activation requirements, which would otherwise
//! silently fail on the plain-`http` fixtures these tests must use.

use axum::http::Method;
use clouddesk_auth::{AuthPolicy, AuthService};
use clouddesk_secrets::SecretCipher;
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::sync::Arc;
use tokio::process::Command as TokioCommand;
use tokio::sync::Mutex as AsyncMutex;
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

async fn application() -> (String, tempfile::TempDir) {
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
    std::fs::write(&secret_path, "browser-clipboard-test-secret\n").unwrap();

    let runtime_root = tempfile::tempdir().unwrap();
    std::mem::forget(runtime_root);
    let runtime_manager = std::sync::Arc::new(
        clouddesk_orchestrator::RuntimeManager::new(
            clouddesk_orchestrator::store::RuntimeStore::new(pool.clone()),
            std::env::temp_dir().join(format!(
                "clouddesk-browser-clipboard-test-{}",
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
        Some(runtime_manager),
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
    (format!("http://127.0.0.1:{port}"), directory)
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
            "secret": "browser-clipboard-test-secret",
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

type WsSink = futures_util::stream::SplitSink<
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
    WsMessage,
>;
type WsSource = futures_util::stream::SplitStream<
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
>;

async fn connect_browser_ws(base: &str, cookie: &str, instance_id: &str) -> (WsSink, WsSource) {
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
    let (stream, _) = tokio_tungstenite::connect_async(request)
        .await
        .expect("owner must connect");
    stream.split()
}

async fn recv_json_matching(
    stream: &mut WsSource,
    predicate: impl Fn(&Value) -> bool,
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
                    if predicate(&v) {
                        return Some(v);
                    }
                }
            }
            Ok(Some(Ok(_))) => {}
            _ => return None,
        }
    }
}

async fn navigate(tx: &mut WsSink, rx: &mut WsSource, url: &str) {
    tx.send(WsMessage::Text(
        json!({"type": "navigate", "url": url}).to_string(),
    ))
    .await
    .unwrap();
    let _ = recv_json_matching(
        rx,
        |v| v["type"] == "page_state" && v.get("url").is_some(),
        std::time::Duration::from_secs(10),
    )
    .await;
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
}

async fn gateway_ip() -> String {
    let output = TokioCommand::new("docker")
        .args([
            "network",
            "inspect",
            "clouddesk-browser-net",
            "--format",
            "{{(index .IPAM.Config 0).Gateway}}",
        ])
        .output()
        .await
        .unwrap();
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

type ReceivedText = Arc<AsyncMutex<Option<String>>>;

/// A real page with a real focused, empty text input (paste target)
/// and a preselected span of known text (copy source): posts the
/// input's live value to `/received` on every keystroke it gets from
/// a real CDP `Input.insertText`.
const CLIPBOARD_PAGE: &str = "<!doctype html><html><body>\
<input id=\"i\" autofocus>\
<span id=\"s\">preselected-copy-source-text</span>\
<script>\
document.getElementById('i').focus();\
const sel = window.getSelection();\
sel.removeAllRanges();\
const range = document.createRange();\
range.selectNodeContents(document.getElementById('s'));\
sel.addRange(range);\
document.getElementById('i').addEventListener('input', (e) => {\
  fetch('/received', {method: 'POST', body: e.target.value});\
});\
</script></body></html>";

async fn received_route(
    state: axum::extract::State<ReceivedText>,
    body: axum::body::Bytes,
) -> axum::http::StatusCode {
    *state.lock().await = Some(String::from_utf8_lossy(&body).into_owned());
    axum::http::StatusCode::NO_CONTENT
}

async fn spawn_clipboard_fixture() -> (u16, ReceivedText) {
    let received: ReceivedText = Arc::new(AsyncMutex::new(None));
    let router = axum::Router::new()
        .route(
            "/",
            axum::routing::get(|| async { axum::response::Html(CLIPBOARD_PAGE) }),
        )
        .route("/received", axum::routing::post(received_route))
        .with_state(received.clone());
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
    (port, received)
}

async fn poll_received(received: &ReceivedText, timeout: std::time::Duration) -> Option<String> {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if let Some(v) = received.lock().await.clone() {
            return Some(v);
        }
        if tokio::time::Instant::now() >= deadline {
            return None;
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
}

/// Task 16: real `clipboard_write` (paste) through the actual product
/// path -- a known string is delivered into the real focused input on
/// a real page, verified via the page's own reported live value.
#[tokio::test]
async fn task_16_real_clipboard_write_paste() {
    if !docker_and_image_available().await {
        eprintln!("SKIP: docker/{BROWSER_IMAGE} not available (build docker/brave first)");
        return;
    }
    let (base, _dir) = application().await;
    let _cross_process_guard = tokio::task::spawn_blocking(acquire_cross_process_browser_lock)
        .await
        .unwrap();
    let _brave_container_guard = BraveContainerGuard::new();

    let (fixture_port, received) = spawn_clipboard_fixture().await;
    let gw = gateway_ip().await;
    clouddeskd::browser_egress_proxy::set_test_allowlist([gw.parse().unwrap()]);
    let fixture_url = format!("http://{gw}:{fixture_port}/");

    let admin_cookie = bootstrap_admin(&base).await;
    enable_browser(&base, &admin_cookie).await;
    let user_cookie = create_user(&base, &admin_cookie, "clipwriteuser", "user").await;
    let instance_id = open_browser_instance(&base, &user_cookie).await;
    let (mut tx, mut rx) = connect_browser_ws(&base, &user_cookie, &instance_id).await;
    let _ = recv_json_matching(
        &mut rx,
        |v| v["type"] == "connected",
        std::time::Duration::from_secs(10),
    )
    .await;
    navigate(&mut tx, &mut rx, &fixture_url).await;

    let known_text = "CloudDesk clipboard paste 2026 \u{1F510} unicode ok";
    tx.send(WsMessage::Text(
        json!({"type": "clipboard_write", "text": known_text}).to_string(),
    ))
    .await
    .unwrap();
    let ack = recv_json_matching(
        &mut rx,
        |v| v["type"] == "clipboard_write_ok" || v["type"] == "error",
        std::time::Duration::from_secs(10),
    )
    .await;
    assert_eq!(
        ack.map(|v| v["type"].clone()),
        Some(json!("clipboard_write_ok"))
    );

    let value = poll_received(&received, std::time::Duration::from_secs(10))
        .await
        .expect("the focused input must actually receive the pasted text");
    assert_eq!(value, known_text);

    let _ = tx.close().await;
}

/// Task 16: real `clipboard_read` (copy) through the actual product
/// path -- returns the real page's current selection.
#[tokio::test]
async fn task_16_real_clipboard_read_copy() {
    if !docker_and_image_available().await {
        eprintln!("SKIP: docker/{BROWSER_IMAGE} not available (build docker/brave first)");
        return;
    }
    let (base, _dir) = application().await;
    let _cross_process_guard = tokio::task::spawn_blocking(acquire_cross_process_browser_lock)
        .await
        .unwrap();
    let _brave_container_guard = BraveContainerGuard::new();

    let (fixture_port, _received) = spawn_clipboard_fixture().await;
    let gw = gateway_ip().await;
    clouddeskd::browser_egress_proxy::set_test_allowlist([gw.parse().unwrap()]);
    let fixture_url = format!("http://{gw}:{fixture_port}/");

    let admin_cookie = bootstrap_admin(&base).await;
    enable_browser(&base, &admin_cookie).await;
    let user_cookie = create_user(&base, &admin_cookie, "clipreaduser", "user").await;
    let instance_id = open_browser_instance(&base, &user_cookie).await;
    let (mut tx, mut rx) = connect_browser_ws(&base, &user_cookie, &instance_id).await;
    let _ = recv_json_matching(
        &mut rx,
        |v| v["type"] == "connected",
        std::time::Duration::from_secs(10),
    )
    .await;
    navigate(&mut tx, &mut rx, &fixture_url).await;

    tx.send(WsMessage::Text(
        json!({"type": "clipboard_read"}).to_string(),
    ))
    .await
    .unwrap();
    let read = recv_json_matching(
        &mut rx,
        |v| v["type"] == "clipboard_read" || v["type"] == "error",
        std::time::Duration::from_secs(10),
    )
    .await
    .expect("a real clipboard_read response must arrive");
    assert_eq!(read["type"], json!("clipboard_read"));
    assert_eq!(read["text"], json!("preselected-copy-source-text"));

    let _ = tx.close().await;
}

/// Task 17: hostile-input bound -- an oversized clipboard write must
/// be rejected cleanly, not silently truncated or unboundedly
/// allocated.
#[tokio::test]
async fn task_17_clipboard_write_size_bound_enforced() {
    if !docker_and_image_available().await {
        eprintln!("SKIP: docker/{BROWSER_IMAGE} not available (build docker/brave first)");
        return;
    }
    let (base, _dir) = application().await;
    let _cross_process_guard = tokio::task::spawn_blocking(acquire_cross_process_browser_lock)
        .await
        .unwrap();
    let _brave_container_guard = BraveContainerGuard::new();

    let (fixture_port, _received) = spawn_clipboard_fixture().await;
    let gw = gateway_ip().await;
    clouddeskd::browser_egress_proxy::set_test_allowlist([gw.parse().unwrap()]);
    let fixture_url = format!("http://{gw}:{fixture_port}/");

    let admin_cookie = bootstrap_admin(&base).await;
    enable_browser(&base, &admin_cookie).await;
    let user_cookie = create_user(&base, &admin_cookie, "clipbounduser", "user").await;
    let instance_id = open_browser_instance(&base, &user_cookie).await;
    let (mut tx, mut rx) = connect_browser_ws(&base, &user_cookie, &instance_id).await;
    let _ = recv_json_matching(
        &mut rx,
        |v| v["type"] == "connected",
        std::time::Duration::from_secs(10),
    )
    .await;
    navigate(&mut tx, &mut rx, &fixture_url).await;

    // Just over the app-level 1,000,000-byte clipboard bound, but
    // still comfortably under the WS transport's own 1 MiB single-
    // frame limit (`MAX_RUNTIME_WS_FRAME_BYTES`), so this exercises
    // the clipboard-specific bound rather than the unrelated,
    // already-covered transport frame limit.
    let oversized = "a".repeat(1_010_000);
    tx.send(WsMessage::Text(
        json!({"type": "clipboard_write", "text": oversized}).to_string(),
    ))
    .await
    .unwrap();
    let ack = recv_json_matching(
        &mut rx,
        |v| v["type"] == "clipboard_write_ok" || v["type"] == "error",
        std::time::Duration::from_secs(10),
    )
    .await;
    assert_eq!(
        ack.map(|v| v["type"].clone()),
        Some(json!("error")),
        "an oversized clipboard write must be rejected"
    );

    let _ = tx.close().await;
}
