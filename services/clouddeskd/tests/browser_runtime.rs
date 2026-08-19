//! Phase 9 (foundation pass -- see `PHASE9_BROWSER_EVIDENCE.md`): real
//! integration evidence for the Brave OCI runtime adapter registered
//! in `browser_runtime.rs`. This proves the adapter genuinely
//! integrates with the real Phase 6 `RuntimeManager` and the real
//! generic `/api/v1/runtime-instances` HTTP surface -- it does not
//! test a browser broker, frame streaming, or input handling, none of
//! which exist yet.

use axum::http::Method;
use clouddesk_auth::{AuthPolicy, AuthService};
use clouddesk_secrets::SecretCipher;
use serde_json::{json, Value};
use tokio::process::Command as TokioCommand;

const BROWSER_IMAGE: &str = "clouddesk-brave:1.93.136";

async fn docker_and_image_available() -> bool {
    TokioCommand::new("docker")
        .args(["image", "inspect", BROWSER_IMAGE])
        .output()
        .await
        .is_ok_and(|o| o.status.success())
}

async fn application() -> (String, tempfile::TempDir) {
    let pool = clouddesk_db::connect("sqlite::memory:", 1).await.unwrap();
    clouddesk_db::migrate(&pool).await.unwrap();
    let auth = AuthService::new(
        pool.clone(),
        SecretCipher::new(&[97_u8; 32]).unwrap(),
        AuthPolicy::default(),
    )
    .unwrap();
    let directory = tempfile::tempdir().unwrap();
    let secret_path = directory.path().join("bootstrap.secret");
    std::fs::write(&secret_path, "browser-runtime-test-secret\n").unwrap();

    let runtime_root = tempfile::tempdir().unwrap();
    std::mem::forget(runtime_root);
    let runtime_manager = std::sync::Arc::new(
        clouddesk_orchestrator::RuntimeManager::new(
            clouddesk_orchestrator::store::RuntimeStore::new(pool.clone()),
            std::env::temp_dir().join(format!(
                "clouddesk-browser-runtime-test-{}",
                std::process::id()
            )),
            clouddesk_orchestrator::ResourcePolicy {
                start_timeout: std::time::Duration::from_secs(30),
                health_timeout: std::time::Duration::from_secs(20),
                // Code/Office's shared default (64) is far too tight
                // for a real Chromium-family browser -- live-verified
                // this pass: at 64, Brave's own zygote/GPU/renderer
                // process tree hits the pids cgroup ceiling immediately
                // (`pthread_create: Resource temporarily unavailable`)
                // and never reaches a working state. `ResourcePolicy`
                // is currently one struct shared by every adapter this
                // `RuntimeManager` registers (not yet per-kind), so
                // this override is scoped to this test's own manager
                // only -- wiring a real per-kind resource policy into
                // the product's own `main.rs` remains a documented
                // follow-up, not done this pass.
                pids_limit: Some(512),
                ..clouddesk_orchestrator::ResourcePolicy::default()
            },
        )
        .with_adapter(std::sync::Arc::new(
            clouddesk_orchestrator::oci::OciAdapter::new(
                clouddeskd::browser_runtime::browser_oci_spec(BROWSER_IMAGE.to_owned()),
            ),
        )),
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
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
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
            "secret": "browser-runtime-test-secret",
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

/// Real, live evidence for Task 1-3 only: a genuine pinned Brave
/// container, started through the real `RuntimeManager`/generic
/// `/api/v1/runtime-instances` HTTP surface (no Browser-specific route
/// code exists -- this exercises the same generic path Code/Office
/// share), reaching a real `Running` state defined exactly as Task 3
/// requires (process alive AND the real CDP HTTP endpoint answering,
/// not PID existence alone -- the OCI adapter's health check is a real
/// GET against `/json/version` relayed from Brave's own loopback-only
/// `DevTools` port). Then stopped, and the container's real removal is
/// verified via `docker inspect`.
#[tokio::test]
async fn task_1_2_3_brave_runtime_reaches_real_running_state() {
    if !docker_and_image_available().await {
        eprintln!("SKIP: docker/{BROWSER_IMAGE} not available (build docker/brave first)");
        return;
    }
    let (base, _dir) = application().await;
    let admin_cookie = bootstrap_admin(&base).await;

    let enable = http(
        &base,
        Method::POST,
        "/api/v1/runtimes/browser/enable",
        Some(&admin_cookie),
        None,
    )
    .await;
    assert_eq!(enable.status(), reqwest::StatusCode::NO_CONTENT);

    let create = http(
        &base,
        Method::POST,
        "/api/v1/runtime-instances",
        Some(&admin_cookie),
        Some(&json!({"kind": "browser"})),
    )
    .await;
    assert_eq!(
        create.status(),
        reqwest::StatusCode::OK,
        "creating a browser instance through the real generic API must succeed"
    );
    let body: Value = create.json().await.unwrap();
    let instance_id = body["instance_id"].as_str().unwrap().to_owned();

    let mut running = false;
    let mut last_state = String::new();
    for _ in 0..30 {
        let status = http(
            &base,
            Method::GET,
            &format!("/api/v1/runtime-instances/browser/{instance_id}"),
            Some(&admin_cookie),
            None,
        )
        .await;
        let status_body: Value = status.json().await.unwrap();
        last_state = status_body["state"].as_str().unwrap_or_default().to_owned();
        if last_state == "running" {
            running = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    assert!(
        running,
        "browser instance never reached Running (real process alive + real CDP health check passing); last state: {last_state}"
    );

    // Find the real container clouddeskd started, to prove real
    // teardown after stop (not merely a state-flag flip).
    let ps = TokioCommand::new("docker")
        .args([
            "ps",
            "-a",
            "-q",
            "--filter",
            &format!("ancestor={BROWSER_IMAGE}"),
        ])
        .output()
        .await
        .unwrap();
    let container_id = String::from_utf8_lossy(&ps.stdout)
        .lines()
        .next()
        .unwrap_or_default()
        .to_owned();
    assert!(
        !container_id.is_empty(),
        "expected a real Brave container to be running"
    );

    let stop = http(
        &base,
        Method::POST,
        &format!("/api/v1/runtime-instances/browser/{instance_id}/stop"),
        Some(&admin_cookie),
        None,
    )
    .await;
    assert_eq!(stop.status(), reqwest::StatusCode::NO_CONTENT);

    let mut container_gone = false;
    for _ in 0..30 {
        let inspect = TokioCommand::new("docker")
            .args(["inspect", &container_id])
            .output()
            .await
            .unwrap();
        if !inspect.status.success() || inspect.stdout.is_empty() {
            container_gone = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    }
    assert!(
        container_gone,
        "the real Brave container must be gone after stop, not merely marked stopped"
    );
}
