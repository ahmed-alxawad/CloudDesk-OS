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

/// A second, independently-owned account that can create its OWN
/// vault-credentialed `RemoteServer` through the real product API --
/// this product's role model grants `secrets.manage` only to
/// `administrator` (a pre-existing fact, not something this pass
/// changes), so a plain `user`-role account (like `create_second_user`
/// above) cannot independently register credentials of its own. Used
/// only for the simultaneous-terminal isolation test, where the point
/// is proving per-connection PTY isolation between two distinct
/// owners, not exercising role-based authorization (that is already
/// covered by the cross-user-denial tests using `create_second_user`).
async fn create_second_admin(base: &str, admin_cookie: &str) -> String {
    let create = http(
        base,
        Method::POST,
        "/api/v1/users",
        Some(admin_cookie),
        Some(&json!({
            "username": "usersecondadmin",
            "display_name": "Second Admin",
            "password": "second horse battery staple",
            "role_ids": ["administrator"],
        })),
    )
    .await;
    assert_eq!(create.status(), reqwest::StatusCode::CREATED);
    let cookie = login(base, "usersecondadmin", "second horse battery staple").await;
    step_up(base, &cookie, "second horse battery staple").await;
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
        clouddesk_test_support::blocked_by_environment(
            "task_10_11_open_terminal_product_flow_runs_real_remote_shell",
            clouddesk_test_support::reason::SSH_ACCEPTANCE_FIXTURE_UNAVAILABLE,
        );
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
        clouddesk_test_support::blocked_by_environment(
            "task_21_cross_user_terminal_access_denied",
            clouddesk_test_support::reason::SSH_ACCEPTANCE_FIXTURE_UNAVAILABLE,
        );
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
        clouddesk_test_support::blocked_by_environment(
            "task_30_unauthenticated_terminal_access_denied",
            clouddesk_test_support::reason::SSH_ACCEPTANCE_FIXTURE_UNAVAILABLE,
        );
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
        clouddesk_test_support::blocked_by_environment(
            "task_20_deleted_server_denies_further_terminal_access",
            clouddesk_test_support::reason::SSH_ACCEPTANCE_FIXTURE_UNAVAILABLE,
        );
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
        clouddesk_test_support::blocked_by_environment(
            "task_19_logout_revokes_open_terminal",
            clouddesk_test_support::reason::SSH_ACCEPTANCE_FIXTURE_UNAVAILABLE,
        );
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
        clouddesk_test_support::blocked_by_environment(
            "task_32_33_hostile_ws_input_handled_safely",
            clouddesk_test_support::reason::SSH_ACCEPTANCE_FIXTURE_UNAVAILABLE,
        );
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

/// PASS SSH-C-2, Gap 2 (Task 4): collects binary PTY output for a
/// fixed wall-clock window rather than stopping the instant a
/// predicate matches -- needed here because these tests must also
/// prove the ABSENCE of another terminal's output, not just the
/// presence of this one's.
async fn collect_output_for(
    ws: &mut (impl StreamExt<Item = Result<WsMessage, tokio_tungstenite::tungstenite::Error>> + Unpin),
    window: std::time::Duration,
) -> String {
    let mut buf = Vec::new();
    let deadline = tokio::time::Instant::now() + window;
    while tokio::time::Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        match tokio::time::timeout(remaining, ws.next()).await {
            Ok(Some(Ok(WsMessage::Binary(data)))) => buf.extend_from_slice(&data),
            Ok(Some(Ok(WsMessage::Close(_))) | None) | Err(_) => break,
            Ok(_) => {}
        }
    }
    String::from_utf8_lossy(&buf).into_owned()
}

/// PASS SSH-C-2, Gap 2, Task 4: real simultaneous terminals for two
/// different users, through the actual product/API WebSocket route --
/// live proof (not structural reasoning) that output, resize, and
/// close are each fully isolated per connection.
#[tokio::test(flavor = "multi_thread")]
async fn task_4_simultaneous_user_a_b_terminals_no_crosstalk() {
    if !fixture_available().await {
        clouddesk_test_support::blocked_by_environment(
            "task_4_simultaneous_user_a_b_terminals_no_crosstalk",
            clouddesk_test_support::reason::SSH_ACCEPTANCE_FIXTURE_UNAVAILABLE,
        );
        return;
    }
    let _guard = tokio::task::spawn_blocking(acquire_cross_process_ssh_lock)
        .await
        .unwrap();
    let (base, _dir) = application().await;
    let admin = bootstrap_admin(&base).await;
    let user_b = create_second_admin(&base, &admin).await;
    let server_a = create_password_server(&base, &admin).await;
    let server_b = create_password_server(&base, &user_b).await;

    let mut request_a = ws_url(&base, &server_a).into_client_request().unwrap();
    request_a
        .headers_mut()
        .insert(COOKIE, admin.parse().unwrap());
    let (mut ws_a, _) = tokio_tungstenite::connect_async(request_a).await.unwrap();

    let mut request_b = ws_url(&base, &server_b).into_client_request().unwrap();
    request_b
        .headers_mut()
        .insert(COOKIE, user_b.parse().unwrap());
    let (mut ws_b, _) = tokio_tungstenite::connect_async(request_b).await.unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    // Drain each connection's own shell banner/prompt before the real
    // assertions, so it never gets mistaken for the other side's output.
    let _ = collect_output_for(&mut ws_a, std::time::Duration::from_millis(300)).await;
    let _ = collect_output_for(&mut ws_b, std::time::Duration::from_millis(300)).await;

    ws_a.send(WsMessage::Binary(b"printf 'ONLY-A\\n'\n".to_vec()))
        .await
        .unwrap();
    ws_b.send(WsMessage::Binary(b"printf 'ONLY-B\\n'\n".to_vec()))
        .await
        .unwrap();

    let out_a = collect_output_for(&mut ws_a, std::time::Duration::from_secs(3)).await;
    let out_b = collect_output_for(&mut ws_b, std::time::Duration::from_secs(3)).await;

    assert!(
        out_a.matches("ONLY-A").count() >= 2,
        "A's own terminal must show real ONLY-A output: {out_a:?}"
    );
    assert!(
        !out_a.contains("ONLY-B"),
        "A's terminal must never show B's output: {out_a:?}"
    );
    assert!(
        out_b.matches("ONLY-B").count() >= 2,
        "B's own terminal must show real ONLY-B output: {out_b:?}"
    );
    assert!(
        !out_b.contains("ONLY-A"),
        "B's terminal must never show A's output: {out_b:?}"
    );

    // Interleaved input while both are active -- still isolated.
    ws_a.send(WsMessage::Binary(b"printf 'A-AGAIN\\n'\n".to_vec()))
        .await
        .unwrap();
    ws_b.send(WsMessage::Binary(b"printf 'B-AGAIN\\n'\n".to_vec()))
        .await
        .unwrap();
    ws_a.send(WsMessage::Binary(b"printf 'A-THIRD\\n'\n".to_vec()))
        .await
        .unwrap();
    let repeat_a = collect_output_for(&mut ws_a, std::time::Duration::from_secs(3)).await;
    let repeat_b = collect_output_for(&mut ws_b, std::time::Duration::from_secs(3)).await;
    assert!(repeat_a.contains("A-AGAIN") && repeat_a.contains("A-THIRD"));
    assert!(
        !repeat_a.contains("B-AGAIN"),
        "A must never see B's interleaved input/output"
    );
    assert!(repeat_b.contains("B-AGAIN"));
    assert!(!repeat_b.contains("A-AGAIN") && !repeat_b.contains("A-THIRD"));

    // Resizing A must never alter B's real PTY dimensions.
    ws_a.send(WsMessage::Text(
        json!({"type": "resize", "cols": 120, "rows": 40}).to_string(),
    ))
    .await
    .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    ws_b.send(WsMessage::Binary(b"stty size\n".to_vec()))
        .await
        .unwrap();
    let resize_check = collect_output_for(&mut ws_b, std::time::Duration::from_secs(3)).await;
    assert!(
        resize_check.contains("24 80"),
        "B's PTY must remain at its own original dimensions after A resizes: {resize_check:?}"
    );
    assert!(
        !resize_check.contains("40 120"),
        "A's resize must never leak into B's PTY: {resize_check:?}"
    );

    // Closing A must never affect B.
    ws_a.send(WsMessage::Text(json!({"type": "close"}).to_string()))
        .await
        .unwrap();
    let _ = ws_a.close(None).await;
    ws_b.send(WsMessage::Binary(b"printf 'B-STILL-ALIVE\\n'\n".to_vec()))
        .await
        .unwrap();
    let survives_close = collect_output_for(&mut ws_b, std::time::Duration::from_secs(3)).await;
    assert!(
        survives_close.matches("B-STILL-ALIVE").count() >= 2,
        "closing A's terminal must never close or disrupt B's: {survives_close:?}"
    );
}

/// PASS SSH-C-2, Gap 4 (Task 8): a real `clouddeskd` restart. Unlike
/// this codebase's established in-process two-`axum::serve`-instances
/// restart-simulation convention (`office_restart.rs`, sufficient for
/// DB-persisted state), an in-process simulation cannot honestly prove
/// a live WebSocket/PTY connection dies on restart: axum's WebSocket
/// upgrade hands the raw connection off to a task that outlives the
/// enclosing HTTP serve future, so neither aborting the serve task nor
/// `axum-server`'s `Handle::shutdown()` reaches it (verified live
/// during this pass -- both left the socket open). A real OS process
/// exiting has no such gap: the kernel closes every one of its file
/// descriptors unconditionally. So this test spawns the actual
/// compiled `clouddeskd` binary as a real child process and sends it a
/// real `SIGKILL`.
struct RealClouddeskd {
    child: tokio::process::Child,
    base: String,
}

impl RealClouddeskd {
    async fn spawn(db_path: &std::path::Path, port: u16) -> Self {
        let root = db_path.parent().unwrap();
        let secret_path = root.join("bootstrap.secret");
        if !secret_path.exists() {
            std::fs::write(&secret_path, "remote-terminal-product-test-secret\n").unwrap();
        }
        let master_key_path = root.join("master.key");
        if !master_key_path.exists() {
            std::fs::write(&master_key_path, [151_u8; 32]).unwrap();
        }
        let media_cache = root.join("media-cache");
        std::fs::create_dir_all(&media_cache).unwrap();
        let runtime_state = root.join("runtime-state");
        std::fs::create_dir_all(&runtime_state).unwrap();
        let static_dir =
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../apps/web/dist");
        let static_dir = if static_dir.join("index.html").exists() {
            static_dir
        } else {
            root.to_owned()
        };

        let config_path = root.join("clouddesk.toml");
        std::fs::write(
            &config_path,
            format!(
                r#"
[server]
address = "127.0.0.1"
port = {port}
development_http = true

[security]
master_key = "{master_key}"
bootstrap_secret = "{bootstrap_secret}"

[privilege]
enabled = false

[database]
url = "sqlite://{db}?mode=rwc"
max_connections = 4

[web]
static_dir = "{static_dir}"

[media]
cache_dir = "{media_cache}"

[runtime]
state_dir = "{runtime_state}"
"#,
                master_key = master_key_path.display(),
                bootstrap_secret = secret_path.display(),
                db = db_path.display(),
                static_dir = static_dir.display(),
                media_cache = media_cache.display(),
                runtime_state = runtime_state.display(),
            ),
        )
        .unwrap();

        let child = tokio::process::Command::new(env!("CARGO_BIN_EXE_clouddeskd"))
            .args(["serve", "--config"])
            .arg(&config_path)
            .env("RUST_LOG", "error")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("failed to spawn the real clouddeskd binary");

        let base = format!("http://127.0.0.1:{port}");
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(15);
        loop {
            if reqwest::Client::new()
                .get(format!("{base}/api/v1/setup/status"))
                .send()
                .await
                .is_ok_and(|r| r.status().is_success())
            {
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "the real clouddeskd process must become reachable"
            );
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        }
        Self { child, base }
    }

    /// A real `SIGKILL` (`Child::kill` on Unix) -- no graceful shutdown,
    /// no chance for the process to clean anything up. The kernel alone
    /// is responsible for closing every socket this process held.
    async fn kill(&mut self) {
        let _ = self.child.kill().await;
        let _ = self.child.wait().await;
    }
}

fn unused_tcp_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// Real remote shell PIDs running as `BASTION_USER` on the fixture,
/// via `docker exec ... ps` -- used to prove a PTY's shell process is
/// genuinely gone after the owning `clouddeskd` process is killed, not
/// merely that the client-side WebSocket closed.
async fn remote_shell_pids() -> std::collections::HashSet<String> {
    let output = tokio::process::Command::new("docker")
        .args([
            "exec",
            "acceptance-openssh-1",
            "sh",
            "-c",
            &format!("ps aux | awk -v u={BASTION_USER} '$1==u {{print $2}}'"),
        ])
        .output()
        .await
        .unwrap();
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Task 8: after a real `clouddeskd` restart -- old WS severed, old
/// remote shell process reaped (no orphan), and a fresh PTY opens
/// successfully against the same `RemoteServer` on the new instance.
/// Terminal persistence across a restart is NOT attempted (by design,
/// disclosed): terminals are ephemeral, tied to one process's
/// lifetime, matching the pre-existing local-terminal precedent.
#[tokio::test(flavor = "multi_thread")]
async fn task_8_real_clouddeskd_restart_severs_old_pty_and_allows_a_fresh_one() {
    if !fixture_available().await {
        clouddesk_test_support::blocked_by_environment(
            "task_8_real_clouddeskd_restart_severs_old_pty_and_allows_a_fresh_one",
            clouddesk_test_support::reason::SSH_ACCEPTANCE_FIXTURE_UNAVAILABLE,
        );
        return;
    }
    let _guard = tokio::task::spawn_blocking(acquire_cross_process_ssh_lock)
        .await
        .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("clouddesk.sqlite");

    let mut instance_a = RealClouddeskd::spawn(&db_path, unused_tcp_port()).await;
    let admin = bootstrap_admin(&instance_a.base).await;
    let server_id = create_password_server(&instance_a.base, &admin).await;

    let before = remote_shell_pids().await;
    let mut request = ws_url(&instance_a.base, &server_id)
        .into_client_request()
        .unwrap();
    request.headers_mut().insert(COOKIE, admin.parse().unwrap());
    let (mut ws, _) = tokio_tungstenite::connect_async(request).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    ws.send(WsMessage::Binary(b"whoami && echo $$\n".to_vec()))
        .await
        .unwrap();
    let out = read_until(&mut ws, |s| s.contains("testuser")).await;
    assert!(
        out.contains("testuser"),
        "the real shell must run before the restart: {out:?}"
    );
    let after_open = remote_shell_pids().await;
    let new_pids: Vec<&String> = after_open.difference(&before).collect();
    assert!(
        !new_pids.is_empty(),
        "a real new remote shell process must exist for this PTY: before={before:?} after={after_open:?}"
    );

    // A real process restart: SIGKILL the real clouddeskd child
    // process. No graceful shutdown, no chance to clean anything up --
    // the kernel alone closes every socket this process held.
    instance_a.kill().await;

    // The client-side WebSocket must observe the connection die.
    let client_saw_close = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            match ws.next().await {
                Some(Ok(WsMessage::Close(_)) | Err(_)) | None => return true,
                Some(Ok(_)) => {}
            }
        }
    })
    .await
    .unwrap_or(false);
    assert!(
        client_saw_close,
        "the WebSocket must observe the connection die on a real restart"
    );

    // The old remote shell process must be reaped -- no orphan
    // interactive shell survives the SSH connection dying (the SSH
    // server itself notices the TCP connection died and closes the
    // session, exactly as it would for any other client).
    let mut orphan_gone = false;
    for _ in 0..20 {
        let now = remote_shell_pids().await;
        if new_pids.iter().all(|pid| !now.contains(*pid)) {
            orphan_gone = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    assert!(orphan_gone, "the old PTY's remote shell must be reaped after the owning connection dies, not left as an orphan");

    // A fresh, real, independently-started process, same on-disk state
    // (same DB file), can open a brand new PTY against the same
    // RemoteServer. Terminal persistence across the restart is NOT
    // attempted (by design, disclosed): terminals are ephemeral, tied
    // to one process's lifetime, matching the pre-existing
    // local-terminal precedent -- there is no "old terminal ID" to even
    // attempt reusing, since no attach-by-ID capability exists at all.
    let mut instance_b = RealClouddeskd::spawn(&db_path, unused_tcp_port()).await;
    let admin_b = login(&instance_b.base, "admin", "correct horse battery staple").await;
    let mut request_b = ws_url(&instance_b.base, &server_id)
        .into_client_request()
        .unwrap();
    request_b
        .headers_mut()
        .insert(COOKIE, admin_b.parse().unwrap());
    let (mut ws_b, response_b) = tokio_tungstenite::connect_async(request_b)
        .await
        .expect("a fresh process must be able to open a brand new terminal after a restart");
    assert_eq!(
        response_b.status(),
        axum::http::StatusCode::SWITCHING_PROTOCOLS
    );
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    ws_b.send(WsMessage::Binary(b"whoami\n".to_vec()))
        .await
        .unwrap();
    let out_b = read_until(&mut ws_b, |s| s.contains("testuser")).await;
    assert!(
        out_b.contains("testuser"),
        "a fresh PTY must work on the restarted instance: {out_b:?}"
    );
    instance_b.kill().await;
}
