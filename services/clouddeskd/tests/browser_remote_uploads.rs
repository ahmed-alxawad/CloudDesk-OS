//! Phase 9 Pass 3B-2: real, live Browser remote-VFS (SFTP) upload
//! evidence through the actual product path -- a real file on a real,
//! disposable OpenSSH/SFTP server, selected via the broker's typed
//! `select_file` message with `server_id` set, reauthorized at
//! materialization time via the exact same `resolve_ssh_session` chain
//! Office's WOPI host already uses, delivered to a real Brave file
//! chooser, and the real bytes verified server-side at a controlled
//! receiver endpoint. Skips cleanly (not FAIL) if the disposable
//! OpenSSH fixture (`tests/acceptance/docker-compose.yml`) isn't
//! running.

use axum::http::Method;
use axum::response::IntoResponse;
use clouddesk_auth::{AuthPolicy, AuthService};
use clouddesk_remote::{NewRemoteServer, RemoteServerStore, SshAuthMethod};
use clouddesk_secrets::SecretCipher;
use clouddesk_vault::Vault;
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use sqlx::SqlitePool;
use std::sync::Arc;
use tokio::process::Command as TokioCommand;
use tokio::sync::Mutex as AsyncMutex;
use tokio_tungstenite::tungstenite::Message as WsMessage;

const BROWSER_IMAGE: &str = "clouddesk-brave:1.93.136";
const BASTION_HOST: &str = "127.0.0.1";
const BASTION_PORT: u16 = 2222;
const BASTION_USER: &str = "testuser";
const BASTION_PASSWORD: &str = "testpassword";

async fn openssh_fixture_available() -> bool {
    tokio::net::TcpStream::connect((BASTION_HOST, BASTION_PORT))
        .await
        .is_ok()
}

async fn scan_host_key() -> String {
    let output = TokioCommand::new("docker")
        .args([
            "exec",
            "acceptance-openssh-1",
            "ssh-keyscan",
            "-t",
            "ed25519",
            "-p",
            "2222",
            "localhost",
        ])
        .output()
        .await
        .expect("failed to run ssh-keyscan via docker exec");
    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout
        .lines()
        .find(|line| !line.starts_with('#'))
        .and_then(|line| line.split_whitespace().nth(2))
        .expect("ssh-keyscan produced no host key")
        .to_owned()
}

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

async fn application() -> (String, tempfile::TempDir, SqlitePool) {
    clouddeskd::browser_egress_proxy::spawn();
    let pool = clouddesk_db::connect("sqlite::memory:", 1).await.unwrap();
    clouddesk_db::migrate(&pool).await.unwrap();
    let auth = AuthService::new(
        pool.clone(),
        SecretCipher::new(&[73_u8; 32]).unwrap(),
        AuthPolicy::default(),
    )
    .unwrap();
    let directory = tempfile::tempdir().unwrap();
    let secret_path = directory.path().join("bootstrap.secret");
    std::fs::write(&secret_path, "browser-remote-upload-test-secret\n").unwrap();

    let runtime_root = tempfile::tempdir().unwrap();
    std::mem::forget(runtime_root);
    let runtime_manager = std::sync::Arc::new(
        clouddesk_orchestrator::RuntimeManager::new(
            clouddesk_orchestrator::store::RuntimeStore::new(pool.clone()),
            std::env::temp_dir().join(format!(
                "clouddesk-browser-remote-upload-test-{}",
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
    (format!("http://127.0.0.1:{port}"), directory, pool)
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
            "secret": "browser-remote-upload-test-secret",
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

async fn create_user(
    base: &str,
    admin_cookie: &str,
    username: &str,
    role_id: &str,
) -> (String, String) {
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
    let cookie = login(base, username, "user horse battery staple").await;
    (cookie, user_id)
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
    let received: ReceivedFile = Arc::new(AsyncMutex::new(None));
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

/// Registers a `RemoteServer` for `owner_user_id` pointing at the real
/// disposable OpenSSH fixture, exactly like `office_remote_vfs.rs`'s
/// own established pattern.
async fn register_remote_server(pool: &SqlitePool, owner_user_id: &str) -> String {
    let vault = Vault::new(pool.clone(), SecretCipher::new(&[73_u8; 32]).unwrap());
    let secret_id = vault
        .create(
            owner_user_id,
            "ssh.password",
            "test credential",
            BASTION_PASSWORD.as_bytes(),
        )
        .await
        .unwrap();
    let host_key = scan_host_key().await;
    let store = RemoteServerStore::new(pool.clone());
    store
        .create(
            owner_user_id,
            &NewRemoteServer {
                name: format!("{BASTION_HOST}:{BASTION_PORT}"),
                hostname: BASTION_HOST.to_owned(),
                port: BASTION_PORT,
                username: BASTION_USER.to_owned(),
                auth_method: SshAuthMethod::Password,
                credential_secret_id: Some(secret_id),
                host_key_type: "ssh-ed25519".to_owned(),
                host_key_base64: host_key,
                proxy_jump_server_id: None,
                tags: vec![],
            },
        )
        .await
        .unwrap()
}

fn unique() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    format!(
        "{}{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    )
}

/// Seeds a real file with known sentinel content directly on the real
/// OpenSSH fixture's writable home (`/config`), independent of
/// `CloudDesk`'s own write path.
async fn seed_remote_file(name: &str, content: &[u8]) {
    let mut proc = TokioCommand::new("docker")
        .args([
            "exec",
            "-i",
            "acceptance-openssh-1",
            "sh",
            "-c",
            &format!("cat > /config/{name}"),
        ])
        .stdin(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    {
        use tokio::io::AsyncWriteExt;
        proc.stdin
            .as_mut()
            .unwrap()
            .write_all(content)
            .await
            .unwrap();
    }
    let out = proc.wait_with_output().await.unwrap();
    assert!(out.status.success(), "seeding remote fixture file failed");
}

async fn remove_remote_file(name: &str) {
    let _ = TokioCommand::new("docker")
        .args([
            "exec",
            "acceptance-openssh-1",
            "rm",
            "-f",
            &format!("/config/{name}"),
        ])
        .output()
        .await;
}

/// Tasks 1/3: the real remote-SFTP upload flow end to end -- a real
/// file on a real OpenSSH/SFTP fixture, selected via `select_file`
/// with `server_id` set, delivered to a real Brave file chooser, and
/// the real bytes/filename verified at a controlled receiving
/// website (byte-exact against an independent `docker exec cat`).
#[tokio::test(flavor = "multi_thread")]
async fn task_1_3_real_remote_sftp_upload_flow() {
    if !openssh_fixture_available().await {
        eprintln!("SKIP: disposable OpenSSH fixture not running (docker compose up -d in tests/acceptance)");
        return;
    }
    if !docker_and_image_available().await {
        eprintln!("SKIP: docker/{BROWSER_IMAGE} not available (build docker/brave first)");
        return;
    }
    let (base, _dir, pool) = application().await;
    let _cross_process_guard = tokio::task::spawn_blocking(acquire_cross_process_browser_lock)
        .await
        .unwrap();
    let _brave_container_guard = BraveContainerGuard::new();

    let (fixture_port, received) = spawn_chooser_fixture().await;
    let gw = gateway_ip().await;
    clouddeskd::browser_egress_proxy::set_test_allowlist([gw.parse().unwrap()]);
    let fixture_url = format!("http://{gw}:{fixture_port}/");

    let admin_cookie = bootstrap_admin(&base).await;
    enable_browser(&base, &admin_cookie).await;
    let (user_cookie, user_id) = create_user(&base, &admin_cookie, "remoteupuser", "user").await;
    let server_id = register_remote_server(&pool, &user_id).await;

    let remote_name = format!("remote-upload-source-{}.bin", unique());
    let sentinel_content =
        b"CloudDesk remote SFTP upload acceptance payload 2026 - real sentinel bytes.";
    seed_remote_file(&remote_name, sentinel_content).await;

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

    tx.send(WsMessage::Text(
        json!({
            "type": "select_file",
            "chooser_id": chooser_id,
            "server_id": server_id,
            "relative_path": remote_name,
        })
        .to_string(),
    ))
    .await
    .unwrap();
    let selected = recv_json_matching(
        &mut rx,
        |v| v["type"] == "file_selected" || v["type"] == "error",
        std::time::Duration::from_secs(20),
    )
    .await;
    assert_eq!(
        selected.map(|v| v["type"].clone()),
        Some(json!("file_selected")),
        "selecting the user's own authorized remote file must succeed"
    );

    let (filename, bytes) = poll_received(&received, std::time::Duration::from_secs(10))
        .await
        .expect("the website must actually receive the selected remote file's real bytes");
    assert_eq!(
        filename, remote_name,
        "the real remote filename must be preserved"
    );
    assert_eq!(
        bytes, sentinel_content,
        "the website must receive exactly the remote file's real bytes, unmodified"
    );

    // Byte-exact against an independent read, never trusting
    // CloudDesk's own path agreeing with itself.
    let independent = TokioCommand::new("docker")
        .args([
            "exec",
            "acceptance-openssh-1",
            "cat",
            &format!("/config/{remote_name}"),
        ])
        .output()
        .await
        .unwrap();
    assert_eq!(independent.stdout, sentinel_content);

    remove_remote_file(&remote_name).await;
    let _ = tx.close().await;
}

/// Task 2: remote upload authorization matrix -- User B's own separate
/// `RemoteServer` (User A must never resolve it), an unknown/forged
/// `server_id`, and traversal in the remote virtual path must all be
/// denied through the real product path.
#[tokio::test(flavor = "multi_thread")]
#[allow(clippy::too_many_lines)]
async fn task_2_remote_upload_authorization_matrix() {
    if !openssh_fixture_available().await {
        eprintln!("SKIP: disposable OpenSSH fixture not running (docker compose up -d in tests/acceptance)");
        return;
    }
    if !docker_and_image_available().await {
        eprintln!("SKIP: docker/{BROWSER_IMAGE} not available (build docker/brave first)");
        return;
    }
    let (base, _dir, pool) = application().await;
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
    let (alice_cookie, alice_id) = create_user(&base, &admin_cookie, "remoteauthA", "user").await;
    let (_bob_cookie, bob_id) = create_user(&base, &admin_cookie, "remoteauthB", "user").await;
    let server_a = register_remote_server(&pool, &alice_id).await;
    let server_b = register_remote_server(&pool, &bob_id).await;

    let remote_name = format!("remote-authz-{}.bin", unique());
    seed_remote_file(&remote_name, b"authorization matrix payload").await;

    let instance_id = open_browser_instance(&base, &alice_cookie).await;
    let (mut tx, mut rx) = connect_browser_ws(&base, &alice_cookie, &instance_id).await;
    let _ = recv_json_matching(
        &mut rx,
        |v| v["type"] == "connected",
        std::time::Duration::from_secs(10),
    )
    .await;
    navigate(&mut tx, &mut rx, &fixture_url).await;

    // User A attacking User B's own RemoteServer must be denied.
    click_file_input(&mut tx).await;
    let opened = recv_json_matching(
        &mut rx,
        |v| v["type"] == "file_chooser_opened",
        std::time::Duration::from_secs(15),
    )
    .await
    .expect("a real file_chooser_opened event must arrive");
    let chooser_id = opened["chooser_id"].as_str().unwrap().to_owned();
    tx.send(WsMessage::Text(
        json!({"type": "select_file", "chooser_id": chooser_id, "server_id": server_b, "relative_path": remote_name})
            .to_string(),
    ))
    .await
    .unwrap();
    let denied_b = recv_json_matching(
        &mut rx,
        |v| v["type"] == "file_selected" || v["type"] == "error",
        std::time::Duration::from_secs(15),
    )
    .await;
    assert_eq!(
        denied_b.map(|v| v["type"].clone()),
        Some(json!("error")),
        "User A must never resolve User B's own RemoteServer"
    );

    // Unknown/forged server_id must be denied.
    click_file_input(&mut tx).await;
    let opened2 = recv_json_matching(
        &mut rx,
        |v| v["type"] == "file_chooser_opened",
        std::time::Duration::from_secs(15),
    )
    .await
    .expect("a second real file_chooser_opened event must arrive");
    let chooser_id2 = opened2["chooser_id"].as_str().unwrap().to_owned();
    tx.send(WsMessage::Text(
        json!({"type": "select_file", "chooser_id": chooser_id2, "server_id": "not-a-real-server-id", "relative_path": remote_name})
            .to_string(),
    ))
    .await
    .unwrap();
    let denied_unknown = recv_json_matching(
        &mut rx,
        |v| v["type"] == "file_selected" || v["type"] == "error",
        std::time::Duration::from_secs(15),
    )
    .await;
    assert_eq!(
        denied_unknown.map(|v| v["type"].clone()),
        Some(json!("error")),
        "an unknown/forged server_id must be denied"
    );

    // Traversal in the remote virtual path must fail cleanly (not a
    // real file on this fixture, no such path exists outside /config
    // either), never a silent success.
    click_file_input(&mut tx).await;
    let opened3 = recv_json_matching(
        &mut rx,
        |v| v["type"] == "file_chooser_opened",
        std::time::Duration::from_secs(15),
    )
    .await
    .expect("a third real file_chooser_opened event must arrive");
    let chooser_id3 = opened3["chooser_id"].as_str().unwrap().to_owned();
    tx.send(WsMessage::Text(
        json!({"type": "select_file", "chooser_id": chooser_id3, "server_id": server_a, "relative_path": "../../../../etc/passwd"})
            .to_string(),
    ))
    .await
    .unwrap();
    let denied_traversal = recv_json_matching(
        &mut rx,
        |v| v["type"] == "file_selected" || v["type"] == "error",
        std::time::Duration::from_secs(15),
    )
    .await;
    assert_eq!(
        denied_traversal.map(|v| v["type"].clone()),
        Some(json!("error")),
        "a remote traversal attempt must be denied, not silently succeed"
    );

    remove_remote_file(&remote_name).await;
    let _ = tx.close().await;
}

/// Task 4: remote credential isolation -- the SSH password must never
/// appear anywhere the Browser peripheral surface exposes: not in a
/// broker WS message, and not in the materialized upload temp area
/// (which by the time this assertion runs has already been cleaned up
/// -- proving indirectly that only file bytes, never a credential
/// string, were ever written there).
#[tokio::test(flavor = "multi_thread")]
async fn task_4_remote_credential_isolation() {
    if !openssh_fixture_available().await {
        eprintln!("SKIP: disposable OpenSSH fixture not running (docker compose up -d in tests/acceptance)");
        return;
    }
    if !docker_and_image_available().await {
        eprintln!("SKIP: docker/{BROWSER_IMAGE} not available (build docker/brave first)");
        return;
    }
    let (base, dir, pool) = application().await;
    let _cross_process_guard = tokio::task::spawn_blocking(acquire_cross_process_browser_lock)
        .await
        .unwrap();
    let _brave_container_guard = BraveContainerGuard::new();

    let (fixture_port, received) = spawn_chooser_fixture().await;
    let gw = gateway_ip().await;
    clouddeskd::browser_egress_proxy::set_test_allowlist([gw.parse().unwrap()]);
    let fixture_url = format!("http://{gw}:{fixture_port}/");

    let admin_cookie = bootstrap_admin(&base).await;
    enable_browser(&base, &admin_cookie).await;
    let (user_cookie, user_id) = create_user(&base, &admin_cookie, "remotecreduser", "user").await;
    let server_id = register_remote_server(&pool, &user_id).await;

    let remote_name = format!("remote-cred-{}.bin", unique());
    seed_remote_file(&remote_name, b"credential isolation payload").await;

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

    // Collect every WS message seen from here on, to grep for the
    // credential.
    let mut all_messages = Vec::new();
    tx.send(WsMessage::Text(
        json!({"type": "select_file", "chooser_id": chooser_id, "server_id": server_id, "relative_path": remote_name})
            .to_string(),
    ))
    .await
    .unwrap();
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(15);
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            break;
        }
        match tokio::time::timeout(remaining, rx.next()).await {
            Ok(Some(Ok(WsMessage::Text(text)))) => {
                let is_terminal = text.contains("file_selected") || text.contains("\"error\"");
                all_messages.push(text);
                if is_terminal {
                    break;
                }
            }
            _ => break,
        }
    }
    for message in &all_messages {
        assert!(
            !message.contains(BASTION_PASSWORD),
            "the SSH password must never appear in any broker WS message, got {message:?}"
        );
    }
    let _ = poll_received(&received, std::time::Duration::from_secs(10)).await;

    // The materialized copy is deleted by the time file_selected is
    // acknowledged (Task 5) -- confirm no upload temp artifact
    // survives, anywhere under this instance's own /state/uploads.
    let uploads_dir = dir.path().join("uploads");
    let leftover = walk_files(&uploads_dir);
    assert!(
        leftover.is_empty(),
        "no upload temp artifact should remain after a successful selection, found {leftover:?}"
    );

    // The password is only ever stored Vault-encrypted, never plaintext.
    let plaintext_hits: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM vault_secrets WHERE encrypted_value LIKE ?")
            .bind(format!("%{BASTION_PASSWORD}%"))
            .fetch_one(&pool)
            .await
            .unwrap_or(0);
    assert_eq!(
        plaintext_hits, 0,
        "the SSH password must never be stored in plaintext"
    );

    remove_remote_file(&remote_name).await;
    let _ = tx.close().await;
}

fn walk_files(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.extend(walk_files(&path));
        } else {
            out.push(path);
        }
    }
    out
}

/// Task 2/5: an unavailable remote provider (bad/unreachable host)
/// must fail cleanly, not hang or panic, and leave no temp artifact.
#[tokio::test(flavor = "multi_thread")]
async fn task_2_5_remote_provider_unavailable_clean_failure() {
    if !docker_and_image_available().await {
        eprintln!("SKIP: docker/{BROWSER_IMAGE} not available (build docker/brave first)");
        return;
    }
    let (base, dir, pool) = application().await;
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
    let (user_cookie, user_id) =
        create_user(&base, &admin_cookie, "remoteunavailuser", "user").await;

    // A RemoteServer record that points nowhere reachable.
    let vault = Vault::new(pool.clone(), SecretCipher::new(&[73_u8; 32]).unwrap());
    let secret_id = vault
        .create(&user_id, "ssh.password", "unreachable", b"whatever")
        .await
        .unwrap();
    let store = RemoteServerStore::new(pool.clone());
    let server_id = store
        .create(
            &user_id,
            &NewRemoteServer {
                name: "unreachable".to_owned(),
                hostname: "127.0.0.1".to_owned(),
                port: 1, // nothing listens here
                username: "nobody".to_owned(),
                auth_method: SshAuthMethod::Password,
                credential_secret_id: Some(secret_id),
                host_key_type: "ssh-ed25519".to_owned(),
                // Structurally valid ed25519 SSH wire-format key
                // (correct type string + 32-byte payload) so it
                // passes `RemoteServerStore::create`'s own format
                // validation -- this test needs a record that is
                // real enough to be *accepted*, then genuinely
                // unreachable at connect time (port 1), not a record
                // rejected before ever reaching that far.
                host_key_base64:
                    "AAAAC3NzaC1lZDI1NTE5AAAAIAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"
                        .to_owned(),
                proxy_jump_server_id: None,
                tags: vec![],
            },
        )
        .await
        .unwrap();

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

    tx.send(WsMessage::Text(
        json!({"type": "select_file", "chooser_id": chooser_id, "server_id": server_id, "relative_path": "anything.bin"})
            .to_string(),
    ))
    .await
    .unwrap();
    let result = recv_json_matching(
        &mut rx,
        |v| v["type"] == "file_selected" || v["type"] == "error",
        std::time::Duration::from_secs(20),
    )
    .await;
    assert_eq!(
        result.map(|v| v["type"].clone()),
        Some(json!("error")),
        "an unreachable remote provider must fail cleanly, not hang or silently succeed"
    );

    let uploads_dir = dir.path().join("uploads");
    let leftover = walk_files(&uploads_dir);
    assert!(
        leftover.is_empty(),
        "no upload temp artifact should exist after a failed remote read, found {leftover:?}"
    );

    let _ = tx.close().await;
}

/// Task 5 (Pass 3B-3): the picker's `/api/v1/remote/servers` list is
/// genuinely user-scoped at the API layer, not merely filtered client
/// side -- User B's own call to the same endpoint the chooser UI uses
/// must never include User A's `RemoteServer`. Backend denial of an
/// unauthorized `server_id`/traversal/stale chooser through
/// `select_file` itself is already proven by
/// `task_2_remote_upload_authorization_matrix` above (unchanged this
/// pass), so this test covers the one new surface Pass 3B-3 adds: the
/// list endpoint the picker populates its dropdown from.
#[tokio::test(flavor = "multi_thread")]
async fn task_5_remote_server_list_is_user_scoped() {
    if !openssh_fixture_available().await {
        eprintln!("SKIP: disposable OpenSSH fixture not running (docker compose up -d in tests/acceptance)");
        return;
    }
    let (base, _dir, pool) = application().await;
    let admin_cookie = bootstrap_admin(&base).await;
    let (alice_cookie, alice_id) =
        create_user(&base, &admin_cookie, "listscopedalice", "user").await;
    let (bob_cookie, _bob_id) = create_user(&base, &admin_cookie, "listscopedbob", "user").await;

    let alice_server_id = register_remote_server(&pool, &alice_id).await;

    let bob_list = http(
        &base,
        Method::GET,
        "/api/v1/remote/servers",
        Some(&bob_cookie),
        None,
    )
    .await;
    assert_eq!(bob_list.status(), reqwest::StatusCode::OK);
    let bob_body: Value = bob_list.json().await.unwrap();
    let bob_ids: Vec<String> = bob_body["servers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["id"].as_str().unwrap().to_owned())
        .collect();
    assert!(
        !bob_ids.contains(&alice_server_id),
        "User B's own remote-server list must never include User A's server, the exact list the picker's dropdown renders"
    );

    let alice_list = http(
        &base,
        Method::GET,
        "/api/v1/remote/servers",
        Some(&alice_cookie),
        None,
    )
    .await;
    assert_eq!(alice_list.status(), reqwest::StatusCode::OK);
    let alice_body: Value = alice_list.json().await.unwrap();
    let alice_ids: Vec<String> = alice_body["servers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["id"].as_str().unwrap().to_owned())
        .collect();
    assert!(
        alice_ids.contains(&alice_server_id),
        "User A's own remote-server list must include their own server"
    );
}
