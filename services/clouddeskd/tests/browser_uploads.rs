//! Phase 9 Pass 3B: real, live Browser upload/file-chooser evidence
//! through the actual product path -- a real controlled fixture page
//! with a real `<input type=file>`, a real CDP-intercepted file
//! chooser, a real CloudDesk-authorized local file selected via the
//! broker's typed `select_file` message, and the real bytes verified
//! server-side at a controlled receiver endpoint (never a raw-CDP
//! shortcut, never trusting client-reported state).

use axum::http::Method;
use axum::response::IntoResponse;
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
    std::fs::write(&secret_path, "browser-uploads-test-secret\n").unwrap();

    let runtime_manager = std::sync::Arc::new(
        clouddesk_orchestrator::RuntimeManager::new(
            clouddesk_orchestrator::store::RuntimeStore::new(pool.clone()),
            std::env::temp_dir().join(format!(
                "clouddesk-browser-uploads-test-{}",
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
        false,
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
            "secret": "browser-uploads-test-secret",
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

/// A real page with a real `<input type=file>` at a known screen
/// position; on selection it uploads the chosen file's real bytes to
/// a controlled `/received` endpoint via `fetch`, so the test can
/// verify actual delivered content rather than trusting the page.
const CHOOSER_PAGE: &str = "<!doctype html><html><body>\
<input id=\"fi\" type=\"file\" style=\"position:absolute;left:5px;top:5px;width:100px;height:30px;\">\
<script>\
document.getElementById('fi').addEventListener('change', async (e) => {\
  const f = e.target.files[0];\
  const buf = await f.arrayBuffer();\
  await fetch('/received', {method: 'POST', headers: {'X-Filename': f.name}, body: buf});\
});\
</script></body></html>";

type ReceivedFile = Arc<AsyncMutex<Option<(String, Vec<u8>)>>>;

async fn received_route(
    headers: axum::http::HeaderMap,
    state: axum::extract::State<ReceivedFile>,
    body: axum::body::Bytes,
) -> impl IntoResponse {
    let filename = headers
        .get("X-Filename")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    *state.lock().await = Some((filename, body.to_vec()));
    axum::http::StatusCode::NO_CONTENT
}

async fn spawn_chooser_fixture() -> (u16, ReceivedFile) {
    let received = Arc::new(AsyncMutex::new(None));
    let router = axum::Router::new()
        .route(
            "/",
            axum::routing::get(|| async { axum::response::Html(CHOOSER_PAGE) }),
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

/// Click the real file input at its known fixture position to trigger
/// a real, CDP-intercepted `Page.fileChooserOpened` event.
async fn click_file_input(tx: &mut WsSink) {
    for (action, extra) in [
        ("mouse_move", json!({})),
        ("mouse_down", json!({"button": "left"})),
        ("mouse_up", json!({"button": "left"})),
    ] {
        let mut msg = json!({"type": action, "x": 30.0, "y": 15.0});
        for (k, v) in extra.as_object().unwrap() {
            msg[k] = v.clone();
        }
        tx.send(WsMessage::Text(msg.to_string())).await.unwrap();
    }
}

async fn poll_received(
    received: &ReceivedFile,
    timeout: std::time::Duration,
) -> Option<(String, Vec<u8>)> {
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

/// Tasks 9/10: the real upload/file-chooser flow end to end -- a real
/// `<input type=file>` click intercepted by CDP, a real
/// `select_file` broker message choosing a real, CloudDesk-authorized
/// local file, and the real, correct bytes delivered to the website
/// (never a raw filesystem path exposed to the site).
#[tokio::test]
async fn task_9_10_real_upload_flow_and_hash() {
    if !docker_and_image_available().await {
        clouddesk_test_support::blocked_by_environment(
            "task_9_10_real_upload_flow_and_hash",
            clouddesk_test_support::reason::CONTAINER_RUNTIME_UNAVAILABLE,
        );
        return;
    }
    let (base, _dir) = application().await;
    let _cross_process_guard = tokio::task::spawn_blocking(acquire_cross_process_browser_lock)
        .await
        .unwrap();
    let _brave_container_guard = BraveContainerGuard::new();

    let (fixture_port, received) = spawn_chooser_fixture().await;
    let gw = gateway_ip().await;
    clouddeskd::browser_egress_proxy::set_test_allowlist([gw.parse().unwrap()]);
    let fixture_url = format!("http://{gw}:{fixture_port}/");

    let home = current_process_linux_identity().unwrap().home;
    let source_name = "cloudesk-upload-test-source.bin";
    let source_bytes = b"CloudDesk Browser upload acceptance payload 2026 - real bytes.";
    let source_path = home.join(source_name);
    tokio::fs::write(&source_path, source_bytes).await.unwrap();

    let admin_cookie = bootstrap_admin(&base).await;
    enable_browser(&base, &admin_cookie).await;
    let user_cookie = create_user(&base, &admin_cookie, "upflowuser", "user").await;
    let instance_id = open_browser_instance(&base, &user_cookie).await;
    let (mut tx, mut rx) = connect_browser_ws(&base, &user_cookie, &instance_id).await;
    let _ = recv_json_matching(
        &mut rx,
        |v| v["type"] == "connected",
        std::time::Duration::from_secs(10),
    )
    .await;
    navigate(&mut tx, &mut rx, &fixture_url).await;

    click_file_input(&mut tx).await;

    let opened = recv_json_matching(
        &mut rx,
        |v| v["type"] == "file_chooser_opened",
        std::time::Duration::from_secs(15),
    )
    .await;
    let chooser_id = opened.expect("a real file_chooser_opened event must arrive")["chooser_id"]
        .as_str()
        .unwrap()
        .to_owned();

    tx.send(WsMessage::Text(
        json!({"type": "select_file", "chooser_id": chooser_id, "relative_path": source_name})
            .to_string(),
    ))
    .await
    .unwrap();
    let selected = recv_json_matching(
        &mut rx,
        |v| v["type"] == "file_selected" || v["type"] == "error",
        std::time::Duration::from_secs(10),
    )
    .await;
    assert_eq!(
        selected.map(|v| v["type"].clone()),
        Some(json!("file_selected")),
        "selecting the user's own authorized file must succeed"
    );

    let (filename, bytes) = poll_received(&received, std::time::Duration::from_secs(10))
        .await
        .expect("the website must actually receive the selected file's real bytes");
    assert_eq!(filename, source_name);
    assert_eq!(
        bytes, source_bytes,
        "the website must receive exactly the selected file's bytes, unmodified"
    );

    let _ = tokio::fs::remove_file(&source_path).await;
    let _ = tx.close().await;
}

/// Task 12: security matrix for `select_file` -- User B's file must be
/// denied, a stale/unknown `chooser_id` must be denied, and traversal
/// out of the authorized root must be denied, all via the real broker
/// path (not a unit-level shortcut).
#[tokio::test]
async fn task_12_upload_selection_security_matrix() {
    if !docker_and_image_available().await {
        clouddesk_test_support::blocked_by_environment(
            "task_12_upload_selection_security_matrix",
            clouddesk_test_support::reason::CONTAINER_RUNTIME_UNAVAILABLE,
        );
        return;
    }
    let (base, _dir) = application().await;
    let _cross_process_guard = tokio::task::spawn_blocking(acquire_cross_process_browser_lock)
        .await
        .unwrap();
    let _brave_container_guard = BraveContainerGuard::new();

    let (fixture_port, _received) = spawn_chooser_fixture().await;
    let gw = gateway_ip().await;
    clouddeskd::browser_egress_proxy::set_test_allowlist([gw.parse().unwrap()]);
    let fixture_url = format!("http://{gw}:{fixture_port}/");

    let admin_cookie = bootstrap_admin(&base).await;
    enable_browser(&base, &admin_cookie).await;
    let user_cookie = create_user(&base, &admin_cookie, "upsecuser", "user").await;
    let instance_id = open_browser_instance(&base, &user_cookie).await;
    let (mut tx, mut rx) = connect_browser_ws(&base, &user_cookie, &instance_id).await;
    let _ = recv_json_matching(
        &mut rx,
        |v| v["type"] == "connected",
        std::time::Duration::from_secs(10),
    )
    .await;
    navigate(&mut tx, &mut rx, &fixture_url).await;
    click_file_input(&mut tx).await;
    let opened = recv_json_matching(
        &mut rx,
        |v| v["type"] == "file_chooser_opened",
        std::time::Duration::from_secs(15),
    )
    .await
    .expect("a real file_chooser_opened event must arrive");
    let chooser_id = opened["chooser_id"].as_str().unwrap().to_owned();

    // Unknown/stale chooser_id must be denied.
    tx.send(WsMessage::Text(
        json!({"type": "select_file", "chooser_id": "not-a-real-chooser", "relative_path": "x"})
            .to_string(),
    ))
    .await
    .unwrap();
    let stale = recv_json_matching(
        &mut rx,
        |v| v["type"] == "file_selected" || v["type"] == "error",
        std::time::Duration::from_secs(10),
    )
    .await;
    assert_eq!(
        stale.map(|v| v["type"].clone()),
        Some(json!("error")),
        "an unknown chooser_id must be denied"
    );

    // Traversal outside the authorized home root must be denied, even
    // though the real chooser is still pending.
    tx.send(WsMessage::Text(
        json!({"type": "select_file", "chooser_id": chooser_id, "relative_path": "../../../../etc/passwd"})
            .to_string(),
    ))
    .await
    .unwrap();
    let traversal = recv_json_matching(
        &mut rx,
        |v| v["type"] == "file_selected" || v["type"] == "error",
        std::time::Duration::from_secs(10),
    )
    .await;
    assert_eq!(
        traversal.map(|v| v["type"].clone()),
        Some(json!("error")),
        "a traversal attempt outside the authorized root must be denied"
    );

    let _ = tx.close().await;
}
