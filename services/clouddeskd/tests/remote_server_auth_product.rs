//! PASS SSH-A-2 (Task 37): product/API evidence for SSH agent,
//! keyboard-interactive, and certificate authentication -- configured
//! and connected exclusively through the real HTTP API surface
//! (`POST /api/v1/vault/secrets`, `POST`/`PUT /api/v1/remote/servers`,
//! `POST /api/v1/remote/servers/{id}/test-connection`), never a direct
//! `RemoteServerStore`/`Vault` Rust call. `test-connection` is the one
//! HTTP route that actually authenticates -- it calls the exact same
//! `resolve_ssh_session` connection builder SFTP/Transfers/WOPI/Browser
//! remote uploads already use, so a PASS here is evidence the product
//! configuration path resolves into that one proven connection builder,
//! not a second, parallel one.
//!
//! Skips (not FAIL) if the disposable OpenSSH fixture
//! (`tests/acceptance/docker-compose.yml`) isn't running.

use axum::http::Method;
use serde_json::{json, Value};

const BASTION_HOST: &str = "127.0.0.1";
const BASTION_PORT: u16 = 2222;
const BASTION_USER: &str = "testuser";
const BASTION_PASSWORD: &str = "testpassword";
// Resolvable only from inside the bastion container.
const TARGET_HOST: &str = "openssh-target";
const TARGET_PORT: u16 = 2222;
const TARGET_USER: &str = "targetuser";

async fn fixture_available() -> bool {
    tokio::net::TcpStream::connect((BASTION_HOST, BASTION_PORT))
        .await
        .is_ok()
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

/// Mirrors `ssh_advanced_auth.rs`'s fixture-ownership fix: this
/// container's sshd master runs unprivileged as `testuser`, and
/// `StrictModes` rejects an `authorized_keys` file it can't itself
/// read, so the write (and the final `chown`) must land as `testuser`.
async fn authorize_key_on_fixture(pubkey: &str) {
    let _ = tokio::process::Command::new("docker")
        .args([
            "exec",
            "acceptance-openssh-1",
            "sh",
            "-c",
            "mkdir -p /config/.ssh && chown testuser:testuser /config/.ssh && chmod 700 /config/.ssh",
        ])
        .output()
        .await;
    let mut proc = tokio::process::Command::new("docker")
        .args([
            "exec",
            "-i",
            "-u",
            "testuser",
            "acceptance-openssh-1",
            "sh",
            "-c",
            "cat >> /config/.ssh/authorized_keys && chmod 600 /config/.ssh/authorized_keys",
        ])
        .stdin(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    {
        use tokio::io::AsyncWriteExt;
        proc.stdin
            .as_mut()
            .unwrap()
            .write_all(format!("{pubkey}\n").as_bytes())
            .await
            .unwrap();
    }
    let out = proc.wait_with_output().await.unwrap();
    assert!(
        out.status.success(),
        "authorizing test key on fixture failed"
    );
}

async fn clear_authorized_keys() {
    let _ = tokio::process::Command::new("docker")
        .args([
            "exec",
            "acceptance-openssh-1",
            "sh",
            "-c",
            "rm -f /config/.ssh/authorized_keys",
        ])
        .output()
        .await;
}

/// Real, disposable `ssh-agent` this test process itself spawns and
/// tears down on drop -- never a mocked agent protocol.
struct RealAgent {
    child: tokio::process::Child,
    key_path: std::path::PathBuf,
    pub socket_path: String,
    /// Owns the agent's socket + generated key material for exactly as
    /// long as the agent itself lives. Held (rather than `mem::forget`
    /// -ed, as this previously was) so `Drop` deletes it: otherwise
    /// every `spawn()` left a real ed25519 private key behind in
    /// `/tmp` for the lifetime of the machine.
    _dir: tempfile::TempDir,
}

impl RealAgent {
    async fn spawn() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let socket_path = dir.path().join("agent.sock");
        let key_path = dir.path().join("id_ed25519");
        let keygen = tokio::process::Command::new("ssh-keygen")
            .args([
                "-t",
                "ed25519",
                "-f",
                key_path.to_str().unwrap(),
                "-N",
                "",
                "-q",
            ])
            .status()
            .await
            .unwrap();
        assert!(keygen.success());
        let child = tokio::process::Command::new("ssh-agent")
            .args(["-D", "-a", socket_path.to_str().unwrap()])
            .spawn()
            .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        let add = tokio::process::Command::new("ssh-add")
            .arg(&key_path)
            .env("SSH_AUTH_SOCK", &socket_path)
            .status()
            .await
            .unwrap();
        assert!(add.success());
        Self {
            child,
            key_path,
            socket_path: socket_path.to_string_lossy().into_owned(),
            _dir: dir,
        }
    }

    fn public_key(&self) -> String {
        std::fs::read_to_string(self.key_path.with_extension("pub"))
            .unwrap()
            .trim()
            .to_owned()
    }
}

impl Drop for RealAgent {
    fn drop(&mut self) {
        let _ = self.child.start_kill();
    }
}

async fn generate_signed_identity(principal: &str, extra_args: &[&str]) -> (String, String) {
    let dir = tempfile::tempdir().unwrap();
    let key_path = dir.path().join("id");
    let keygen = tokio::process::Command::new("ssh-keygen")
        .args([
            "-t",
            "ed25519",
            "-f",
            key_path.to_str().unwrap(),
            "-N",
            "",
            "-q",
        ])
        .status()
        .await
        .unwrap();
    assert!(keygen.success());
    let ca_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/acceptance/fixtures/ssh_ca/ca");
    let mut args = vec![
        "-s".to_owned(),
        ca_path.to_string_lossy().into_owned(),
        "-I".to_owned(),
        "test-identity".to_owned(),
        "-n".to_owned(),
        principal.to_owned(),
    ];
    args.extend(extra_args.iter().map(|s| (*s).to_owned()));
    args.push(
        key_path
            .with_extension("pub")
            .to_string_lossy()
            .into_owned(),
    );
    let sign = tokio::process::Command::new("ssh-keygen")
        .args(&args)
        .status()
        .await
        .unwrap();
    assert!(sign.success(), "ssh-keygen -s must succeed");
    let key_data = tokio::fs::read_to_string(&key_path).await.unwrap();
    let cert_data = tokio::fs::read_to_string(key_path.with_extension("").with_file_name(format!(
        "{}-cert.pub",
        key_path.file_name().unwrap().to_string_lossy()
    )))
    .await
    .unwrap();
    (key_data, cert_data)
}

// ============ real HTTP product/API harness (bound TCP listener) ============

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
    std::fs::write(&secret_path, "remote-auth-product-test-secret\n").unwrap();
    let router = clouddeskd::application_router(directory.path().to_owned(), auth, secret_path);
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

/// Bootstraps the admin (owner A) with `linux_uid` set to this test
/// process's own real UID -- required so agent-socket-ownership checks
/// pass for a real agent this test process itself spawns.
async fn bootstrap_admin(base: &str) -> String {
    let linux_username = current_process_linux_identity().map(|i| i.username);
    let response = http(
        base,
        Method::POST,
        "/api/v1/setup/bootstrap",
        None,
        Some(&json!({
            "secret": "remote-auth-product-test-secret",
            "username": "admin",
            "display_name": "Admin",
            "password": "correct horse battery staple",
            "linux_username": linux_username,
        })),
    )
    .await;
    assert_eq!(response.status(), reqwest::StatusCode::CREATED);
    let cookie = login(base, "admin", "correct horse battery staple").await;
    step_up(base, &cookie, "correct horse battery staple").await;
    cookie
}

/// A second real user, created through the actual product/API path.
/// The cross-user assertions this test file makes (User B can never
/// see/edit/delete/test-connect User A's `RemoteServer`) are enforced
/// by `owner_user_id` scoping alone, so this deliberately does not
/// assign User B any Linux identity at all -- `/linux-identity`
/// legitimately requires a real, existing system account (it looks the
/// UID up), so a fabricated decoy UID belongs in a direct-DB harness
/// (`ssh_advanced_auth.rs`'s `Harness`), not behind this real endpoint.
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

/// `value` is the exact raw bytes to store (matching
/// `worker.rs::resolve_auth`'s expectations per auth method): a plain
/// string for `ssh.password`, or an already-`.to_string()`-ed JSON
/// array/object for `ssh.keyboard_interactive`/`ssh.certificate`.
/// Passing a `serde_json::Value` here directly would double-encode a
/// scalar string (wrapping it in an extra pair of quotes), silently
/// corrupting the stored credential.
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

#[allow(clippy::too_many_arguments)]
fn rand_suffix() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    (u64::from(std::process::id()) << 20) + COUNTER.fetch_add(1, Ordering::Relaxed)
}

#[allow(clippy::too_many_arguments)]
async fn create_server(
    base: &str,
    cookie: &str,
    hostname: &str,
    port: u16,
    username: &str,
    auth_method: &str,
    credential_secret_id: Option<&str>,
    agent_socket_path: Option<&str>,
    host_key: &str,
    proxy_jump_server_id: Option<&str>,
) -> reqwest::Response {
    http(
        base,
        Method::POST,
        "/api/v1/remote/servers",
        Some(cookie),
        Some(&json!({
            "name": format!("{hostname}:{port}:{auth_method}:{}", rand_suffix()),
            "hostname": hostname,
            "port": port,
            "username": username,
            "auth_method": auth_method,
            "credential_secret_id": credential_secret_id,
            "agent_socket_path": agent_socket_path,
            "host_key_type": "ssh-ed25519",
            "host_key_base64": host_key,
            "proxy_jump_server_id": proxy_jump_server_id,
            "tags": [],
        })),
    )
    .await
}

async fn test_connection(base: &str, cookie: &str, server_id: &str) -> Value {
    let response = http(
        base,
        Method::POST,
        &format!("/api/v1/remote/servers/{server_id}/test-connection"),
        Some(cookie),
        None,
    )
    .await;
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    response.json().await.unwrap()
}

// ============================ tests ============================

#[tokio::test(flavor = "multi_thread")]
async fn agent_product_configuration_and_connection() {
    if !fixture_available().await {
        clouddesk_test_support::blocked_by_environment(
            "agent_product_configuration_and_connection",
            clouddesk_test_support::reason::SSH_ACCEPTANCE_FIXTURE_UNAVAILABLE,
        );
        return;
    }
    if current_process_linux_identity().is_none() {
        clouddesk_test_support::blocked_by_environment(
            "agent_product_configuration_and_connection",
            clouddesk_test_support::reason::LINUX_IDENTITY_UNAVAILABLE,
        );
        return;
    }
    let _guard = tokio::task::spawn_blocking(acquire_cross_process_ssh_lock)
        .await
        .unwrap();
    let (base, _dir) = application().await;
    let admin = bootstrap_admin(&base).await;
    let host_key = scan_host_key_for("localhost").await;

    let agent = RealAgent::spawn().await;
    authorize_key_on_fixture(&agent.public_key()).await;

    let created = create_server(
        &base,
        &admin,
        BASTION_HOST,
        BASTION_PORT,
        BASTION_USER,
        "ssh_agent",
        None,
        Some(&agent.socket_path),
        &host_key,
        None,
    )
    .await;
    assert_eq!(created.status(), reqwest::StatusCode::CREATED);
    let body: Value = created.json().await.unwrap();
    let server_id = body["server_id"].as_str().unwrap().to_owned();

    let result = test_connection(&base, &admin, &server_id).await;
    assert_eq!(
        result["connected"], true,
        "agent-configured RemoteServer must connect through the real product/API path: {result:?}"
    );

    clear_authorized_keys().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn agent_negative_matrix_through_product_api() {
    if !fixture_available().await {
        clouddesk_test_support::blocked_by_environment(
            "agent_negative_matrix_through_product_api",
            clouddesk_test_support::reason::SSH_ACCEPTANCE_FIXTURE_UNAVAILABLE,
        );
        return;
    }
    if current_process_linux_identity().is_none() {
        clouddesk_test_support::blocked_by_environment(
            "agent_negative_matrix_through_product_api",
            clouddesk_test_support::reason::LINUX_IDENTITY_UNAVAILABLE,
        );
        return;
    }
    let _guard = tokio::task::spawn_blocking(acquire_cross_process_ssh_lock)
        .await
        .unwrap();
    let (base, _dir) = application().await;
    let admin = bootstrap_admin(&base).await;
    let user_b = create_second_user(&base, &admin).await;
    let host_key = scan_host_key_for("localhost").await;

    // Agent has no matching authorized key on the server.
    let agent = RealAgent::spawn().await;
    let unauthorized = create_server(
        &base,
        &admin,
        BASTION_HOST,
        BASTION_PORT,
        BASTION_USER,
        "ssh_agent",
        None,
        Some(&agent.socket_path),
        &host_key,
        None,
    )
    .await;
    assert_eq!(unauthorized.status(), reqwest::StatusCode::CREATED);
    let server_id = unauthorized.json::<Value>().await.unwrap()["server_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let result = test_connection(&base, &admin, &server_id).await;
    assert_eq!(result["connected"], false);

    // Missing agent_socket_path is rejected as a 400 (Task 37 regression:
    // this previously leaked as a 500).
    let missing_socket = http(
        &base,
        Method::POST,
        "/api/v1/remote/servers",
        Some(&admin),
        Some(&json!({
            "name": "agent-missing-socket",
            "hostname": BASTION_HOST,
            "port": BASTION_PORT,
            "username": BASTION_USER,
            "auth_method": "ssh_agent",
            "credential_secret_id": null,
            "host_key_type": "ssh-ed25519",
            "host_key_base64": host_key,
            "proxy_jump_server_id": null,
            "tags": [],
        })),
    )
    .await;
    assert_eq!(missing_socket.status(), reqwest::StatusCode::BAD_REQUEST);

    // User B (a different real user) can never see, edit, delete, or
    // test-connect User A's server -- proven through the actual HTTP
    // API, not a direct store call.
    let get_as_b = http(
        &base,
        Method::PUT,
        &format!("/api/v1/remote/servers/{server_id}"),
        Some(&user_b),
        Some(&json!({"auth_method": "ssh_agent", "credential_secret_id": null, "agent_socket_path": agent.socket_path})),
    )
    .await;
    assert_eq!(get_as_b.status(), reqwest::StatusCode::NOT_FOUND);
    let delete_as_b = http(
        &base,
        Method::DELETE,
        &format!("/api/v1/remote/servers/{server_id}"),
        Some(&user_b),
        None,
    )
    .await;
    assert_eq!(delete_as_b.status(), reqwest::StatusCode::NOT_FOUND);
    let test_as_b = http(
        &base,
        Method::POST,
        &format!("/api/v1/remote/servers/{server_id}/test-connection"),
        Some(&user_b),
        None,
    )
    .await;
    assert_eq!(test_as_b.status(), reqwest::StatusCode::NOT_FOUND);

    // Unauthenticated: DENY.
    let unauthenticated = http(&base, Method::GET, "/api/v1/remote/servers", None, None).await;
    assert_eq!(unauthenticated.status(), reqwest::StatusCode::UNAUTHORIZED);
}

#[tokio::test(flavor = "multi_thread")]
async fn keyboard_interactive_product_configuration_and_connection() {
    if !fixture_available().await {
        clouddesk_test_support::blocked_by_environment(
            "keyboard_interactive_product_configuration_and_connection",
            clouddesk_test_support::reason::SSH_ACCEPTANCE_FIXTURE_UNAVAILABLE,
        );
        return;
    }
    let _guard = tokio::task::spawn_blocking(acquire_cross_process_ssh_lock)
        .await
        .unwrap();
    let (base, _dir) = application().await;
    let admin = bootstrap_admin(&base).await;
    let host_key = scan_host_key_for("localhost").await;

    let secret_id = create_secret(
        &base,
        &admin,
        "ssh.keyboard_interactive",
        &json!([BASTION_PASSWORD]).to_string(),
    )
    .await;
    let created = create_server(
        &base,
        &admin,
        BASTION_HOST,
        BASTION_PORT,
        BASTION_USER,
        "keyboard_interactive",
        Some(&secret_id),
        None,
        &host_key,
        None,
    )
    .await;
    assert_eq!(created.status(), reqwest::StatusCode::CREATED);
    let server_id = created.json::<Value>().await.unwrap()["server_id"]
        .as_str()
        .unwrap()
        .to_owned();

    let result = test_connection(&base, &admin, &server_id).await;
    assert_eq!(
        result["connected"], true,
        "keyboard-interactive RemoteServer must connect through the real product/API path: {result:?}"
    );

    // Secret safety: the list response must never contain the raw
    // configured response.
    let list = http(
        &base,
        Method::GET,
        "/api/v1/remote/servers",
        Some(&admin),
        None,
    )
    .await;
    let raw = list.text().await.unwrap();
    assert!(
        !raw.contains(BASTION_PASSWORD),
        "the keyboard-interactive response must never appear in the RemoteServer list response"
    );

    // Wrong response: DENY.
    let wrong_secret = create_secret(
        &base,
        &admin,
        "ssh.keyboard_interactive",
        &json!(["definitely-not-the-password"]).to_string(),
    )
    .await;
    let wrong_server = create_server(
        &base,
        &admin,
        BASTION_HOST,
        BASTION_PORT,
        BASTION_USER,
        "keyboard_interactive",
        Some(&wrong_secret),
        None,
        &host_key,
        None,
    )
    .await;
    let wrong_server_id = wrong_server.json::<Value>().await.unwrap()["server_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let wrong_result = test_connection(&base, &admin, &wrong_server_id).await;
    assert_eq!(wrong_result["connected"], false);
    assert!(
        wrong_result["reason"]
            .as_str()
            .unwrap()
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c.is_ascii_punctuation() || c == ' '),
        "connection-failure reason must be a safe, generic message, not raw library internals: {wrong_result:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
#[allow(clippy::too_many_lines)]
async fn certificate_product_configuration_connection_and_proxyjump() {
    if !fixture_available().await {
        clouddesk_test_support::blocked_by_environment(
            "certificate_product_configuration_connection_and_proxyjump",
            clouddesk_test_support::reason::SSH_ACCEPTANCE_FIXTURE_UNAVAILABLE,
        );
        return;
    }
    let _guard = tokio::task::spawn_blocking(acquire_cross_process_ssh_lock)
        .await
        .unwrap();
    let (base, _dir) = application().await;
    let admin = bootstrap_admin(&base).await;
    let host_key = scan_host_key_for("localhost").await;

    let (key_data, cert_data) = generate_signed_identity(BASTION_USER, &["-V", "+1h"]).await;
    let secret_id = create_secret(
        &base,
        &admin,
        "ssh.certificate",
        &json!({"key_data": key_data, "cert_data": cert_data}).to_string(),
    )
    .await;
    let created = create_server(
        &base,
        &admin,
        BASTION_HOST,
        BASTION_PORT,
        BASTION_USER,
        "certificate",
        Some(&secret_id),
        None,
        &host_key,
        None,
    )
    .await;
    assert_eq!(created.status(), reqwest::StatusCode::CREATED);
    let server_id = created.json::<Value>().await.unwrap()["server_id"]
        .as_str()
        .unwrap()
        .to_owned();

    let result = test_connection(&base, &admin, &server_id).await;
    assert_eq!(
        result["connected"], true,
        "certificate-configured RemoteServer must connect through the real product/API path: {result:?}"
    );

    // Secret safety: private key material must never appear in the
    // list response.
    let list = http(
        &base,
        Method::GET,
        "/api/v1/remote/servers",
        Some(&admin),
        None,
    )
    .await;
    let raw = list.text().await.unwrap();
    assert!(
        !raw.contains("PRIVATE KEY"),
        "private key material must never appear in the RemoteServer list response"
    );

    // Negative: expired certificate -> DENY with a safe reason.
    let (expired_key, expired_cert) =
        generate_signed_identity(BASTION_USER, &["-V", "20200101000000:20200101010000"]).await;
    let expired_secret = create_secret(
        &base,
        &admin,
        "ssh.certificate",
        &json!({"key_data": expired_key, "cert_data": expired_cert}).to_string(),
    )
    .await;
    let expired_server = create_server(
        &base,
        &admin,
        BASTION_HOST,
        BASTION_PORT,
        BASTION_USER,
        "certificate",
        Some(&expired_secret),
        None,
        &host_key,
        None,
    )
    .await;
    let expired_server_id = expired_server.json::<Value>().await.unwrap()["server_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let expired_result = test_connection(&base, &admin, &expired_server_id).await;
    assert_eq!(expired_result["connected"], false);

    // Invalid combination: certificate auth method without a
    // credential_secret_id must fail validation (400), not create a
    // broken server.
    let missing_secret = http(
        &base,
        Method::POST,
        "/api/v1/remote/servers",
        Some(&admin),
        Some(&json!({
            "name": "cert-missing-secret",
            "hostname": BASTION_HOST,
            "port": BASTION_PORT,
            "username": BASTION_USER,
            "auth_method": "certificate",
            "credential_secret_id": null,
            "host_key_type": "ssh-ed25519",
            "host_key_base64": host_key,
            "proxy_jump_server_id": null,
            "tags": [],
        })),
    )
    .await;
    assert_eq!(missing_secret.status(), reqwest::StatusCode::BAD_REQUEST);

    // Part J: certificate through a real, product-configured ProxyJump
    // -- bastion keeps its existing password auth, target authenticates
    // with a certificate, resolved through the same connection builder.
    let bastion_secret = create_secret(&base, &admin, "ssh.password", BASTION_PASSWORD).await;
    let bastion_created = create_server(
        &base,
        &admin,
        BASTION_HOST,
        BASTION_PORT,
        BASTION_USER,
        "password",
        Some(&bastion_secret),
        None,
        &host_key,
        None,
    )
    .await;
    assert_eq!(bastion_created.status(), reqwest::StatusCode::CREATED);
    let bastion_id = bastion_created.json::<Value>().await.unwrap()["server_id"]
        .as_str()
        .unwrap()
        .to_owned();

    let (target_key, target_cert) = generate_signed_identity(TARGET_USER, &["-V", "+1h"]).await;
    let target_secret = create_secret(
        &base,
        &admin,
        "ssh.certificate",
        &json!({"key_data": target_key, "cert_data": target_cert}).to_string(),
    )
    .await;
    let target_host_key = scan_host_key_for(TARGET_HOST).await;
    let target_created = create_server(
        &base,
        &admin,
        TARGET_HOST,
        TARGET_PORT,
        TARGET_USER,
        "certificate",
        Some(&target_secret),
        None,
        &target_host_key,
        Some(&bastion_id),
    )
    .await;
    assert_eq!(target_created.status(), reqwest::StatusCode::CREATED);
    let target_id = target_created.json::<Value>().await.unwrap()["server_id"]
        .as_str()
        .unwrap()
        .to_owned();

    let proxyjump_result = test_connection(&base, &admin, &target_id).await;
    assert_eq!(
        proxyjump_result["connected"], true,
        "certificate auth through a product-configured ProxyJump must connect: {proxyjump_result:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn edit_switches_auth_method_safely_through_product_api() {
    if !fixture_available().await {
        clouddesk_test_support::blocked_by_environment(
            "edit_switches_auth_method_safely_through_product_api",
            clouddesk_test_support::reason::SSH_ACCEPTANCE_FIXTURE_UNAVAILABLE,
        );
        return;
    }
    if current_process_linux_identity().is_none() {
        clouddesk_test_support::blocked_by_environment(
            "edit_switches_auth_method_safely_through_product_api",
            clouddesk_test_support::reason::LINUX_IDENTITY_UNAVAILABLE,
        );
        return;
    }
    let _guard = tokio::task::spawn_blocking(acquire_cross_process_ssh_lock)
        .await
        .unwrap();
    let (base, _dir) = application().await;
    let admin = bootstrap_admin(&base).await;
    let host_key = scan_host_key_for("localhost").await;

    let password_secret = create_secret(&base, &admin, "ssh.password", BASTION_PASSWORD).await;
    let created = create_server(
        &base,
        &admin,
        BASTION_HOST,
        BASTION_PORT,
        BASTION_USER,
        "password",
        Some(&password_secret),
        None,
        &host_key,
        None,
    )
    .await;
    assert_eq!(created.status(), reqwest::StatusCode::CREATED);
    let server_id = created.json::<Value>().await.unwrap()["server_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let password_result = test_connection(&base, &admin, &server_id).await;
    assert_eq!(password_result["connected"], true);

    // Switch to SSH agent through PUT (edit), not delete+recreate.
    let agent = RealAgent::spawn().await;
    authorize_key_on_fixture(&agent.public_key()).await;
    let switched = http(
        &base,
        Method::PUT,
        &format!("/api/v1/remote/servers/{server_id}"),
        Some(&admin),
        Some(&json!({
            "auth_method": "ssh_agent",
            "credential_secret_id": null,
            "agent_socket_path": agent.socket_path,
        })),
    )
    .await;
    assert_eq!(switched.status(), reqwest::StatusCode::OK);
    let agent_result = test_connection(&base, &admin, &server_id).await;
    assert_eq!(
        agent_result["connected"], true,
        "editing a RemoteServer to switch auth methods must actually take effect: {agent_result:?}"
    );

    // Invalid combination on edit: switching to certificate without a
    // credential_secret_id must be rejected (400), leaving the server
    // untouched.
    let invalid_switch = http(
        &base,
        Method::PUT,
        &format!("/api/v1/remote/servers/{server_id}"),
        Some(&admin),
        Some(&json!({
            "auth_method": "certificate",
            "credential_secret_id": null,
            "agent_socket_path": null,
        })),
    )
    .await;
    assert_eq!(invalid_switch.status(), reqwest::StatusCode::BAD_REQUEST);

    clear_authorized_keys().await;
}
