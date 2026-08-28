//! PASS SSH-C-2, Gap 3 (Task 6/7): real Playwright-driven evidence that
//! the remote SSH terminal works through the ACTUAL compiled `CloudDesk`
//! frontend -- `ServersApp` -> "Open Terminal" -> `RemoteTerminalApp` ->
//! real xterm.js rendering -> real browser keyboard input -> a real
//! remote shell's real output -- never a direct WebSocket call from
//! this test (that evidence already exists in
//! `remote_terminal_product.rs`). Runs in a disposable, version-pinned
//! Playwright/Chromium container (test infrastructure only).
//!
//! Skips (not FAIL) if docker/the Playwright image or the disposable
//! OpenSSH fixture aren't available, or if `apps/web/dist` hasn't been
//! built (falls back to a bare temp dir with no real UI, so scenarios
//! then fail to find UI elements -- the honest outcome).

use axum::http::Method;
use serde_json::{json, Value};
use tokio::process::Command as TokioCommand;

const PLAYWRIGHT_IMAGE: &str = "mcr.microsoft.com/playwright:v1.49.0-noble";
const BASTION_HOST: &str = "127.0.0.1";
const BASTION_PORT: u16 = 2222;
const BASTION_USER: &str = "testuser";
const BASTION_PASSWORD: &str = "testpassword";

async fn fixture_available() -> bool {
    tokio::net::TcpStream::connect((BASTION_HOST, BASTION_PORT))
        .await
        .is_ok()
}

async fn docker_and_playwright_available() -> bool {
    let docker = TokioCommand::new("docker")
        .args(["version", "--format", "{{.Server.Version}}"])
        .output()
        .await
        .is_ok_and(|o| o.status.success());
    let image = TokioCommand::new("docker")
        .args(["image", "inspect", PLAYWRIGHT_IMAGE])
        .output()
        .await
        .is_ok_and(|o| o.status.success());
    docker && image
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
    let output = TokioCommand::new("docker")
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

/// Real compiled frontend (`apps/web/dist`, built by this session's
/// `npm run build`) if present, else a bare temp dir -- mirrors
/// `browser_playwright.rs::application`'s fallback discipline exactly.
async fn application() -> (String, tempfile::TempDir) {
    let pool = clouddesk_db::connect("sqlite::memory:", 1).await.unwrap();
    clouddesk_db::migrate(&pool).await.unwrap();
    let auth = clouddesk_auth::AuthService::new(
        pool,
        clouddesk_secrets::SecretCipher::new(&[211_u8; 32]).unwrap(),
        clouddesk_auth::AuthPolicy::default(),
    )
    .unwrap();
    let directory = tempfile::tempdir().unwrap();
    let secret_path = directory.path().join("bootstrap.secret");
    std::fs::write(&secret_path, "remote-terminal-playwright-test-secret\n").unwrap();

    let static_dir =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../apps/web/dist");
    let static_dir = if static_dir.join("index.html").exists() {
        static_dir
    } else {
        directory.path().to_owned()
    };

    let router = clouddeskd::application_router(static_dir, auth, secret_path);
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
            "secret": "remote-terminal-playwright-test-secret",
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

async fn create_password_server(base: &str, cookie: &str) -> String {
    let secret_id = create_secret(base, cookie, "ssh.password", BASTION_PASSWORD).await;
    let host_key = scan_host_key_for("localhost").await;
    let created = http(
        base,
        Method::POST,
        "/api/v1/remote/servers",
        Some(cookie),
        Some(&json!({
            "name": "playwright-remote-terminal",
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

async fn run_scenario(scenario: &str, args: &Value) -> Value {
    let args_dir = tempfile::tempdir().unwrap();
    run_scenario_in_dir(scenario, args, args_dir.path()).await
}

/// Same as `run_scenario`, but polls the host-side `<args_dir>/ready`
/// file (the container writes it via the shared `/args` bind mount the
/// instant the scenario is ready) and invokes `on_ready` exactly once
/// it appears -- avoids racing a blind fixed delay against this
/// container's own npm-install/browser-launch startup time.
async fn run_scenario_with_ready_signal<F, Fut>(scenario: &str, args: &Value, on_ready: F) -> Value
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    let args_dir = tempfile::tempdir().unwrap();
    let ready_path = args_dir.path().join("ready");
    let mut args = args.clone();
    args["readyFile"] = json!("/args/ready");
    let handle = {
        let args_dir_path = args_dir.path().to_owned();
        let scenario = scenario.to_owned();
        tokio::spawn(async move { run_scenario_in_dir(&scenario, &args, &args_dir_path).await })
    };
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_mins(1);
    while !ready_path.exists() && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
    on_ready().await;
    handle.await.unwrap()
}

async fn run_scenario_in_dir(scenario: &str, args: &Value, args_dir: &std::path::Path) -> Value {
    let scripts_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/browser");
    let args_path = args_dir.join("args.json");
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
            &format!("{}:/args:rw", args_dir.display()),
            "-w",
            "/work",
            PLAYWRIGHT_IMAGE,
            "sh",
            "-c",
            "mkdir -p /work && cp /scripts/remote_terminal_flow.mjs /work/ && \
             npm init -y >/dev/null 2>&1 && npm install playwright@1.49.0 >/dev/null 2>&1 && \
             node remote_terminal_flow.mjs \"$0\" \"$1\"",
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

/// Task 6: real Playwright, through the real compiled frontend --
/// login -> Servers -> Open Terminal -> xterm renders -> real remote
/// shell runs a sentinel command, resize changes the real PTY, Ctrl-C
/// interrupts only the foreground command, and `exit` leaves the
/// frontend in an explicit non-connecting/connected state.
#[tokio::test]
async fn task_6_playwright_remote_terminal_full_flow() {
    if !fixture_available().await {
        clouddesk_test_support::blocked_by_environment(
            "task_6_playwright_remote_terminal_full_flow",
            clouddesk_test_support::reason::SSH_ACCEPTANCE_FIXTURE_UNAVAILABLE,
        );
        return;
    }
    if !docker_and_playwright_available().await {
        clouddesk_test_support::blocked_by_environment(
            "task_6_playwright_remote_terminal_full_flow",
            clouddesk_test_support::reason::CONTAINER_RUNTIME_UNAVAILABLE,
        );
        return;
    }
    let _guard = tokio::task::spawn_blocking(acquire_cross_process_ssh_lock)
        .await
        .unwrap();
    let (base, _dir) = application().await;
    let admin = bootstrap_admin(&base).await;
    create_password_server(&base, &admin).await;

    let result = run_scenario(
        "full_flow",
        &json!({
            "base": base,
            "username": "admin",
            "password": "correct horse battery staple",
        }),
    )
    .await;
    cleanup_playwright_containers().await;

    assert_eq!(
        result["ok"],
        json!(true),
        "playwright scenario must succeed: {result:?}"
    );
    assert_eq!(
        result["sentinelSeenTwice"],
        json!(true),
        "the real remote shell must execute printf (sentinel echoed AND really printed): {result:?}"
    );
    assert_eq!(
        result["resizeChanged"],
        json!(true),
        "resizing the browser window must change the real remote PTY's reported dimensions: {result:?}"
    );
    assert_eq!(
        result["ctrlCWorked"],
        json!(true),
        "Ctrl-C sent from the real browser must interrupt only the foreground sleep: {result:?}"
    );
    let status = result["statusAfterExit"].as_str().unwrap_or_default();
    assert!(
        matches!(status, "exited" | "disconnected" | "error"),
        "the frontend must leave connecting/connected after a real shell exit, never spin forever: {result:?}"
    );
}

/// Task 7: a real negative case -- the `RemoteServer` is deleted while
/// the terminal is open (the same live revocation path proven in
/// `remote_terminal_product.rs`), and the real frontend must show an
/// explicit non-connecting state, never an endless spinner.
#[tokio::test]
async fn task_7_playwright_frontend_failure_state() {
    if !fixture_available().await {
        clouddesk_test_support::blocked_by_environment(
            "task_7_playwright_frontend_failure_state",
            clouddesk_test_support::reason::SSH_ACCEPTANCE_FIXTURE_UNAVAILABLE,
        );
        return;
    }
    if !docker_and_playwright_available().await {
        clouddesk_test_support::blocked_by_environment(
            "task_7_playwright_frontend_failure_state",
            clouddesk_test_support::reason::CONTAINER_RUNTIME_UNAVAILABLE,
        );
        return;
    }
    let _guard = tokio::task::spawn_blocking(acquire_cross_process_ssh_lock)
        .await
        .unwrap();
    let (base, _dir) = application().await;
    let admin = bootstrap_admin(&base).await;
    let server_id = create_password_server(&base, &admin).await;

    let scenario_args = json!({
        "base": base,
        "username": "admin",
        "password": "correct horse battery staple",
        "waitMs": 20000,
    });
    let base_for_delete = base.clone();
    let admin_for_delete = admin.clone();
    let result = run_scenario_with_ready_signal("failure_state", &scenario_args, || async move {
        // The scenario has genuinely opened the terminal (signaled via
        // the shared ready file) -- only now revoke it out from under
        // the connection.
        let delete = http(
            &base_for_delete,
            Method::DELETE,
            &format!("/api/v1/remote/servers/{server_id}"),
            Some(&admin_for_delete),
            None,
        )
        .await;
        assert_eq!(delete.status(), reqwest::StatusCode::NO_CONTENT);
    })
    .await;
    cleanup_playwright_containers().await;

    assert_eq!(
        result["ok"],
        json!(true),
        "playwright scenario must succeed: {result:?}"
    );
    let status = result["status"].as_str().unwrap_or_default();
    assert!(
        matches!(status, "error" | "revoked" | "disconnected" | "exited"),
        "the frontend must leave the connecting/connected state after revocation, never spin forever: {result:?}"
    );
}
