//! Phase 9 Pass 3B-3, Task 3: real Playwright-driven evidence that a
//! user can select an authorized remote-VFS (SFTP) file through the
//! ACTUAL compiled `CloudDesk` frontend UI -- not the broker WebSocket
//! protocol driven directly (that evidence already exists in
//! `browser_remote_uploads.rs`), and never a raw/manually-supplied
//! `server_id`. Login -> Browser app -> a real controlled website's
//! `<input type=file>` -> the real chooser modal -> a real click on
//! "Remote server file" -> a real `<select>` option chosen by its
//! visible label -> a real typed path -> a real click on Select ->
//! the real website receives the real remote file's bytes, verified
//! byte-exact and filename-exact.

use axum::http::Method;
use axum::response::IntoResponse;
use clouddesk_auth::{AuthPolicy, AuthService};
use clouddesk_remote::{NewRemoteServer, RemoteServerStore, SshAuthMethod};
use clouddesk_secrets::SecretCipher;
use clouddesk_vault::Vault;
use serde_json::{json, Value};
use sqlx::SqlitePool;
use std::sync::Arc;
use tokio::process::Command as TokioCommand;
use tokio::sync::Mutex as TokioMutex;

const BROWSER_IMAGE: &str = "clouddesk-brave:1.93.136";
const PLAYWRIGHT_IMAGE: &str = "mcr.microsoft.com/playwright:v1.49.0-noble";
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

async fn docker_available() -> bool {
    TokioCommand::new("docker")
        .args(["version", "--format", "{{.Server.Version}}"])
        .output()
        .await
        .is_ok_and(|o| o.status.success())
}

async fn image_available(image: &str) -> bool {
    TokioCommand::new("docker")
        .args(["image", "inspect", image])
        .output()
        .await
        .is_ok_and(|o| o.status.success())
}

async fn docker_and_images_available() -> bool {
    docker_available().await
        && image_available(BROWSER_IMAGE).await
        && image_available(PLAYWRIGHT_IMAGE).await
}

/// Same product router harness as `browser_playwright.rs`, pointed at
/// the real compiled frontend (`apps/web/dist`) so a real Playwright
/// browser has real UI to click.
async fn application() -> (String, tempfile::TempDir, SqlitePool) {
    clouddeskd::browser_egress_proxy::spawn();
    let pool = clouddesk_db::connect("sqlite::memory:", 1).await.unwrap();
    clouddesk_db::migrate(&pool).await.unwrap();
    let auth = AuthService::new(
        pool.clone(),
        SecretCipher::new(&[113_u8; 32]).unwrap(),
        AuthPolicy::default(),
    )
    .unwrap();
    let directory = tempfile::tempdir().unwrap();
    let secret_path = directory.path().join("bootstrap.secret");
    std::fs::write(&secret_path, "browser-remote-upload-playwright-secret\n").unwrap();

    let runtime_root = tempfile::tempdir().unwrap();
    std::mem::forget(runtime_root);
    let runtime_manager = std::sync::Arc::new(
        clouddesk_orchestrator::RuntimeManager::new(
            clouddesk_orchestrator::store::RuntimeStore::new(pool.clone()),
            std::env::temp_dir().join(format!(
                "clouddesk-browser-remote-upload-playwright-{}",
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

    let static_dir =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../apps/web/dist");
    let static_dir = if static_dir.join("index.html").exists() {
        static_dir
    } else {
        directory.path().to_owned()
    };

    let router = clouddeskd::application_router_and_media_and_library_and_runtime_configured(
        static_dir,
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
            "secret": "browser-remote-upload-playwright-secret",
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

async fn create_user(base: &str, admin_cookie: &str, username: &str, password: &str) -> String {
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
        Some(&json!({"username": username, "display_name": username, "password": password, "role_ids": ["user"]})),
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
    user_id
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

/// Registers a `RemoteServer` for `owner_user_id` against the real
/// disposable OpenSSH fixture, matching the established
/// `office_remote_vfs.rs`/`browser_remote_uploads.rs` pattern.
async fn register_remote_server(pool: &SqlitePool, owner_user_id: &str, name: &str) -> String {
    let vault = Vault::new(pool.clone(), SecretCipher::new(&[113_u8; 32]).unwrap());
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
                name: name.to_owned(),
                hostname: BASTION_HOST.to_owned(),
                port: BASTION_PORT,
                username: BASTION_USER.to_owned(),
                auth_method: SshAuthMethod::Password,
                credential_secret_id: Some(secret_id),
                agent_socket_path: None,
                host_key_type: "ssh-ed25519".to_owned(),
                host_key_base64: host_key,
                proxy_jump_server_id: None,
                tags: vec![],
            },
        )
        .await
        .unwrap()
}

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

/// Same real `<input type=file>` fixture page/receiver as
/// `browser_uploads.rs`, duplicated per this project's own
/// established one-fixture-per-test-file convention.
const CHOOSER_PAGE: &str = "<!doctype html><html><body>\
<input id=\"fi\" type=\"file\" style=\"position:absolute;left:5px;top:5px;width:100px;height:30px;\">\
<script>\
document.getElementById('fi').addEventListener('change', async (e) => {\
  const f = e.target.files[0];\
  const buf = await f.arrayBuffer();\
  await fetch('/received', {method: 'POST', headers: {'X-Filename': f.name}, body: buf});\
});\
</script></body></html>";

type ReceivedFile = Arc<TokioMutex<Option<(String, Vec<u8>)>>>;

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
    let received: ReceivedFile = Arc::new(TokioMutex::new(None));
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

async fn browser_gateway_ip() -> String {
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

async fn run_browser_scenario(scenario: &str, args: &Value) -> Value {
    let scripts_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/browser");
    let args_dir = tempfile::tempdir().unwrap();
    let args_path = args_dir.path().join("args.json");
    std::fs::write(&args_path, serde_json::to_vec(args).unwrap()).unwrap();

    let output = TokioCommand::new("docker")
        .args([
            "run",
            "--rm",
            "--network",
            "host",
            "-v",
            &format!("{}:/scripts:ro", scripts_dir.display()),
            "-v",
            &format!("{}:/args:rw", args_dir.path().display()),
            "-w",
            "/work",
            PLAYWRIGHT_IMAGE,
            "sh",
            "-c",
            "mkdir -p /work && cp /scripts/browser_flow.mjs /work/ && \
             npm init -y >/dev/null 2>&1 && npm install playwright@1.49.0 >/dev/null 2>&1 && \
             node browser_flow.mjs \"$0\" \"$1\"",
            scenario,
            "/args/args.json",
        ])
        .output()
        .await
        .expect("failed to run playwright container");

    let stdout = String::from_utf8_lossy(&output.stdout);
    eprintln!(
        "[{scenario}] playwright stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let last_line = stdout.lines().last().unwrap_or("");
    serde_json::from_str(last_line).unwrap_or_else(|e| {
        json!({
            "ok": false,
            "error": format!("could not parse playwright output: {e}"),
            "stdout": stdout.to_string(),
            "stderr": String::from_utf8_lossy(&output.stderr).to_string(),
        })
    })
}

async fn cleanup_playwright_containers() {
    let ps = TokioCommand::new("docker")
        .args([
            "ps",
            "-a",
            "-q",
            "--filter",
            &format!("ancestor={PLAYWRIGHT_IMAGE}"),
        ])
        .output()
        .await
        .unwrap();
    for id in String::from_utf8_lossy(&ps.stdout).lines() {
        if !id.trim().is_empty() {
            let _ = TokioCommand::new("docker")
                .args(["rm", "-f", id])
                .output()
                .await;
        }
    }
}

/// Task 3: the missing product evidence -- a real user, in a real
/// browser, selects a real authorized remote-VFS (SFTP) file through
/// the actual compiled chooser UI (never a raw `server_id` supplied by
/// this test's own protocol), and the real bytes/filename arrive at a
/// controlled receiving website.
#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn task_3_playwright_remote_upload_through_compiled_ui() {
    if !openssh_fixture_available().await {
        eprintln!("SKIP: disposable OpenSSH fixture not running (docker compose up -d in tests/acceptance)");
        return;
    }
    if !docker_and_images_available().await {
        eprintln!("SKIP: docker/{BROWSER_IMAGE}/{PLAYWRIGHT_IMAGE} not available");
        return;
    }
    let (base, _dir, pool) = application().await;
    let _cross_process_guard = tokio::task::spawn_blocking(acquire_cross_process_browser_lock)
        .await
        .unwrap();
    let _brave_container_guard = BraveContainerGuard::new();

    let admin_cookie = bootstrap_admin(&base).await;
    enable_browser(&base, &admin_cookie).await;
    let password = "user horse battery staple";
    let user_id = create_user(&base, &admin_cookie, "playwrightremoteuser", password).await;
    let server_name = "acceptance-sftp-fixture";
    register_remote_server(&pool, &user_id, server_name).await;

    let remote_name = format!("playwright-remote-upload-{}.bin", std::process::id());
    let sentinel_content = b"CloudDesk real product-UI remote SFTP upload acceptance payload 2026.";
    seed_remote_file(&remote_name, sentinel_content).await;

    let (fixture_port, received) = spawn_chooser_fixture().await;
    let browser_gw = browser_gateway_ip().await;
    clouddeskd::browser_egress_proxy::set_test_allowlist([browser_gw.parse().unwrap()]);
    // The fixture is reached by Brave (inside its own Docker network)
    // at the Browser network's own gateway address, exactly like every
    // other Browser test's controlled fixture.
    let fixture_url = format!("http://{browser_gw}:{fixture_port}/");
    let _bridge_gw = bridge_gateway_ip().await; // documents the alternate bridge gateway is not used here

    let result = run_browser_scenario(
        "remote_upload",
        &json!({
            "base": base,
            "username": "playwrightremoteuser",
            "password": password,
            "fixtureUrl": fixture_url,
            "serverOptionLabel": format!("{server_name} ({BASTION_HOST})"),
            "remoteFileName": remote_name,
        }),
    )
    .await;
    cleanup_playwright_containers().await;

    assert_eq!(
        result["ok"],
        json!(true),
        "playwright remote_upload scenario must succeed: {result:?}"
    );
    assert_eq!(
        result["chooserClosed"],
        json!(true),
        "the chooser must close cleanly after a real successful selection through the UI: {result:?}"
    );
    assert_eq!(
        result["errorText"],
        Value::Null,
        "no error should be shown for the user's own authorized remote file: {result:?}"
    );

    let (filename, bytes) = poll_received(&received, std::time::Duration::from_secs(15))
        .await
        .expect("the controlled website must receive the real remote file's real bytes");
    assert_eq!(
        filename, remote_name,
        "the real remote filename must be preserved through the real UI flow"
    );
    assert_eq!(
        bytes, sentinel_content,
        "the website must receive exactly the remote file's real bytes, selected entirely through the compiled product UI"
    );

    remove_remote_file(&remote_name).await;
}

/// Task 4: the same chooser UI, defaulted to local ("`CloudDesk`
/// file"), selects a real file from the user's own home -- proves the
/// picker didn't need two incompatible workflows, and is a real
/// regression check for the pre-existing local-upload capability now
/// that the chooser UI has grown a source selector.
#[tokio::test]
async fn task_4_playwright_local_upload_regression_through_same_ui() {
    if !docker_and_images_available().await {
        eprintln!("SKIP: docker/{BROWSER_IMAGE}/{PLAYWRIGHT_IMAGE} not available");
        return;
    }
    let (base, _dir, _pool) = application().await;
    let _cross_process_guard = tokio::task::spawn_blocking(acquire_cross_process_browser_lock)
        .await
        .unwrap();
    let _brave_container_guard = BraveContainerGuard::new();

    let admin_cookie = bootstrap_admin(&base).await;
    enable_browser(&base, &admin_cookie).await;
    let password = "user horse battery staple";
    create_user(&base, &admin_cookie, "playwrightlocaluser", password).await;

    let home = current_process_linux_identity().unwrap().home;
    let local_name = format!("playwright-local-upload-{}.bin", std::process::id());
    let local_path = home.join(&local_name);
    let sentinel_content = b"CloudDesk real product-UI local upload regression payload 2026.";
    std::fs::write(&local_path, sentinel_content).unwrap();

    let (fixture_port, received) = spawn_chooser_fixture().await;
    let browser_gw = browser_gateway_ip().await;
    clouddeskd::browser_egress_proxy::set_test_allowlist([browser_gw.parse().unwrap()]);
    let fixture_url = format!("http://{browser_gw}:{fixture_port}/");

    let result = run_browser_scenario(
        "local_upload",
        &json!({
            "base": base,
            "username": "playwrightlocaluser",
            "password": password,
            "fixtureUrl": fixture_url,
            "localFileName": local_name,
        }),
    )
    .await;
    cleanup_playwright_containers().await;
    let _ = std::fs::remove_file(&local_path);

    assert_eq!(
        result["ok"],
        json!(true),
        "playwright local_upload scenario must succeed: {result:?}"
    );
    assert_eq!(
        result["chooserClosed"],
        json!(true),
        "the chooser must close cleanly after a real successful local selection: {result:?}"
    );
    assert_eq!(
        result["errorText"],
        Value::Null,
        "no error expected: {result:?}"
    );

    let (filename, bytes) = poll_received(&received, std::time::Duration::from_secs(15))
        .await
        .expect("the controlled website must receive the real local file's real bytes");
    assert_eq!(filename, local_name);
    assert_eq!(
        bytes, sentinel_content,
        "the website must receive exactly the local file's real bytes through the same UI"
    );
}
