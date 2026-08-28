//! Phase 9 Pass 3A-2 (see `PHASE9_BROWSER_EVIDENCE.md`): real
//! Playwright-driven evidence for the COMPILED `CloudDesk` frontend's
//! Browser app -- not the broker's WebSocket protocol driven directly
//! (that evidence already exists in `browser_broker.rs`). This is the
//! first evidence that a real user, in a real browser, clicking real
//! UI, gets a working Browser experience end to end.

use axum::extract::connect_info::ConnectInfo;
use axum::extract::State;
use axum::http::Method;
use axum::response::IntoResponse;
use axum::routing::get;
use clouddesk_auth::{AuthPolicy, AuthService};
use clouddesk_secrets::SecretCipher;
use serde_json::{json, Value};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::process::Command as TokioCommand;
use tokio::sync::Mutex as TokioMutex;

const BROWSER_IMAGE: &str = "clouddesk-brave:1.93.136";
const PLAYWRIGHT_IMAGE: &str = "mcr.microsoft.com/playwright:v1.49.0-noble";

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

/// Same cross-process serialization every other real-Brave test file
/// uses -- this file also starts real Brave containers.
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

/// Same product router harness as `browser_broker.rs`, but pointed at
/// the REAL compiled frontend (`apps/web/dist`, already built by this
/// session's `npm run build`) instead of a bare temp dir -- a real
/// Playwright browser needs an actual UI to click on. Falls back to a
/// bare temp dir (API-only, no real UI) if the frontend hasn't been
/// built, so this harness never panics merely because the frontend
/// wasn't built -- scenarios then fail to find UI elements, which is
/// the honest outcome for "the frontend was never built".
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
        SecretCipher::new(&[109_u8; 32]).unwrap(),
        AuthPolicy::default(),
    )
    .unwrap();
    let directory = tempfile::tempdir().unwrap();
    let secret_path = directory.path().join("bootstrap.secret");
    std::fs::write(&secret_path, "browser-playwright-test-secret\n").unwrap();

    let runtime_manager = std::sync::Arc::new(
        clouddesk_orchestrator::RuntimeManager::new(
            clouddesk_orchestrator::store::RuntimeStore::new(pool.clone()),
            std::env::temp_dir().join(format!(
                "clouddesk-browser-playwright-test-{}",
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
            "secret": "browser-playwright-test-secret",
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

/// Task 17/18 of the Phase 8 pass: a fixture site with a unique
/// sentinel, a button/checkbox/text-input that each report back via
/// `fetch()` when interacted with (so real broker-dispatched input can
/// be verified without any generic CDP eval capability), a popup
/// trigger, and safe request-source logging for the server-side-origin
/// proof -- same pattern as `browser_broker.rs`'s fixture, duplicated
/// here per this codebase's established one-fixture-per-test-file
/// convention.
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
<div id="sentinel">CLOUDDESK-BROWSER-PLAYWRIGHT-FIXTURE-SENTINEL</div>
<button id="btn" style="position:absolute;left:20px;top:20px;width:100px;height:40px;"
  onclick="document.getElementById('btn').style.background='red'; fetch('/click', {method:'POST'});">
  Click me
</button>
<input id="txt" type="text" style="position:absolute;left:20px;top:80px;"
  oninput="fetch('/input?value=' + encodeURIComponent(this.value))" />
<input id="chk" type="checkbox" style="position:absolute;left:20px;top:120px;"
  onchange="fetch('/checkbox?checked=' + this.checked)" />
<button id="popup" style="position:absolute;left:20px;top:160px;width:100px;height:40px;"
  onclick="window.open('/page2', '_blank')">
  Open popup
</button>
</body></html>"#,
    )
}

async fn fixture_page2() -> impl IntoResponse {
    axum::response::Html(
        r#"<!doctype html><html><body><div id="sentinel2">PAGE2</div></body></html>"#,
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
        .route("/page2", get(fixture_page2))
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

/// Runs the disposable Playwright/Chromium container (test
/// infrastructure only, pinned, never a runtime dependency) against
/// `browser_flow.mjs`. `--network host` lets it reach this test's own
/// loopback-bound `clouddeskd` listener directly. Removed
/// unconditionally afterward (`--rm`).
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

/// Task 1/2/3: real Playwright, driving the real compiled frontend,
/// exercising login -> Browser app -> real screencast frame -> real
/// navigation -> real click/type/checkbox/scroll -> real second tab ->
/// real popup, verified two ways: what Playwright itself observed
/// (canvas pixels, tab-strip DOM count) and what the controlled
/// fixture server independently recorded (click/input/request-source),
/// so the proof does not depend on Playwright's own account agreeing
/// with itself.
#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn task_1_2_3_playwright_compiled_frontend_full_flow() {
    if !docker_and_images_available().await {
        clouddesk_test_support::blocked_by_environment(
            "task_1_2_3_playwright_compiled_frontend_full_flow",
            clouddesk_test_support::reason::CONTAINER_RUNTIME_UNAVAILABLE,
        );
        return;
    }
    let (base, _dir, _runtime_manager) = application().await;
    let _cross_process_guard = tokio::task::spawn_blocking(acquire_cross_process_browser_lock)
        .await
        .unwrap();
    let _brave_container_guard = BraveContainerGuard::new();
    let admin_cookie = bootstrap_admin(&base).await;
    enable_browser(&base, &admin_cookie).await;
    let password = "user horse battery staple";
    create_user(&base, &admin_cookie, "playwrightuser", password).await;

    let (fixture_url_template, fixture_log) = spawn_fixture_site().await;
    let gateway = bridge_gateway_ip().await;
    clouddeskd::browser_egress_proxy::set_test_allowlist([gateway.parse().unwrap()]);
    let fixture_url = fixture_url_template.replace("REPLACE_WITH_GATEWAY", &gateway);

    let result = run_browser_scenario(
        "full_flow",
        &json!({
            "base": base,
            "username": "playwrightuser",
            "password": password,
            "fixtureUrl": fixture_url,
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
        result["frameArrived"],
        json!(true),
        "a real, non-blank screencast frame must be drawn onto the canvas: {result:?}"
    );
    assert_eq!(
        result["iframeCount"],
        json!(0),
        "the compiled frontend must never render the target site as an iframe: {result:?}"
    );
    assert_eq!(
        result["gotSecondTab"],
        json!(true),
        "creating a second tab through the real UI must work: {result:?}"
    );
    assert_eq!(
        result["backToOneTab"],
        json!(true),
        "closing a tab through the real UI must work: {result:?}"
    );
    assert_eq!(
        result["popupBecameTab"],
        json!(true),
        "a real window.open() popup must appear as a managed tab in the real UI: {result:?}"
    );

    // Task 2/18: the fixture's own independent log must show the
    // click/input really landed, and that the request came from
    // Brave's own container network (server-side origin), not from
    // Playwright's own browser (which never talks to the fixture
    // directly -- it only ever clicks/types into the CloudDesk page).
    let log = fixture_log.lock().await;
    assert!(log.click_count > 0, "the real button click dispatched through the compiled frontend must reach the real Brave page");
    assert!(
        log.last_input_value
            .as_deref()
            .is_some_and(|v| !v.is_empty()),
        "real keyboard input must reach the real Brave page's text input"
    );
    let remote = log.last_remote_addr.clone().unwrap_or_default();
    assert_ne!(remote, "127.0.0.1", "the fixture must be reached by Brave's own container, never by the Playwright browser directly");
    let ua = log.last_user_agent.clone().unwrap_or_default();
    assert!(
        ua.contains("Chrome") || ua.contains("Brave") || ua.contains("HeadlessChrome"),
        "the fixture must observe a real Brave User-Agent, got {ua:?}"
    );
}
