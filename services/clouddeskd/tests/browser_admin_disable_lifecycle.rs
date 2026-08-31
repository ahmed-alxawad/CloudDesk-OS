//! Phase 9 Pass 3B-3, Tasks 7/8: closes the two remaining evidence
//! gaps a prior report left as "relies on previous evidence" rather
//! than a fresh PASS -- Guest cleanup specifically triggered by a real
//! Administrator disable (not a user-initiated stop, and not merely
//! Guest-restart-without-disable), and persistent-profile retention
//! specifically across a real admin disable/re-enable cycle (not only
//! across a plain container restart). Uses the same real CDP-probe
//! `localStorage` sentinel mechanism already established and trusted
//! in `browser_runtime.rs`.

use axum::http::Method;
use clouddesk_auth::{AuthPolicy, AuthService};
use clouddesk_secrets::SecretCipher;
use serde_json::{json, Value};
use tokio::process::Command as TokioCommand;

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
        SecretCipher::new(&[131_u8; 32]).unwrap(),
        AuthPolicy::default(),
    )
    .unwrap();
    let directory = tempfile::tempdir().unwrap();
    let secret_path = directory.path().join("bootstrap.secret");
    std::fs::write(&secret_path, "browser-disable-lifecycle-test-secret\n").unwrap();

    let runtime_manager = std::sync::Arc::new(
        clouddesk_orchestrator::RuntimeManager::new(
            clouddesk_orchestrator::store::RuntimeStore::new(pool.clone()),
            std::env::temp_dir().join(format!(
                "clouddesk-browser-disable-lifecycle-test-{}",
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
            "secret": "browser-disable-lifecycle-test-secret",
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

async fn disable_browser(base: &str, admin_cookie: &str) {
    let disable = http(
        base,
        Method::POST,
        "/api/v1/runtimes/browser/disable",
        Some(admin_cookie),
        None,
    )
    .await;
    assert_eq!(disable.status(), reqwest::StatusCode::NO_CONTENT);
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

async fn wait_for_stopped(base: &str, cookie: &str, instance_id: &str) -> bool {
    for _ in 0..30 {
        let status = http(
            base,
            Method::GET,
            &format!("/api/v1/runtime-instances/browser/{instance_id}"),
            Some(cookie),
            None,
        )
        .await;
        let body: Value = status.json().await.unwrap();
        if matches!(
            body["state"].as_str(),
            Some("stopped" | "failed" | "unavailable")
        ) {
            return true;
        }
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    }
    false
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
    let port = instance_port_for(base, cookie, runtime_manager, &instance_id).await;
    (instance_id, port)
}

async fn instance_port_for(
    base: &str,
    cookie: &str,
    runtime_manager: &clouddesk_orchestrator::RuntimeManager,
    instance_id: &str,
) -> u16 {
    let me = http(base, Method::GET, "/api/v1/auth/me", Some(cookie), None).await;
    let me_body: Value = me.json().await.unwrap();
    let user_id = me_body["user_id"].as_str().unwrap().to_owned();
    let id = clouddesk_orchestrator::InstanceId {
        kind: clouddesk_orchestrator::RuntimeKind::Browser,
        owner_user_id: user_id.clone(),
        instance_id: instance_id.to_owned(),
    };
    runtime_manager
        .instance_port(&user_id, &id)
        .await
        .expect("running instance must have a real port")
}

/// Runs the disposable CDP probe script (test infrastructure only)
/// against a real running Brave instance's real CDP endpoint -- same
/// established mechanism `browser_runtime.rs` already uses for
/// profile-persistence/ephemeral evidence.
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

/// Task 7: Guest cleanup specifically triggered by a real
/// Administrator disable, not a user-initiated stop. A real Guest
/// sets real `localStorage` state, a real Administrator disables
/// Browser (the container is torn down as a side effect), then
/// re-enables it; the Guest's restarted instance (Browser has no
/// instance-reuse-on-create for a genuinely new session, a
/// pre-existing documented gap, so this restarts the same instance --
/// exactly the mechanism a fresh Guest session would rely on) must
/// never see the pre-disable state.
#[tokio::test]
async fn task_7_guest_cleanup_on_admin_disable() {
    if !docker_and_image_available().await {
        clouddesk_test_support::blocked_by_environment(
            "task_7_guest_cleanup_on_admin_disable",
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
    enable_browser(&base, &admin_cookie).await;
    let guest_cookie = create_user(&base, &admin_cookie, "disableguest", "guest").await;

    let (guest_instance, guest_port) =
        open_instance_and_get_port(&base, &guest_cookie, &runtime_manager).await;
    let set = cdp_probe(
        &format!("http://127.0.0.1:{guest_port}"),
        "set",
        Some("guest-state-must-not-survive-admin-disable"),
    )
    .await;
    assert_eq!(
        set["ok"],
        json!(true),
        "seeding real Guest state must succeed: {set:?}"
    );

    disable_browser(&base, &admin_cookie).await;
    assert!(
        wait_for_stopped(&base, &guest_cookie, &guest_instance).await,
        "the Guest runtime instance must stop when Browser is disabled"
    );

    let mut containers_gone = false;
    for _ in 0..30 {
        if list_brave_container_ids().is_empty() {
            containers_gone = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    }
    assert!(
        containers_gone,
        "the Guest's real Brave container must be fully removed after admin disable"
    );

    // New sessions denied while disabled.
    let denied = http(
        &base,
        Method::POST,
        "/api/v1/runtime-instances",
        Some(&guest_cookie),
        Some(&json!({"kind": "browser"})),
    )
    .await;
    assert_ne!(
        denied.status(),
        reqwest::StatusCode::OK,
        "a new Guest Browser session must be denied while disabled"
    );

    enable_browser(&base, &admin_cookie).await;
    let restart = http(
        &base,
        Method::POST,
        &format!("/api/v1/runtime-instances/browser/{guest_instance}/restart"),
        Some(&guest_cookie),
        None,
    )
    .await;
    assert_eq!(
        restart.status(),
        reqwest::StatusCode::OK,
        "the Guest instance must be usable again after re-enabling"
    );
    assert!(wait_for_running(&base, &guest_cookie, &guest_instance).await);

    let fresh_port =
        instance_port_for(&base, &guest_cookie, &runtime_manager, &guest_instance).await;
    let get = cdp_probe(&format!("http://127.0.0.1:{fresh_port}"), "get", None).await;
    assert_eq!(
        get["value"],
        json!(null),
        "GUEST CLEANUP ON DISABLE: the pre-disable Guest state must never survive into the fresh post-re-enable session, got {get:?}"
    );
}

/// Task 8: persistent User profile retention specifically across a
/// real admin disable/re-enable cycle -- distinct from a plain
/// container restart (already proven in `browser_runtime.rs`).
#[tokio::test]
async fn task_8_persistent_profile_retained_across_admin_disable() {
    if !docker_and_image_available().await {
        clouddesk_test_support::blocked_by_environment(
            "task_8_persistent_profile_retained_across_admin_disable",
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
    enable_browser(&base, &admin_cookie).await;
    let user_cookie = create_user(&base, &admin_cookie, "disablepersistuser", "user").await;

    let (instance_id, port) =
        open_instance_and_get_port(&base, &user_cookie, &runtime_manager).await;
    let sentinel = "persistent-profile-survives-admin-disable-2026";
    let set = cdp_probe(&format!("http://127.0.0.1:{port}"), "set", Some(sentinel)).await;
    assert_eq!(
        set["ok"],
        json!(true),
        "seeding the real sentinel must succeed: {set:?}"
    );

    disable_browser(&base, &admin_cookie).await;
    assert!(wait_for_stopped(&base, &user_cookie, &instance_id).await);

    enable_browser(&base, &admin_cookie).await;
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

    let fresh_port = instance_port_for(&base, &user_cookie, &runtime_manager, &instance_id).await;
    let get = cdp_probe(&format!("http://127.0.0.1:{fresh_port}"), "get", None).await;
    assert_eq!(
        get["value"],
        json!(sentinel),
        "PERSISTENT PROFILE RETAINED ACROSS DISABLE: the User's real localStorage sentinel must survive a real admin disable/re-enable cycle, got {get:?}"
    );
}
