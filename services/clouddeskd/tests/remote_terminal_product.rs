//! PASS SSH-C: product/API evidence for the remote SSH PTY terminal --
//! exercised exclusively through the real HTTP+WebSocket surface
//! (`POST /api/v1/vault/secrets`, `POST /api/v1/remote/servers`,
//! `GET /api/v1/remote/servers/{id}/terminal/ws`), never a direct
//! `SshSession`/`TerminalSession` Rust call (that live-protocol
//! evidence already exists in `crates/remote/tests/pty.rs`). A PASS
//! here proves the product route wires authentication, `RemoteServer`
//! ownership, and the real PTY together correctly.
//!
//! Skips (not FAIL) if the disposable OpenSSH fixture
//! (`tests/acceptance/docker-compose.yml`) isn't running.

use axum::http::Method;
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::header::COOKIE;
use tokio_tungstenite::tungstenite::Message as WsMessage;

const BASTION_HOST: &str = "127.0.0.1";
const BASTION_PORT: u16 = 2222;
const BASTION_USER: &str = "testuser";
const BASTION_PASSWORD: &str = "testpassword";

async fn fixture_available() -> bool {
    tokio::net::TcpStream::connect((BASTION_HOST, BASTION_PORT))
        .await
        .is_ok()
}

async fn scan_host_key_for(host: &str) -> String {
    let output = tokio::process::Command::new("docker")
        .args([
            "exec",
            "acceptance-openssh-1",
            "ssh-keyscan",
            "-t",
            "ed25519",
            "-p",
            "2222",
            host,
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

async fn application() -> (String, tempfile::TempDir) {
    let pool = clouddesk_db::connect("sqlite::memory:", 1).await.unwrap();
    clouddesk_db::migrate(&pool).await.unwrap();
    let auth = clouddesk_auth::AuthService::new(
        pool,
        clouddesk_secrets::SecretCipher::new(&[73_u8; 32]).unwrap(),
        clouddesk_auth::AuthPolicy::default(),
    )
    .unwrap();
    let directory = tempfile::tempdir().unwrap();
    let secret_path = directory.path().join("bootstrap.secret");
    std::fs::write(&secret_path, "remote-terminal-product-test-secret\n").unwrap();
    let router = clouddeskd::application_router(directory.path().to_owned(), auth, secret_path);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(
            listener,
            router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await
        .unwrap();
    });
    (format!("http://{addr}"), directory)
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

async fn step_up(base: &str, cookie: &str, password: &str) {
    let response = http(
        base,
        Method::POST,
        "/api/v1/auth/step-up",
        Some(cookie),
        Some(&json!({"password": password})),
    )
    .await;
    assert_eq!(response.status(), reqwest::StatusCode::OK);
}

async fn bootstrap_admin(base: &str) -> String {
    let response = http(
        base,
        Method::POST,
        "/api/v1/setup/bootstrap",
        None,
        Some(&json!({
            "secret": "remote-terminal-product-test-secret",
            "username": "admin",
            "display_name": "Admin",
            "password": "correct horse battery staple",
            "linux_username": Value::Null,
        })),
    )
    .await;
    assert_eq!(response.status(), reqwest::StatusCode::CREATED);
    let cookie = login(base, "admin", "correct horse battery staple").await;
    step_up(base, &cookie, "correct horse battery staple").await;
    cookie
}

async fn create_second_user(base: &str, admin_cookie: &str) -> String {
    let create = http(
        base,
        Method::POST,
        "/api/v1/users",
        Some(admin_cookie),
        Some(&json!({
            "username": "userb",
            "display_name": "User B",
            "password": "user horse battery staple",
            "role_ids": ["user"],
        })),
    )
    .await;
    assert_eq!(create.status(), reqwest::StatusCode::CREATED);
    let cookie = login(base, "userb", "user horse battery staple").await;
    step_up(base, &cookie, "user horse battery staple").await;
    cookie
}

async fn create_secret(base: &str, cookie: &str, kind: &str, value: &str) -> String {
    let response = http(
        base,
        Method::POST,
        "/api/v1/vault/secrets",
        Some(cookie),
        Some(&json!({"kind": kind, "label": "test", "value": value})),
    )
    .await;
    assert_eq!(response.status(), reqwest::StatusCode::CREATED);
    let body: Value = response.json().await.unwrap();
    body["secret_id"].as_str().unwrap().to_owned()
}

fn rand_suffix() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    (u64::from(std::process::id()) << 20) + COUNTER.fetch_add(1, Ordering::Relaxed)
}

async fn create_password_server(base: &str, cookie: &str) -> String {
    let secret_id = create_secret(base, cookie, "ssh.password", BASTION_PASSWORD).await;
    let host_key = scan_host_key_for("localhost").await;
    let created = http(
        base,
        Method::POST,
        "/api/v1/remote/servers",
        Some(cookie),
        Some(&json!({
            "name": format!("terminal-test-{}", rand_suffix()),
            "hostname": BASTION_HOST,
            "port": BASTION_PORT,
            "username": BASTION_USER,
            "auth_method": "password",
            "credential_secret_id": secret_id,
            "agent_socket_path": Value::Null,
            "host_key_type": "ssh-ed25519",
            "host_key_base64": host_key,
            "proxy_jump_server_id": Value::Null,
            "tags": [],
        })),
    )
    .await;
    assert_eq!(created.status(), reqwest::StatusCode::CREATED);
    created.json::<Value>().await.unwrap()["server_id"]
        .as_str()
        .unwrap()
        .to_owned()
}

fn ws_url(base: &str, server_id: &str) -> String {
    format!(
        "ws://{}/api/v1/remote/servers/{server_id}/terminal/ws",
        base.trim_start_matches("http://")
    )
}

fn acquire_cross_process_ssh_lock() -> std::fs::File {
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

/// Reads binary/text frames from `ws` until `predicate` matches the
/// accumulated binary output, or a bounded number of frames pass
/// without a match.
async fn read_until(
    ws: &mut (impl StreamExt<Item = Result<WsMessage, tokio_tungstenite::tungstenite::Error>> + Unpin),
    predicate: impl Fn(&str) -> bool,
) -> String {
    let mut buf = Vec::new();
    for _ in 0..200 {
        match tokio::time::timeout(std::time::Duration::from_secs(8), ws.next()).await {
            Ok(Some(Ok(WsMessage::Binary(data)))) => {
                buf.extend_from_slice(&data);
                if predicate(&String::from_utf8_lossy(&buf)) {
                    break;
                }
            }
            Ok(Some(Ok(WsMessage::Text(_) | WsMessage::Close(_)) | Err(_)) | None) | Err(_) => {
                break
            }
            Ok(Some(Ok(_))) => {}
        }
    }
    String::from_utf8_lossy(&buf).into_owned()
}

/// Task 10/11: the "Open Terminal" product flow end-to-end -- a real
/// authenticated WebSocket, a real PTY, a real remote shell (`whoami`
/// through the actual OpenSSH bastion), never a mock.
#[tokio::test(flavor = "multi_thread")]
async fn task_10_11_open_terminal_product_flow_runs_real_remote_shell() {
    if !fixture_available().await {
        eprintln!("SKIP: disposable OpenSSH fixture not running (docker compose up -d in tests/acceptance)");
        return;
    }
    let _guard = tokio::task::spawn_blocking(acquire_cross_process_ssh_lock)
        .await
        .unwrap();
    let (base, _dir) = application().await;
    let admin = bootstrap_admin(&base).await;
    let server_id = create_password_server(&base, &admin).await;

    let mut request = ws_url(&base, &server_id).into_client_request().unwrap();
    request.headers_mut().insert(COOKIE, admin.parse().unwrap());
    let (mut ws, response) = tokio_tungstenite::connect_async(request)
        .await
        .expect("owner must be able to open a real remote terminal");
    assert_eq!(
        response.status(),
        axum::http::StatusCode::SWITCHING_PROTOCOLS
    );

    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    ws.send(WsMessage::Binary(b"whoami\n".to_vec()))
        .await
        .unwrap();
    let out = read_until(&mut ws, |s| s.contains("testuser")).await;
    assert!(
        out.contains("testuser"),
        "real remote whoami output expected: {out:?}"
    );

    // Task 12: resize through the real product WS control channel.
    ws.send(WsMessage::Text(
        json!({"type": "resize", "cols": 100, "rows": 30}).to_string(),
    ))
    .await
    .unwrap();
    ws.send(WsMessage::Binary(b"stty size\n".to_vec()))
        .await
        .unwrap();
    let out = read_until(&mut ws, |s| s.contains("30 100")).await;
    assert!(
        out.contains("30 100"),
        "resize must change the real PTY: {out:?}"
    );

    ws.send(WsMessage::Binary(b"exit\n".to_vec()))
        .await
        .unwrap();
    let _ = tokio::time::timeout(std::time::Duration::from_secs(5), ws.next()).await;
}

/// Task 21: cross-user denial -- a user who does not own the
/// `RemoteServer` must never be able to open a terminal on it. The
/// authorization/ownership check runs before the WS upgrade, so the
/// HTTP handshake itself is rejected (never a 101 that gets closed
/// afterward).
#[tokio::test(flavor = "multi_thread")]
async fn task_21_cross_user_terminal_access_denied() {
    if !fixture_available().await {
        eprintln!("SKIP: disposable OpenSSH fixture not running (docker compose up -d in tests/acceptance)");
        return;
    }
    let _guard = tokio::task::spawn_blocking(acquire_cross_process_ssh_lock)
        .await
        .unwrap();
    let (base, _dir) = application().await;
    let admin = bootstrap_admin(&base).await;
    let user_b = create_second_user(&base, &admin).await;
    let server_id = create_password_server(&base, &admin).await;

    let mut request = ws_url(&base, &server_id).into_client_request().unwrap();
    request
        .headers_mut()
        .insert(COOKIE, user_b.parse().unwrap());
    let outcome = tokio_tungstenite::connect_async(request).await;
    assert!(
        outcome.is_err(),
        "a non-owner must never be able to open a WebSocket onto another user's terminal"
    );

    // A random/stale server ID is denied the exact same way, never
    // distinguished from "exists but not yours" (Task 21).
    let mut stale_request = ws_url(&base, "00000000-0000-0000-0000-000000000000")
        .into_client_request()
        .unwrap();
    stale_request
        .headers_mut()
        .insert(COOKIE, admin.parse().unwrap());
    let stale_outcome = tokio_tungstenite::connect_async(stale_request).await;
    assert!(
        stale_outcome.is_err(),
        "a stale/nonexistent server id must be denied"
    );
}

/// Task 30/31: an unauthenticated request (no session cookie at all)
/// must never reach the PTY.
#[tokio::test(flavor = "multi_thread")]
async fn task_30_unauthenticated_terminal_access_denied() {
    if !fixture_available().await {
        eprintln!("SKIP: disposable OpenSSH fixture not running (docker compose up -d in tests/acceptance)");
        return;
    }
    let _guard = tokio::task::spawn_blocking(acquire_cross_process_ssh_lock)
        .await
        .unwrap();
    let (base, _dir) = application().await;
    let admin = bootstrap_admin(&base).await;
    let server_id = create_password_server(&base, &admin).await;

    let request = ws_url(&base, &server_id).into_client_request().unwrap();
    let outcome = tokio_tungstenite::connect_async(request).await;
    assert!(outcome.is_err(), "no cookie at all must be denied");
}

/// Task 20: deleting the `RemoteServer` must prevent any further
/// terminal use of it, through the real product/API path.
#[tokio::test(flavor = "multi_thread")]
async fn task_20_deleted_server_denies_further_terminal_access() {
    if !fixture_available().await {
        eprintln!("SKIP: disposable OpenSSH fixture not running (docker compose up -d in tests/acceptance)");
        return;
    }
    let _guard = tokio::task::spawn_blocking(acquire_cross_process_ssh_lock)
        .await
        .unwrap();
    let (base, _dir) = application().await;
    let admin = bootstrap_admin(&base).await;
    let server_id = create_password_server(&base, &admin).await;

    let delete = http(
        &base,
        Method::DELETE,
        &format!("/api/v1/remote/servers/{server_id}"),
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(delete.status(), reqwest::StatusCode::NO_CONTENT);

    let mut request = ws_url(&base, &server_id).into_client_request().unwrap();
    request.headers_mut().insert(COOKIE, admin.parse().unwrap());
    let outcome = tokio_tungstenite::connect_async(request).await;
    assert!(
        outcome.is_err(),
        "a deleted RemoteServer must never allow a new terminal to open"
    );
}

/// Task 19: logging out must revoke an already-open terminal promptly
/// (through the real periodic re-validation loop), not merely block
/// new connections.
#[tokio::test(flavor = "multi_thread")]
async fn task_19_logout_revokes_open_terminal() {
    if !fixture_available().await {
        eprintln!("SKIP: disposable OpenSSH fixture not running (docker compose up -d in tests/acceptance)");
        return;
    }
    let _guard = tokio::task::spawn_blocking(acquire_cross_process_ssh_lock)
        .await
        .unwrap();
    let (base, _dir) = application().await;
    let admin = bootstrap_admin(&base).await;
    let server_id = create_password_server(&base, &admin).await;

    let mut request = ws_url(&base, &server_id).into_client_request().unwrap();
    request.headers_mut().insert(COOKIE, admin.parse().unwrap());
    let (mut ws, _) = tokio_tungstenite::connect_async(request).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    let logout = http(
        &base,
        Method::POST,
        "/api/v1/auth/logout",
        Some(&admin),
        Some(&json!({})),
    )
    .await;
    assert!(logout.status().is_success());

    // The periodic re-validation loop runs every 5s in production code;
    // give it up to three cycles' worth of margin.
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(20);
    let mut revoked = false;
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(std::time::Duration::from_secs(3), ws.next()).await {
            Ok(Some(Ok(WsMessage::Text(text)))) if text.contains("revoked") => {
                revoked = true;
                break;
            }
            Ok(Some(Ok(WsMessage::Close(_))) | None) => break,
            Ok(_) | Err(_) => {}
        }
    }
    assert!(
        revoked,
        "logging out must revoke the still-open terminal, not leave it usable"
    );
}

/// Task 32/33: malformed WebSocket input and resize abuse are handled
/// safely -- ignored, never a panic, never corrupting the still-usable
/// terminal.
#[tokio::test(flavor = "multi_thread")]
async fn task_32_33_hostile_ws_input_handled_safely() {
    if !fixture_available().await {
        eprintln!("SKIP: disposable OpenSSH fixture not running (docker compose up -d in tests/acceptance)");
        return;
    }
    let _guard = tokio::task::spawn_blocking(acquire_cross_process_ssh_lock)
        .await
        .unwrap();
    let (base, _dir) = application().await;
    let admin = bootstrap_admin(&base).await;
    let server_id = create_password_server(&base, &admin).await;

    let mut request = ws_url(&base, &server_id).into_client_request().unwrap();
    request.headers_mut().insert(COOKIE, admin.parse().unwrap());
    let (mut ws, _) = tokio_tungstenite::connect_async(request).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    // Malformed JSON control message.
    ws.send(WsMessage::Text("{ not json at all".into()))
        .await
        .unwrap();
    // An absurd resize request -- must be rejected, never passed through
    // to the SSH library unchecked.
    ws.send(WsMessage::Text(
        json!({"type": "resize", "cols": 999_999, "rows": 0}).to_string(),
    ))
    .await
    .unwrap();

    // The terminal must still be fully usable afterward.
    ws.send(WsMessage::Binary(b"whoami\n".to_vec()))
        .await
        .unwrap();
    let out = read_until(&mut ws, |s| s.contains("testuser")).await;
    assert!(
        out.contains("testuser"),
        "the terminal must remain usable after hostile input: {out:?}"
    );
}
