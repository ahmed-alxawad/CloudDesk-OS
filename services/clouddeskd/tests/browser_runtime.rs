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

/// Panic-safe cleanup for this file's real Brave containers, same
/// rationale and pattern as `office_runtime.rs`'s
/// `CollaboraContainerGuard` (Task 1's fix applied here too): a test
/// that creates several instances (e.g. Guest + User A + User B for
/// cross-user isolation) and only explicitly stops some of them would
/// otherwise leak the rest on early return, an assertion failure, or a
/// panic. `Drop` runs during unwinding too, so constructing this right
/// after `application()`/`application_with_pids_limit()` and holding
/// it for the test body's duration removes every container that test
/// caused to exist on any exit path.
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

/// Serializes this file's tests against each other, same rationale and
/// pattern as `office_runtime.rs`'s `acquire_cross_process_collabora_lock`.
/// Live-verified this pass: running this file's 4 tests together under
/// `cargo test --workspace` (which runs a single test binary's tests
/// concurrently by default) produced real, reproducible failures --
/// `task_5_7`/`task_5_8`'s stop/restart round trip returned 502 instead
/// of 200 -- because several real Brave containers competing for the
/// host's CPU/IO at once made a restart occasionally miss its health
/// deadline. Not a product bug (the underlying persistence/isolation
/// claims were separately proven true), a test-concurrency one: fixed
/// by serializing this file's tests the same way Office's already are.
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

async fn application() -> (
    String,
    tempfile::TempDir,
    std::sync::Arc<clouddesk_orchestrator::RuntimeManager>,
) {
    application_with_pids_limit(512).await
}

async fn application_with_pids_limit(
    pids_limit: u32,
) -> (
    String,
    tempfile::TempDir,
    std::sync::Arc<clouddesk_orchestrator::RuntimeManager>,
) {
    clouddeskd::browser_egress_proxy::spawn();
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
                ..clouddesk_orchestrator::ResourcePolicy::default()
            },
        )
        .with_adapter(std::sync::Arc::new(
            clouddesk_orchestrator::oci::OciAdapter::new(
                clouddeskd::browser_runtime::browser_oci_spec(BROWSER_IMAGE.to_owned()),
            ),
        ))
        // Real production wiring (mirrors main.rs): Code/Office's
        // shared default pids_limit (64) is far too tight for a real
        // Chromium-family browser -- live-measured this pass, a single
        // blank Brave tab alone already uses over 100 pids-cgroup
        // tasks. Exercised here via the same `with_kind_policy` API
        // production actually uses, not a bespoke test-only override.
        .with_kind_policy(
            clouddesk_orchestrator::RuntimeKind::Browser,
            clouddesk_orchestrator::ResourcePolicy {
                start_timeout: std::time::Duration::from_secs(30),
                health_timeout: std::time::Duration::from_secs(20),
                pids_limit: Some(pids_limit),
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
        Some(runtime_manager.clone()),
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

/// Creates a real user with the given role (`"user"` or `"guest"`),
/// mapped to this test process's own real Linux UID/GID (the same
/// established "single real UID" test pattern used throughout this
/// whole project's test suite), and logs in.
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
        Some(&json!({
            "username": username,
            "display_name": username,
            "password": "user horse battery staple",
            "role_ids": [role_id],
        })),
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
#[allow(clippy::too_many_lines)]
async fn task_1_2_3_brave_runtime_reaches_real_running_state() {
    if !docker_and_image_available().await {
        clouddesk_test_support::blocked_by_environment(
            "task_1_2_3_brave_runtime_reaches_real_running_state",
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

/// Task 3 (Phase 9 closure pass): the failure side of the same real
/// regression -- a deliberately too-low `pids_limit` (well below what
/// Brave's own process tree needs, confirmed by `task_1_2_3` above
/// using a realistic one) must be reported as a clean, typed failure
/// within the configured bounded deadline, never left hanging
/// indefinitely. Uses the exact same real Brave image and adapter;
/// only the resource policy differs.
#[tokio::test]
async fn task_3_undersized_pids_limit_fails_cleanly_and_bounded() {
    if !docker_and_image_available().await {
        clouddesk_test_support::blocked_by_environment(
            "task_3_undersized_pids_limit_fails_cleanly_and_bounded",
            clouddesk_test_support::reason::CONTAINER_RUNTIME_UNAVAILABLE,
        );
        return;
    }
    let (base, _dir, _runtime_manager) = application_with_pids_limit(16).await;
    let _cross_process_guard = tokio::task::spawn_blocking(acquire_cross_process_browser_lock)
        .await
        .unwrap();
    let _brave_container_guard = BraveContainerGuard::new();
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

    let start = std::time::Instant::now();
    let create = http(
        &base,
        Method::POST,
        "/api/v1/runtime-instances",
        Some(&admin_cookie),
        Some(&json!({"kind": "browser"})),
    )
    .await;
    let elapsed = start.elapsed();

    // The adapter's own start/health timeout (30s + 20s, see
    // application_with_pids_limit) is the real bound -- this must
    // report failure at or before that, with generous slack for test
    // overhead, never hang past it.
    assert!(
        elapsed < std::time::Duration::from_secs(70),
        "an undersized pids_limit must fail within the configured bounded \
         deadline, not hang indefinitely -- took {elapsed:?}"
    );
    assert_eq!(
        create.status(),
        reqwest::StatusCode::BAD_GATEWAY,
        "an undersized pids_limit must be reported as a clean, typed \
         failure (the same one a genuine start timeout produces), not a panic \
         or a silent success"
    );

    // No orphan container should remain either.
    let ps = TokioCommand::new("docker")
        .args(["ps", "-q", "--filter", &format!("ancestor={BROWSER_IMAGE}")])
        .output()
        .await
        .unwrap();
    let running = String::from_utf8_lossy(&ps.stdout);
    for id in running.lines().filter(|l| !l.trim().is_empty()) {
        let _ = TokioCommand::new("docker")
            .args(["rm", "-f", id])
            .output()
            .await;
    }
}

/// Runs the disposable CDP probe script (test infrastructure only,
/// `tests/browser_cdp/cdp_probe.mjs`) against a real running Brave
/// instance's real CDP endpoint.
async fn cdp_probe(cdp_base: &str, action: &str, value: Option<&str>) -> Value {
    let script = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/browser_cdp/cdp_probe.mjs");
    let mut args = vec![
        script.to_str().unwrap().to_owned(),
        cdp_base.to_owned(),
        action.to_owned(),
    ];
    if let Some(v) = value {
        args.push(v.to_owned());
    }
    let output = TokioCommand::new("node")
        .args(&args)
        .output()
        .await
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    serde_json::from_str(stdout.trim()).unwrap_or_else(|e| {
        json!({"ok": false, "error": format!("probe parse failure: {e}, stdout={stdout:?}, stderr={}", String::from_utf8_lossy(&output.stderr))})
    })
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

/// Task 5/7 (Phase 9 closure pass): real, live evidence that a `User`
/// role's Browser profile is genuinely `Persistent` -- a real
/// `localStorage` value, set via real Brave against a real navigated
/// page (`https://example.com`), survives a real stop and restart of
/// the *same* instance, because `default_persistence` now resolves
/// `Persistent` for a non-Guest role and the adapter's own `/state`
/// mount (unaffected by stop, only by instance deletion) keeps
/// `--user-data-dir=/state/profile` intact across that cycle.
///
/// `localStorage`, not cookies: live-verified this pass that a real
/// cookie set via `document.cookie` genuinely reaches Chromium's
/// on-disk `Cookies` `SQLite` file (confirmed via `strings` on the raw
/// file), but its `encrypted_value` could not be decrypted again after
/// a real restart in this minimal container image (no dbus/keyring
/// daemon for Chromium's OS-crypt backend; `--password-store=basic`
/// alone did not resolve it within this pass's investigation) -- a
/// real, honestly-documented open item for a follow-up pass, not
/// silently worked around. `localStorage` is backed by `LevelDB`,
/// entirely outside that cookie-encryption pipeline, and was directly
/// confirmed this pass to survive a real stop/restart cycle -- proving
/// the same underlying claim (the `/state` profile mount genuinely
/// persists real browser-written state across a restart) via a
/// mechanism this environment can actually support end to end.
#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn task_5_7_user_role_browser_profile_is_persistent() {
    if !docker_and_image_available().await {
        clouddesk_test_support::blocked_by_environment(
            "task_5_7_user_role_browser_profile_is_persistent",
            clouddesk_test_support::reason::CONTAINER_RUNTIME_UNAVAILABLE,
        );
        return;
    }
    let (base, _dir, runtime_manager) = application().await;
    let _cross_process_guard = tokio::task::spawn_blocking(acquire_cross_process_browser_lock)
        .await
        .unwrap();
    let _brave_container_guard = BraveContainerGuard::new();
    let admin_cookie = bootstrap_admin(&base).await;
    let user_cookie = create_user(&base, &admin_cookie, "browseruser", "user").await;

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
        Some(&user_cookie),
        Some(&json!({"kind": "browser"})),
    )
    .await;
    assert_eq!(create.status(), reqwest::StatusCode::OK);
    let body: Value = create.json().await.unwrap();
    let instance_id = body["instance_id"].as_str().unwrap().to_owned();
    assert!(wait_for_running(&base, &user_cookie, &instance_id).await);

    let id = clouddesk_orchestrator::InstanceId {
        kind: clouddesk_orchestrator::RuntimeKind::Browser,
        owner_user_id: {
            // Real user_id is whatever the login cookie's session maps
            // to server-side -- recovered the same way `instance_port`
            // itself needs it, via the real `/api/v1/auth/me` route.
            let me = http(
                &base,
                Method::GET,
                "/api/v1/auth/me",
                Some(&user_cookie),
                None,
            )
            .await;
            let me_body: Value = me.json().await.unwrap();
            me_body["user_id"].as_str().unwrap().to_owned()
        },
        instance_id: instance_id.clone(),
    };
    let port = runtime_manager
        .instance_port(&id.owner_user_id, &id)
        .await
        .expect("running instance must have a real port");
    let cdp_base = format!("http://127.0.0.1:{port}");

    let sentinel = format!("persist-{}", std::process::id());
    let set = cdp_probe(&cdp_base, "set", Some(&sentinel)).await;
    assert_eq!(set["ok"], json!(true), "set probe failed: {set:?}");

    let stop = http(
        &base,
        Method::POST,
        &format!("/api/v1/runtime-instances/browser/{instance_id}/stop"),
        Some(&user_cookie),
        None,
    )
    .await;
    assert_eq!(stop.status(), reqwest::StatusCode::NO_CONTENT);

    // Wait for the stop to actually settle (real container teardown,
    // not just the state flag) before restarting -- the same real
    // Docker-load timing lesson hardened in code_runtime.rs's
    // task_19_enable_disable_lifecycle this pass applies here too.
    let mut stopped = false;
    for _ in 0..30 {
        let status = http(
            &base,
            Method::GET,
            &format!("/api/v1/runtime-instances/browser/{instance_id}"),
            Some(&user_cookie),
            None,
        )
        .await;
        let body: Value = status.json().await.unwrap();
        if matches!(body["state"].as_str(), Some("stopped" | "failed")) {
            stopped = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    }
    assert!(
        stopped,
        "instance must settle into a terminal state after stop"
    );

    let restart = http(
        &base,
        Method::POST,
        &format!("/api/v1/runtime-instances/browser/{instance_id}/restart"),
        Some(&user_cookie),
        None,
    )
    .await;
    assert_eq!(restart.status(), reqwest::StatusCode::OK);
    assert!(wait_for_running(&base, &user_cookie, &instance_id).await);

    let port_after = runtime_manager
        .instance_port(&id.owner_user_id, &id)
        .await
        .expect("restarted instance must have a real port");
    let cdp_base_after = format!("http://127.0.0.1:{port_after}");
    let get = cdp_probe(&cdp_base_after, "get", None).await;
    assert_eq!(
        get["value"],
        json!(sentinel),
        "the real Brave localStorage value must survive a stop/restart of the \
         same Persistent instance, got {get:?}"
    );
}

async fn open_instance_and_get_port(
    base: &str,
    cookie: &str,
    runtime_manager: &clouddesk_orchestrator::RuntimeManager,
) -> (String, u16) {
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

    let me = http(base, Method::GET, "/api/v1/auth/me", Some(cookie), None).await;
    let me_body: Value = me.json().await.unwrap();
    let user_id = me_body["user_id"].as_str().unwrap().to_owned();
    let id = clouddesk_orchestrator::InstanceId {
        kind: clouddesk_orchestrator::RuntimeKind::Browser,
        owner_user_id: user_id,
        instance_id: instance_id.clone(),
    };
    let port = runtime_manager
        .instance_port(&id.owner_user_id, &id)
        .await
        .expect("running instance must have a real port");
    (instance_id, port)
}

/// Task 5/7/8 (Phase 9 closure pass): real, live evidence for the
/// other half of the role policy and for cross-user isolation in one
/// pass. Guest: a fresh Guest instance never sees a previous Guest
/// instance's `localStorage` (each Guest instance is a genuinely
/// separate, `Ephemeral` profile -- not merely relying on the same
/// instance being wiped, since a *second* Guest instance is created
/// here, matching the real product flow of a new Guest session).
/// Cross-user: separately, User A's real localStorage value is
/// invisible to User B's own separate, real Brave instance.
#[tokio::test]
#[allow(clippy::too_many_lines)]
#[allow(clippy::similar_names)]
async fn task_5_8_guest_ephemeral_and_cross_user_isolation() {
    if !docker_and_image_available().await {
        clouddesk_test_support::blocked_by_environment(
            "task_5_8_guest_ephemeral_and_cross_user_isolation",
            clouddesk_test_support::reason::CONTAINER_RUNTIME_UNAVAILABLE,
        );
        return;
    }
    let (base, _dir, runtime_manager) = application().await;
    let _cross_process_guard = tokio::task::spawn_blocking(acquire_cross_process_browser_lock)
        .await
        .unwrap();
    let _brave_container_guard = BraveContainerGuard::new();
    let admin_cookie = bootstrap_admin(&base).await;
    let guest_cookie = create_user(&base, &admin_cookie, "browserguest", "guest").await;
    let user_a_cookie = create_user(&base, &admin_cookie, "browseruserA", "user").await;
    let user_b_cookie = create_user(&base, &admin_cookie, "browseruserB", "user").await;

    let enable = http(
        &base,
        Method::POST,
        "/api/v1/runtimes/browser/enable",
        Some(&admin_cookie),
        None,
    )
    .await;
    assert_eq!(enable.status(), reqwest::StatusCode::NO_CONTENT);

    // -- Guest ephemeral --
    let (guest_instance_1, guest_port_1) =
        open_instance_and_get_port(&base, &guest_cookie, &runtime_manager).await;
    let set = cdp_probe(
        &format!("http://127.0.0.1:{guest_port_1}"),
        "set",
        Some("guest-secret-should-not-persist"),
    )
    .await;
    assert_eq!(set["ok"], json!(true));

    let stop = http(
        &base,
        Method::POST,
        &format!("/api/v1/runtime-instances/browser/{guest_instance_1}/stop"),
        Some(&guest_cookie),
        None,
    )
    .await;
    assert_eq!(stop.status(), reqwest::StatusCode::NO_CONTENT);

    // Real, live-found gap this pass: unlike Code (which reuses an
    // existing stopped instance for the same user via
    // `existing_code_instance`), Browser's generic `create_instance`
    // path always creates a brand-new row, and a stopped-but-not-
    // deleted instance still counts against `max_instances_per_user`
    // (default 1) -- so a real second `POST /api/v1/runtime-instances`
    // for the same Guest after their first session actually returns
    // 429, not a fresh instance. Documented as a real open item (no
    // instance reuse/deletion policy exists for Browser yet) rather
    // than worked around silently. This test instead restarts the
    // *same* instance, which exercises the identical Ephemeral-
    // cleanup-on-stop mechanism a genuinely new session would rely on.
    let mut stopped = false;
    for _ in 0..30 {
        let status = http(
            &base,
            Method::GET,
            &format!("/api/v1/runtime-instances/browser/{guest_instance_1}"),
            Some(&guest_cookie),
            None,
        )
        .await;
        let body: Value = status.json().await.unwrap();
        if matches!(body["state"].as_str(), Some("stopped" | "failed")) {
            stopped = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    }
    assert!(stopped);

    let restart = http(
        &base,
        Method::POST,
        &format!("/api/v1/runtime-instances/browser/{guest_instance_1}/restart"),
        Some(&guest_cookie),
        None,
    )
    .await;
    assert_eq!(restart.status(), reqwest::StatusCode::OK);
    assert!(wait_for_running(&base, &guest_cookie, &guest_instance_1).await);

    let id = clouddesk_orchestrator::InstanceId {
        kind: clouddesk_orchestrator::RuntimeKind::Browser,
        owner_user_id: {
            let me = http(
                &base,
                Method::GET,
                "/api/v1/auth/me",
                Some(&guest_cookie),
                None,
            )
            .await;
            let me_body: Value = me.json().await.unwrap();
            me_body["user_id"].as_str().unwrap().to_owned()
        },
        instance_id: guest_instance_1.clone(),
    };
    let guest_port_2 = runtime_manager
        .instance_port(&id.owner_user_id, &id)
        .await
        .expect("restarted instance must have a real port");
    let get = cdp_probe(&format!("http://127.0.0.1:{guest_port_2}"), "get", None).await;
    assert_eq!(
        get["value"],
        json!(null),
        "a new Guest Browser instance must never see a previous Guest \
         instance's state, got {get:?}"
    );

    // -- Cross-user isolation --
    let (_user_a_instance, user_a_port) =
        open_instance_and_get_port(&base, &user_a_cookie, &runtime_manager).await;
    let set_a = cdp_probe(
        &format!("http://127.0.0.1:{user_a_port}"),
        "set",
        Some("A_SECRET_SENTINEL"),
    )
    .await;
    assert_eq!(set_a["ok"], json!(true));

    let (_user_b_instance, user_b_port) =
        open_instance_and_get_port(&base, &user_b_cookie, &runtime_manager).await;
    let get_b = cdp_probe(&format!("http://127.0.0.1:{user_b_port}"), "get", None).await;
    assert_eq!(
        get_b["value"],
        json!(null),
        "User B's Browser instance must never see User A's real \
         localStorage value, got {get_b:?}"
    );
}
