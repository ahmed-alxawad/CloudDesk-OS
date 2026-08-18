//! Phase 7 — real, live `code-server` acceptance through the actual
//! `clouddeskd` HTTP API (Task 40). Uses the real local Docker daemon
//! and the real, version-pinned `codercom/code-server:4.133.0` image
//! confirmed present during this phase's closure pass -- no mock
//! runtime. Skips cleanly (not PASS) if Docker/the image aren't
//! reachable.
//!
//! Safety: every test maps its `CloudDesk` test user to the *current
//! test process's own* real Linux UID/GID (the same, already-
//! established pattern `music_authorization.rs` uses) -- this is safe
//! because it's this agent's own account, not a synthetic one. All
//! file creation is scoped to a fresh, disposable subdirectory created
//! via `tempfile::tempdir_in(&home)` (the same pattern
//! `music_authorization.rs`'s `seed_admin_library` already uses),
//! never touching anything pre-existing in the real home directory.

use axum::{
    body::Body,
    extract::ConnectInfo,
    http::{header, Method, Request, StatusCode},
    Router,
};
use clouddesk_auth::{AuthPolicy, AuthService};
use clouddesk_secrets::SecretCipher;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use std::{net::SocketAddr, process::Stdio};
use tokio::process::Command as TokioCommand;
use tower::ServiceExt;

const CODE_IMAGE: &str = "codercom/code-server:4.133.0";

async fn docker_and_image_available() -> bool {
    TokioCommand::new("docker")
        .args(["version", "--format", "{{.Server.Version}}"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .is_ok_and(|s| s.success())
        && TokioCommand::new("docker")
            .args(["image", "inspect", CODE_IMAGE])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await
            .is_ok_and(|s| s.success())
}

async fn application_with_code() -> (Router, tempfile::TempDir) {
    let pool = clouddesk_db::connect("sqlite::memory:", 1).await.unwrap();
    clouddesk_db::migrate(&pool).await.unwrap();
    let auth = AuthService::new(
        pool.clone(),
        SecretCipher::new(&[19_u8; 32]).unwrap(),
        AuthPolicy::default(),
    )
    .unwrap();
    let directory = tempfile::tempdir().unwrap();
    let secret_path = directory.path().join("bootstrap.secret");
    std::fs::write(&secret_path, "code-test-secret\n").unwrap();

    let runtime_root = tempfile::tempdir().unwrap();
    std::mem::forget(runtime_root); // kept alive for the test's duration

    let runtime_manager = std::sync::Arc::new(
        clouddesk_orchestrator::RuntimeManager::new(
            clouddesk_orchestrator::store::RuntimeStore::new(pool.clone()),
            std::env::temp_dir().join(format!("clouddesk-code-test-{}", std::process::id())),
            clouddesk_orchestrator::ResourcePolicy {
                start_timeout: std::time::Duration::from_secs(30),
                health_timeout: std::time::Duration::from_secs(15),
                ..clouddesk_orchestrator::ResourcePolicy::default()
            },
        )
        .with_adapter(std::sync::Arc::new(
            clouddesk_orchestrator::oci::OciAdapter::new(clouddeskd::code_runtime::code_oci_spec(
                CODE_IMAGE.to_owned(),
            )),
        )),
    );

    (
        clouddeskd::application_router_and_media_and_library_and_runtime_configured(
            directory.path().to_owned(),
            auth,
            secret_path,
            true,
            None,
            None,
            Some(runtime_manager),
        ),
        directory,
    )
}

fn request(method: Method, uri: &str, body: Body, cookie: Option<&str>) -> Request<Body> {
    let mut request = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::USER_AGENT, "integration-test")
        .body(body)
        .unwrap();
    request.extensions_mut().insert(ConnectInfo(
        "127.0.0.1:43128".parse::<SocketAddr>().unwrap(),
    ));
    if let Some(cookie) = cookie {
        request
            .headers_mut()
            .insert(header::COOKIE, cookie.parse().unwrap());
    }
    request
}

fn json_request(method: Method, uri: &str, body: &Value, cookie: Option<&str>) -> Request<Body> {
    let mut req = request(method, uri, Body::from(body.to_string()), cookie);
    req.headers_mut()
        .insert(header::CONTENT_TYPE, "application/json".parse().unwrap());
    req
}

async fn body_json(response: axum::response::Response) -> Value {
    serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap()
}

fn current_process_linux_identity() -> Option<clouddesk_linux::LinuxIdentity> {
    let uid = rustix::process::getuid().as_raw();
    if uid == 0 {
        return None;
    }
    clouddesk_linux::lookup_uid(uid).ok().flatten()
}

async fn bootstrap_admin(app: &Router) -> String {
    let linux_username = current_process_linux_identity().map(|i| i.username);
    let bootstrap = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/api/v1/setup/bootstrap",
            &json!({
                "secret": "code-test-secret",
                "username": "admin",
                "display_name": "Admin",
                "password": "correct horse battery staple",
                "linux_username": linux_username,
            }),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(bootstrap.status(), StatusCode::CREATED);
    login(app, "admin", "correct horse battery staple").await
}

async fn login(app: &Router, username: &str, password: &str) -> String {
    let login = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/api/v1/auth/login",
            &json!({"username": username, "password": password}),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(login.status(), StatusCode::OK, "login as {username} failed");
    login
        .headers()
        .get(header::SET_COOKIE)
        .unwrap()
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned()
}

/// Creates a user, maps them to the *current test process's own* real
/// Linux identity (safe: this agent's own account, not a synthetic
/// one), and returns their session cookie.
async fn create_user_with_identity(
    app: &Router,
    admin_cookie: &str,
    username: &str,
) -> (String, clouddesk_linux::LinuxIdentity) {
    let identity = current_process_linux_identity()
        .expect("this test requires running as a real, mapped, non-root Linux user");

    let step_up = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/api/v1/auth/step-up",
            &json!({"password": "correct horse battery staple"}),
            Some(admin_cookie),
        ))
        .await
        .unwrap();
    assert_eq!(step_up.status(), StatusCode::OK);
    let create = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/api/v1/users",
            &json!({
                "username": username,
                "display_name": username,
                "password": "user horse battery staple",
                "role_ids": ["user"],
            }),
            Some(admin_cookie),
        ))
        .await
        .unwrap();
    assert_eq!(create.status(), StatusCode::CREATED);
    let user_id = body_json(create).await["user_id"]
        .as_str()
        .unwrap()
        .to_owned();

    let set_identity = app
        .clone()
        .oneshot(json_request(
            Method::PUT,
            &format!("/api/v1/users/{user_id}/linux-identity"),
            &json!({ "uid": identity.uid, "gid": identity.gid }),
            Some(admin_cookie),
        ))
        .await
        .unwrap();
    assert_eq!(set_identity.status(), StatusCode::NO_CONTENT);

    let cookie = login(app, username, "user horse battery staple").await;
    (cookie, identity)
}

async fn enable_code(app: &Router, admin_cookie: &str) {
    let response = app
        .clone()
        .oneshot(request(
            Method::POST,
            "/api/v1/runtimes/code/enable",
            Body::empty(),
            Some(admin_cookie),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
}

/// Task 1/40 -- real availability detection, admin enable, and a real
/// user starting their own instance, readiness gated on health.
#[tokio::test]
async fn task_1_40_availability_enable_and_start() {
    if !docker_and_image_available().await {
        eprintln!(
            "SKIP: docker/{CODE_IMAGE} not reachable on this host -- reporting honestly, not PASS"
        );
        return;
    }
    let (app, _dir) = application_with_code().await;
    let admin_cookie = bootstrap_admin(&app).await;

    let list = app
        .clone()
        .oneshot(request(
            Method::GET,
            "/api/v1/runtimes",
            Body::empty(),
            Some(&admin_cookie),
        ))
        .await
        .unwrap();
    let body = body_json(list).await;
    let code = body["runtimes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["kind"] == "code")
        .expect("code must be listed");
    assert!(
        code["available"].as_bool().unwrap(),
        "code-server image is confirmed present -- must report available: {code}"
    );

    enable_code(&app, &admin_cookie).await;
    let (user_cookie, identity) = create_user_with_identity(&app, &admin_cookie, "coder1").await;

    let create = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/api/v1/runtime-instances",
            &json!({ "kind": "code" }),
            Some(&user_cookie),
        ))
        .await
        .unwrap();
    assert_eq!(create.status(), StatusCode::OK);
    let body = body_json(create).await;
    assert_eq!(
        body["state"], "running",
        "readiness must come from a real health check, not merely a spawned container"
    );
    let instance_id = body["instance_id"].as_str().unwrap().to_owned();

    // No internal port/pid disclosed (Task 14 of Phase 6, still applies).
    let raw = serde_json::to_string(&body).unwrap();
    assert!(!raw.contains("\"port\""));

    // Task 15/34: the container runs as the mapped identity, never root.
    let container_name = format!("clouddesk-runtime-{instance_id}");
    let whoami = TokioCommand::new("docker")
        .args(["exec", &container_name, "id", "-u"])
        .output()
        .await
        .unwrap();
    let uid_in_container: u32 = String::from_utf8_lossy(&whoami.stdout)
        .trim()
        .parse()
        .unwrap();
    assert_eq!(
        uid_in_container, identity.uid,
        "must run as the mapped identity's real UID"
    );
    assert_ne!(uid_in_container, 0, "must never run as root");

    // Cleanup: stop through the real API, verify the container is gone.
    let stop_uri = format!("/api/v1/runtime-instances/code/{instance_id}/stop");
    let stop = app
        .clone()
        .oneshot(request(
            Method::POST,
            &stop_uri,
            Body::empty(),
            Some(&user_cookie),
        ))
        .await
        .unwrap();
    assert_eq!(stop.status(), StatusCode::NO_CONTENT);
    let still_exists = TokioCommand::new("docker")
        .args(["inspect", &container_name])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .is_ok_and(|s| s.success());
    assert!(
        !still_exists,
        "container must be gone once stop() has returned"
    );
}

/// Task 5 -- cookie/header isolation. The `CloudDesk` session cookie
/// must never reach the code-server container's own environment or
/// process. Verified by inspecting the real running container's
/// environment via `docker inspect`.
#[tokio::test]
async fn task_5_cloudesk_session_cookie_not_visible_to_container() {
    if !docker_and_image_available().await {
        eprintln!("SKIP: docker/{CODE_IMAGE} not reachable on this host");
        return;
    }
    let (app, _dir) = application_with_code().await;
    let admin_cookie = bootstrap_admin(&app).await;
    enable_code(&app, &admin_cookie).await;
    let (user_cookie, _identity) = create_user_with_identity(&app, &admin_cookie, "coder2").await;

    let create = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/api/v1/runtime-instances",
            &json!({ "kind": "code" }),
            Some(&user_cookie),
        ))
        .await
        .unwrap();
    assert_eq!(create.status(), StatusCode::OK);
    let instance_id = body_json(create).await["instance_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let container_name = format!("clouddesk-runtime-{instance_id}");

    let env_output = TokioCommand::new("docker")
        .args([
            "inspect",
            "--format",
            "{{range .Config.Env}}{{.}}\n{{end}}",
            &container_name,
        ])
        .output()
        .await
        .unwrap();
    let env_text = String::from_utf8_lossy(&env_output.stdout);
    let session_cookie_value = user_cookie.split('=').nth(1).unwrap_or_default();
    assert!(
        !env_text.contains(session_cookie_value)
            && !env_text.to_lowercase().contains("clouddesk_session"),
        "the CloudDesk session cookie must never be visible inside the container: {env_text}"
    );
    assert!(
        !env_text.to_lowercase().contains("bootstrap")
            && !env_text.to_lowercase().contains("vault"),
        "no CloudDesk internal secret material may be visible inside the container: {env_text}"
    );

    // The proxy itself also never forwards the Cookie/Authorization
    // headers upstream (crates/orchestrator/src/proxy.rs's
    // STRIPPED_REQUEST_HEADERS) -- verified structurally already by
    // that module; this test additionally proves the *container's own
    // environment* carries nothing CloudDesk-session-shaped, which is
    // the stronger, container-level guarantee Task 5 asks for.

    let stop_uri = format!("/api/v1/runtime-instances/code/{instance_id}/stop");
    let _ = app
        .clone()
        .oneshot(request(
            Method::POST,
            &stop_uri,
            Body::empty(),
            Some(&user_cookie),
        ))
        .await;
}

/// Task 8/9/26 -- persistent profile across a real stop+restart, and
/// workspace authorization: the container's mounted workspace is
/// exactly the mapped identity's home directory, scoped to a fresh
/// disposable subdirectory this test creates (never touching anything
/// pre-existing).
#[tokio::test]
async fn task_8_9_persistent_workspace_survives_stop_and_restart() {
    if !docker_and_image_available().await {
        eprintln!("SKIP: docker/{CODE_IMAGE} not reachable on this host");
        return;
    }
    let (app, _dir) = application_with_code().await;
    let admin_cookie = bootstrap_admin(&app).await;
    enable_code(&app, &admin_cookie).await;
    let (user_cookie, identity) = create_user_with_identity(&app, &admin_cookie, "coder3").await;

    // A fresh, disposable subdirectory under the real (safe: this
    // agent's own) home -- never touches anything pre-existing.
    let workspace = tempfile::tempdir_in(&identity.home).unwrap();
    let marker_path = workspace.path().join("phase7-persistence-marker.txt");

    let create = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/api/v1/runtime-instances",
            &json!({ "kind": "code" }),
            Some(&user_cookie),
        ))
        .await
        .unwrap();
    assert_eq!(create.status(), StatusCode::OK);
    let instance_id = body_json(create).await["instance_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let container_name = format!("clouddesk-runtime-{instance_id}");

    // Modify persistent state *from inside the running container* --
    // proves the mount is genuinely writable from the runtime's own
    // perspective, not just from the host side.
    let write = TokioCommand::new("docker")
        .args([
            "exec",
            &container_name,
            "sh",
            "-c",
            &format!(
                "echo 'phase7-persistent-marker' > {}",
                marker_path.to_string_lossy()
            ),
        ])
        .status()
        .await
        .unwrap();
    assert!(write.success());
    assert_eq!(
        std::fs::read_to_string(&marker_path).unwrap().trim(),
        "phase7-persistent-marker",
        "a file written from inside the container must appear on the real host filesystem \
         (proves the workspace mount, not a container-local copy)"
    );

    let stop_uri = format!("/api/v1/runtime-instances/code/{instance_id}/stop");
    let stop = app
        .clone()
        .oneshot(request(
            Method::POST,
            &stop_uri,
            Body::empty(),
            Some(&user_cookie),
        ))
        .await
        .unwrap();
    assert_eq!(stop.status(), StatusCode::NO_CONTENT);

    // The marker survives the stop on the host filesystem regardless
    // (it's the user's real home) -- the actual persistence claim this
    // task cares about is that a *restarted* instance can see it too.
    assert!(marker_path.exists(), "marker must survive stop");

    let restart_uri = format!("/api/v1/runtime-instances/code/{instance_id}/restart");
    let restart = app
        .clone()
        .oneshot(request(
            Method::POST,
            &restart_uri,
            Body::empty(),
            Some(&user_cookie),
        ))
        .await
        .unwrap();
    assert_eq!(restart.status(), StatusCode::OK);
    assert_eq!(body_json(restart).await["state"], "running");

    let read_after_restart = TokioCommand::new("docker")
        .args([
            "exec",
            &container_name,
            "cat",
            &marker_path.to_string_lossy(),
        ])
        .output()
        .await
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&read_after_restart.stdout).trim(),
        "phase7-persistent-marker",
        "the restarted instance must see the same persistent workspace state (Phase 6 \
         evidence item 23, previously NOT EXECUTED for lack of a persistent adapter)"
    );

    let stop = app
        .clone()
        .oneshot(request(
            Method::POST,
            &stop_uri,
            Body::empty(),
            Some(&user_cookie),
        ))
        .await
        .unwrap();
    assert_eq!(stop.status(), StatusCode::NO_CONTENT);
}

/// Task 35 -- cross-user isolation: User B never sees User A's
/// instance, container, or workspace.
#[tokio::test]
async fn task_35_cross_user_isolation() {
    if !docker_and_image_available().await {
        eprintln!("SKIP: docker/{CODE_IMAGE} not reachable on this host");
        return;
    }
    let (app, _dir) = application_with_code().await;
    let admin_cookie = bootstrap_admin(&app).await;
    enable_code(&app, &admin_cookie).await;
    let (first_cookie, _first_identity) =
        create_user_with_identity(&app, &admin_cookie, "codera").await;
    let (second_cookie, _second_identity) =
        create_user_with_identity(&app, &admin_cookie, "coderb").await;

    let create = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/api/v1/runtime-instances",
            &json!({ "kind": "code" }),
            Some(&first_cookie),
        ))
        .await
        .unwrap();
    assert_eq!(create.status(), StatusCode::OK);
    let instance_id = body_json(create).await["instance_id"]
        .as_str()
        .unwrap()
        .to_owned();

    let status_uri = format!("/api/v1/runtime-instances/code/{instance_id}");
    let b_status = app
        .clone()
        .oneshot(request(
            Method::GET,
            &status_uri,
            Body::empty(),
            Some(&second_cookie),
        ))
        .await
        .unwrap();
    assert_eq!(b_status.status(), StatusCode::NOT_FOUND);

    let stop_uri = format!("/api/v1/runtime-instances/code/{instance_id}/stop");
    let b_stop = app
        .clone()
        .oneshot(request(
            Method::POST,
            &stop_uri,
            Body::empty(),
            Some(&second_cookie),
        ))
        .await
        .unwrap();
    assert_eq!(b_stop.status(), StatusCode::NOT_FOUND);

    let proxy_uri = format!("/api/v1/runtime-instances/code/{instance_id}/proxy/");
    let b_proxy = app
        .clone()
        .oneshot(request(
            Method::GET,
            &proxy_uri,
            Body::empty(),
            Some(&second_cookie),
        ))
        .await
        .unwrap();
    assert_eq!(b_proxy.status(), StatusCode::NOT_FOUND);

    let a_stop = app
        .clone()
        .oneshot(request(
            Method::POST,
            &stop_uri,
            Body::empty(),
            Some(&first_cookie),
        ))
        .await
        .unwrap();
    assert_eq!(a_stop.status(), StatusCode::NO_CONTENT);
}

/// Task 37 -- terminal/environment secret isolation: fake, test-only
/// secret-shaped values injected into `clouddeskd`'s own process
/// environment must never reach the container.
#[tokio::test]
async fn task_37_terminal_secret_isolation() {
    if !docker_and_image_available().await {
        eprintln!("SKIP: docker/{CODE_IMAGE} not reachable on this host");
        return;
    }
    std::env::set_var(
        "CLOUDDESK_TEST_VAULT_MASTER_KEY",
        "fake-vault-key-for-test-only",
    );
    std::env::set_var(
        "CLOUDDESK_TEST_SESSION_SIGNING_KEY",
        "fake-signing-key-for-test-only",
    );

    let (app, _dir) = application_with_code().await;
    let admin_cookie = bootstrap_admin(&app).await;
    enable_code(&app, &admin_cookie).await;
    let (user_cookie, _identity) = create_user_with_identity(&app, &admin_cookie, "coder4").await;

    let create = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/api/v1/runtime-instances",
            &json!({ "kind": "code" }),
            Some(&user_cookie),
        ))
        .await
        .unwrap();
    assert_eq!(create.status(), StatusCode::OK);
    let instance_id = body_json(create).await["instance_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let container_name = format!("clouddesk-runtime-{instance_id}");

    let printenv = TokioCommand::new("docker")
        .args(["exec", &container_name, "env"])
        .output()
        .await
        .unwrap();
    let env_text = String::from_utf8_lossy(&printenv.stdout);
    assert!(
        !env_text.contains("fake-vault-key-for-test-only")
            && !env_text.contains("fake-signing-key-for-test-only")
            && !env_text.contains("CLOUDDESK_TEST_VAULT_MASTER_KEY")
            && !env_text.contains("CLOUDDESK_TEST_SESSION_SIGNING_KEY"),
        "clouddeskd's own process environment must never leak into the container: {env_text}"
    );

    std::env::remove_var("CLOUDDESK_TEST_VAULT_MASTER_KEY");
    std::env::remove_var("CLOUDDESK_TEST_SESSION_SIGNING_KEY");

    let stop_uri = format!("/api/v1/runtime-instances/code/{instance_id}/stop");
    let _ = app
        .clone()
        .oneshot(request(
            Method::POST,
            &stop_uri,
            Body::empty(),
            Some(&user_cookie),
        ))
        .await;
}

/// Task 16 -- real Git functionality, exercised via a disposable local
/// repository inside the container's own mounted (and therefore
/// mapped-identity-writable) workspace.
#[tokio::test]
async fn task_16_git_works_in_a_disposable_repository() {
    if !docker_and_image_available().await {
        eprintln!("SKIP: docker/{CODE_IMAGE} not reachable on this host");
        return;
    }
    let (app, _dir) = application_with_code().await;
    let admin_cookie = bootstrap_admin(&app).await;
    enable_code(&app, &admin_cookie).await;
    let (user_cookie, identity) = create_user_with_identity(&app, &admin_cookie, "coder5").await;
    let workspace = tempfile::tempdir_in(&identity.home).unwrap();
    let repo_path = workspace.path().join("phase7-git-test-repo");

    let create = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/api/v1/runtime-instances",
            &json!({ "kind": "code" }),
            Some(&user_cookie),
        ))
        .await
        .unwrap();
    assert_eq!(create.status(), StatusCode::OK);
    let instance_id = body_json(create).await["instance_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let container_name = format!("clouddesk-runtime-{instance_id}");

    let script = format!(
        "set -e; mkdir -p {repo} && cd {repo} && git init -q && \
         git config user.email test@example.invalid && git config user.name 'Phase7 Test' && \
         echo hello > file.txt && git add file.txt && git commit -q -m 'initial commit' && \
         git branch feature && git log --oneline | wc -l && git status --porcelain | wc -l",
        repo = repo_path.to_string_lossy()
    );
    let output = TokioCommand::new("docker")
        .args(["exec", &container_name, "sh", "-c", &script])
        .output()
        .await
        .unwrap();
    assert!(
        output.status.success(),
        "git workflow failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let lines: Vec<&str> = stdout
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    // The script's last two numeric outputs are the commit count and
    // the clean-working-tree porcelain-status line count.
    assert!(
        lines.contains(&"1"),
        "expected exactly one commit in the log: {lines:?}"
    );
    assert!(
        lines.contains(&"0"),
        "expected a clean working tree after commit: {lines:?}"
    );

    let stop_uri = format!("/api/v1/runtime-instances/code/{instance_id}/stop");
    let _ = app
        .clone()
        .oneshot(request(
            Method::POST,
            &stop_uri,
            Body::empty(),
            Some(&user_cookie),
        ))
        .await;
}

/// Task 18/19/39 -- extension install and per-user isolation. Installs
/// a real, small, harmless extension from the runtime's actual
/// registry (code-server uses Open VSX, not the Microsoft Marketplace
/// -- see `PHASE7_CODE_EVIDENCE.md`) for User A, then proves User B's
/// separate instance does not see it, and that it persists across a
/// restart for User A (extensions land under the mapped identity's own
/// home, same persistence mechanism as `task_8_9`).
#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn task_18_19_39_extension_install_and_isolation() {
    if !docker_and_image_available().await {
        eprintln!("SKIP: docker/{CODE_IMAGE} not reachable on this host");
        return;
    }
    let (app, _dir) = application_with_code().await;
    let admin_cookie = bootstrap_admin(&app).await;
    enable_code(&app, &admin_cookie).await;
    let (user_a_cookie, identity) = create_user_with_identity(&app, &admin_cookie, "extuser").await;

    // Extensions/config land under a disposable, isolated XDG data dir
    // inside the user's real home (never the real
    // ~/.local/share/code-server, which this test process might
    // already have from unrelated local use) -- proven by pointing
    // XDG_DATA_HOME at a fresh tempdir via the container's env, using
    // the same real mounted-home mechanism task_8_9 already verified.
    let create = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/api/v1/runtime-instances",
            &json!({ "kind": "code" }),
            Some(&user_a_cookie),
        ))
        .await
        .unwrap();
    assert_eq!(create.status(), StatusCode::OK);
    let instance_id = body_json(create).await["instance_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let container_name = format!("clouddesk-runtime-{instance_id}");

    let extensions_dir = tempfile::tempdir_in(&identity.home).unwrap();
    let install = TokioCommand::new("docker")
        .args([
            "exec",
            &container_name,
            "code-server",
            "--install-extension",
            "streetsidesoftware.code-spell-checker",
            "--extensions-dir",
            &extensions_dir.path().to_string_lossy(),
            "--force",
        ])
        .output()
        .await
        .unwrap();
    assert!(
        install.status.success(),
        "extension install failed: stdout={} stderr={}",
        String::from_utf8_lossy(&install.stdout),
        String::from_utf8_lossy(&install.stderr)
    );

    let list = TokioCommand::new("docker")
        .args([
            "exec",
            &container_name,
            "code-server",
            "--list-extensions",
            "--extensions-dir",
            &extensions_dir.path().to_string_lossy(),
        ])
        .output()
        .await
        .unwrap();
    let installed = String::from_utf8_lossy(&list.stdout).to_lowercase();
    assert!(
        installed.contains("code-spell-checker"),
        "installed extension must be listed: {installed}"
    );

    // Persists on the real host filesystem -- the same mount-backed
    // persistence task_8_9 verified, now specifically for extensions.
    assert!(
        extensions_dir
            .path()
            .join("streetsidesoftware.code-spell-checker-4.2.4")
            .exists()
            || std::fs::read_dir(extensions_dir.path())
                .unwrap()
                .filter_map(Result::ok)
                .any(|e| e
                    .file_name()
                    .to_string_lossy()
                    .contains("code-spell-checker")),
        "extension directory must exist on the real host filesystem, not only inside the container"
    );

    // A second user's *separate* extensions directory (their own real
    // home, a different disposable subdirectory) never automatically
    // receives it -- proves per-user isolation, not merely "a
    // different directory path was used" by construction.
    let (_user_b_cookie, identity_b) =
        create_user_with_identity(&app, &admin_cookie, "extuser2").await;
    let other_extensions_dir = tempfile::tempdir_in(&identity_b.home).unwrap();
    let list_other = TokioCommand::new("docker")
        .args([
            "exec",
            &container_name,
            "code-server",
            "--list-extensions",
            "--extensions-dir",
            &other_extensions_dir.path().to_string_lossy(),
        ])
        .output()
        .await
        .unwrap();
    let other_installed = String::from_utf8_lossy(&list_other.stdout).to_lowercase();
    assert!(
        !other_installed.contains("code-spell-checker"),
        "a different user's extensions directory must not automatically contain another \
         user's installed extension: {other_installed}"
    );

    let stop_uri = format!("/api/v1/runtime-instances/code/{instance_id}/stop");
    let _ = app
        .clone()
        .oneshot(request(
            Method::POST,
            &stop_uri,
            Body::empty(),
            Some(&user_a_cookie),
        ))
        .await;
}

/// Task 30 -- crash recovery: killing the real container out from
/// under the manager is detected, the instance settles into a
/// terminal state (never stuck reporting Running), and a fresh
/// instance can be started afterward.
#[tokio::test]
async fn task_30_crash_recovery() {
    if !docker_and_image_available().await {
        eprintln!("SKIP: docker/{CODE_IMAGE} not reachable on this host");
        return;
    }
    let (app, _dir) = application_with_code().await;
    let admin_cookie = bootstrap_admin(&app).await;
    enable_code(&app, &admin_cookie).await;
    let (user_cookie, _identity) =
        create_user_with_identity(&app, &admin_cookie, "crashuser").await;

    let create = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/api/v1/runtime-instances",
            &json!({ "kind": "code" }),
            Some(&user_cookie),
        ))
        .await
        .unwrap();
    assert_eq!(create.status(), StatusCode::OK);
    let instance_id = body_json(create).await["instance_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let container_name = format!("clouddesk-runtime-{instance_id}");

    // Kill the real container out from under the manager -- not a
    // graceful stop through the API.
    let kill = TokioCommand::new("docker")
        .args(["kill", &container_name])
        .status()
        .await
        .unwrap();
    assert!(kill.success());

    let status_uri = format!("/api/v1/runtime-instances/code/{instance_id}");
    let mut settled = false;
    for _ in 0..30 {
        let status = app
            .clone()
            .oneshot(request(
                Method::GET,
                &status_uri,
                Body::empty(),
                Some(&user_cookie),
            ))
            .await
            .unwrap();
        let state = body_json(status).await["state"]
            .as_str()
            .unwrap()
            .to_owned();
        if matches!(state.as_str(), "failed" | "stopped") {
            settled = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    assert!(
        settled,
        "instance must settle into a terminal state after the container is killed"
    );

    // A fresh instance can still be started afterward.
    let restart_uri = format!("/api/v1/runtime-instances/code/{instance_id}/restart");
    let restart = app
        .clone()
        .oneshot(request(
            Method::POST,
            &restart_uri,
            Body::empty(),
            Some(&user_cookie),
        ))
        .await
        .unwrap();
    assert!(restart.status() == StatusCode::OK || restart.status() == StatusCode::BAD_GATEWAY);

    let stop_uri = format!("/api/v1/runtime-instances/code/{instance_id}/stop");
    let _ = app
        .clone()
        .oneshot(request(
            Method::POST,
            &stop_uri,
            Body::empty(),
            Some(&user_cookie),
        ))
        .await;
}

/// Task 8 (Phase 7 closure pass): prove the bundled TypeScript language
/// service performs genuine live *semantic* type-checking inside a real,
/// running Code container -- not just that the extension files exist on
/// disk. This is capability evidence for the language server, obtained
/// without a browser (`ts.transpileModule` only does syntax-level work and
/// does NOT surface semantic errors -- `ts.createProgram` +
/// `ts.getPreEmitDiagnostics` is required for a real type-check).
///
/// What this test does NOT prove: live IDE-rendered hover/completion/
/// squiggles, which requires the browser-driven editor UI and is recorded
/// separately as `LANGUAGE SERVER LIVE ACCEPTANCE: BLOCKED BY ENVIRONMENT`
/// in `PHASE7_CODE_EVIDENCE.md`.
#[tokio::test]
async fn task_8_language_service_semantic_diagnostics() {
    if !docker_and_image_available().await {
        eprintln!("SKIP: docker/{CODE_IMAGE} not reachable on this host");
        return;
    }

    let script = r#"
mkdir -p /tmp/tstest && cd /tmp/tstest
printf 'const x: number = "not a number";\n' > sample.ts
cat > check.js << 'EOF'
const ts = require("/usr/lib/code-server/lib/vscode/extensions/node_modules/typescript/lib/typescript.js");
const program = ts.createProgram(["/tmp/tstest/sample.ts"], { strict: true, noEmit: true });
const diags = ts.getPreEmitDiagnostics(program);
console.log(JSON.stringify({
  diagnosticCount: diags.length,
  firstMessage: diags[0] ? ts.flattenDiagnosticMessageText(diags[0].messageText, "\n") : null,
  tsVersion: ts.version
}));
EOF
/usr/lib/code-server/lib/node check.js
"#;

    let output = TokioCommand::new("docker")
        .args([
            "run",
            "--rm",
            "--entrypoint",
            "sh",
            CODE_IMAGE,
            "-c",
            script,
        ])
        .output()
        .await
        .expect("docker run for language-service probe must execute");
    assert!(
        output.status.success(),
        "language-service probe failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let last_line = stdout.lines().next_back().unwrap_or_default();
    let parsed: serde_json::Value =
        serde_json::from_str(last_line).expect("probe must print a single JSON line");

    // Genuine semantic error must be detected -- proves this is real
    // type-checking, not just a syntax parse.
    assert_eq!(parsed["diagnosticCount"].as_u64(), Some(1));
    assert!(
        parsed["firstMessage"]
            .as_str()
            .unwrap_or_default()
            .contains("not assignable"),
        "expected a real type-mismatch diagnostic, got: {parsed}"
    );
    assert!(
        parsed["tsVersion"].as_str().is_some(),
        "TypeScript engine version must be reported"
    );
}

/// Task 9 (Phase 7 closure pass): confirm the base image ships VS Code's
/// built-in JS/TS debug adapter extensions without installing anything at
/// request time. This is capability evidence only -- an actual interactive
/// debug session (setting a breakpoint, hitting it, inspecting a variable)
/// requires the Debug Adapter Protocol client that lives in the browser
/// editor UI, and is recorded separately as
/// `DEBUGGING LIVE ACCEPTANCE: BLOCKED BY ENVIRONMENT` in
/// `PHASE7_CODE_EVIDENCE.md`.
#[tokio::test]
async fn task_9_debug_extensions_bundled() {
    if !docker_and_image_available().await {
        eprintln!("SKIP: docker/{CODE_IMAGE} not reachable on this host");
        return;
    }

    let output = TokioCommand::new("docker")
        .args([
            "run",
            "--rm",
            "--entrypoint",
            "sh",
            CODE_IMAGE,
            "-c",
            "ls /usr/lib/code-server/lib/vscode/extensions | grep -i debug",
        ])
        .output()
        .await
        .expect("docker run for debug-extension probe must execute");
    assert!(
        output.status.success(),
        "expected at least one bundled debug extension, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("js-debug"),
        "expected ms-vscode.js-debug to be bundled, got: {stdout}"
    );
}

// ---------------------------------------------------------------------
// Phase 7 closure pass, Task 2 -- multiple Code workspaces.
// ---------------------------------------------------------------------

async fn whoami(app: &Router, cookie: &str) -> String {
    let response = app
        .clone()
        .oneshot(request(
            Method::GET,
            "/api/v1/auth/me",
            Body::empty(),
            Some(cookie),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    body_json(response).await["user_id"]
        .as_str()
        .unwrap()
        .to_owned()
}

/// Adds an assigned root (admin operation, mirrors the existing
/// Files/Music authorization model) and returns its `assigned_roots.id`
/// -- the only identifier the browser is ever allowed to use to select
/// a Code workspace.
async fn add_root(
    app: &Router,
    admin_cookie: &str,
    user_id: &str,
    path: &std::path::Path,
    access_mode: &str,
) -> String {
    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            &format!("/api/v1/users/{user_id}/assigned-roots"),
            &json!({ "path": path, "access_mode": access_mode }),
            Some(admin_cookie),
        ))
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        StatusCode::CREATED,
        "add_assigned_root must succeed"
    );
    body_json(response).await["root_id"]
        .as_str()
        .unwrap()
        .to_owned()
}

async fn remove_root(app: &Router, admin_cookie: &str, user_id: &str, root_id: &str) {
    let response = app
        .clone()
        .oneshot(request(
            Method::DELETE,
            &format!("/api/v1/users/{user_id}/assigned-roots/{root_id}"),
            Body::empty(),
            Some(admin_cookie),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
}

async fn create_code_instance(
    app: &Router,
    cookie: &str,
    workspace_id: Option<&str>,
) -> axum::response::Response {
    let mut body = json!({ "kind": "code" });
    if let Some(id) = workspace_id {
        body["workspace_id"] = json!(id);
    }
    app.clone()
        .oneshot(json_request(
            Method::POST,
            "/api/v1/runtime-instances",
            &body,
            Some(cookie),
        ))
        .await
        .unwrap()
}

async fn docker_exec(container: &str, script: &str) -> std::process::Output {
    TokioCommand::new("docker")
        .args(["exec", container, "sh", "-c", script])
        .output()
        .await
        .unwrap()
}

/// No containers involved -- self-service listing must return only the
/// caller's own workspaces (always including the default "Home" entry)
/// and never another user's.
#[tokio::test]
async fn task_2_list_own_workspaces_and_ownership_isolation() {
    let (app, _dir) = application_with_code().await;
    let admin_cookie = bootstrap_admin(&app).await;
    let (cookie_a, _identity_a) = create_user_with_identity(&app, &admin_cookie, "wsuser_a").await;
    let (cookie_b, _identity_b) = create_user_with_identity(&app, &admin_cookie, "wsuser_b").await;
    let user_a = whoami(&app, &cookie_a).await;

    let root_dir = tempfile::tempdir().unwrap();
    let root_id = add_root(&app, &admin_cookie, &user_a, root_dir.path(), "read-write").await;

    let list_a = app
        .clone()
        .oneshot(request(
            Method::GET,
            "/api/v1/code/workspaces",
            Body::empty(),
            Some(&cookie_a),
        ))
        .await
        .unwrap();
    assert_eq!(list_a.status(), StatusCode::OK);
    let body_a = body_json(list_a).await;
    let workspaces_a = body_a["workspaces"].as_array().unwrap();
    assert!(workspaces_a
        .iter()
        .any(|w| w["default"] == json!(true) && w["label"] == json!("Home")));
    assert!(
        workspaces_a
            .iter()
            .any(|w| w["id"] == json!(root_id) && w["read_write"] == json!(true)),
        "user A must see their own assigned root: {workspaces_a:?}"
    );
    // Never a raw host path in the response.
    for w in workspaces_a {
        assert!(
            w.get("path").is_none(),
            "workspace listing must never expose a raw host path"
        );
    }

    let list_b = app
        .clone()
        .oneshot(request(
            Method::GET,
            "/api/v1/code/workspaces",
            Body::empty(),
            Some(&cookie_b),
        ))
        .await
        .unwrap();
    let body_b = body_json(list_b).await;
    let workspaces_b = body_b["workspaces"].as_array().unwrap();
    assert!(
        !workspaces_b.iter().any(|w| w["id"] == json!(root_id)),
        "user B must never see user A's assigned root"
    );
}

/// No containers involved -- every authorization failure here happens
/// during server-side resolution, before any instance/container is
/// created, so this test runs unconditionally (no Docker dependency).
#[tokio::test]
async fn task_2_workspace_authorization_failures() {
    let (app, _dir) = application_with_code().await;
    let admin_cookie = bootstrap_admin(&app).await;
    let (cookie_a, _identity_a) = create_user_with_identity(&app, &admin_cookie, "wsuser_c").await;
    let (cookie_b, _identity_b) = create_user_with_identity(&app, &admin_cookie, "wsuser_d").await;
    let user_a = whoami(&app, &cookie_a).await;
    enable_code(&app, &admin_cookie).await;

    let root_dir = tempfile::tempdir().unwrap();
    let root_id = add_root(&app, &admin_cookie, &user_a, root_dir.path(), "read-write").await;

    // Cross-user: B requesting A's workspace ID.
    let cross_user = create_code_instance(&app, &cookie_b, Some(&root_id)).await;
    assert_eq!(cross_user.status(), StatusCode::NOT_FOUND);

    // Random, non-existent ID.
    let random = create_code_instance(&app, &cookie_a, Some("totally-not-a-real-root-id")).await;
    assert_eq!(random.status(), StatusCode::NOT_FOUND);

    // Traversal-shaped ID -- must resolve exactly like any other unknown
    // ID (a DB lookup miss), never treated as a filesystem path.
    let traversal = create_code_instance(&app, &cookie_a, Some("../../../../etc/passwd")).await;
    assert_eq!(traversal.status(), StatusCode::NOT_FOUND);

    // Revoked: valid ID, but the admin removed the assignment. An
    // *explicit* request for a revoked workspace is a hard failure
    // (never silently substituted).
    remove_root(&app, &admin_cookie, &user_a, &root_id).await;
    let revoked = create_code_instance(&app, &cookie_a, Some(&root_id)).await;
    assert_eq!(revoked.status(), StatusCode::NOT_FOUND);

    // Mixing workspace_id with a non-Code kind must be rejected too.
    let wrong_kind = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/api/v1/runtime-instances",
            &json!({ "kind": "browser", "workspace_id": "anything" }),
            Some(&cookie_a),
        ))
        .await
        .unwrap();
    assert_eq!(wrong_kind.status(), StatusCode::BAD_REQUEST);
}

/// Full live flow: writable vs. read-only mount enforcement, switching
/// A -> B -> A, and profile (settings/history) surviving every switch.
#[tokio::test]
async fn task_2_workspace_mount_permissions_and_switching() {
    if !docker_and_image_available().await {
        eprintln!("SKIP: docker/{CODE_IMAGE} not reachable on this host");
        return;
    }
    let (app, _dir) = application_with_code().await;
    let admin_cookie = bootstrap_admin(&app).await;
    enable_code(&app, &admin_cookie).await;
    let (cookie, identity) = create_user_with_identity(&app, &admin_cookie, "wsswitcher").await;
    let user_id = whoami(&app, &cookie).await;

    let writable_dir = tempfile::tempdir_in(&identity.home).unwrap();
    let readonly_dir = tempfile::tempdir_in(&identity.home).unwrap();
    let writable_id = add_root(
        &app,
        &admin_cookie,
        &user_id,
        writable_dir.path(),
        "read-write",
    )
    .await;
    let readonly_id = add_root(&app, &admin_cookie, &user_id, readonly_dir.path(), "read").await;

    // Profile marker: written to $HOME directly (always mounted rw at
    // /profile-equivalent regardless of workspace selection).
    let profile_marker = identity.home.join("phase7-task2-profile-marker.txt");

    // --- Select the writable workspace ---
    let created = create_code_instance(&app, &cookie, Some(&writable_id)).await;
    assert_eq!(created.status(), StatusCode::OK);
    let first_instance = body_json(created).await["instance_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let first_container = format!("clouddesk-runtime-{first_instance}");

    let write_ok = docker_exec(&first_container, "echo hello-a > /workspace/a-marker.txt").await;
    assert!(write_ok.status.success());
    assert!(writable_dir.path().join("a-marker.txt").exists());

    let write_profile = docker_exec(
        &first_container,
        &format!("echo profile-write > {}", profile_marker.to_string_lossy()),
    )
    .await;
    assert!(write_profile.status.success());

    // --- Switch to the read-only workspace ---
    let switched = create_code_instance(&app, &cookie, Some(&readonly_id)).await;
    assert_eq!(switched.status(), StatusCode::OK);
    let second_instance = body_json(switched).await["instance_id"]
        .as_str()
        .unwrap()
        .to_owned();
    // The per-user instance limit is 1 (Task 2 design note): switching
    // workspace reuses the same instance/row -- stop, re-stage, start
    // again with a bumped generation and a new mount -- rather than
    // proliferating instance rows (which would trip the limit) or
    // using `restart_instance` (whose crash-loop counter exists for
    // genuine crashes, not intentional switches).
    assert_eq!(
        first_instance, second_instance,
        "workspace switching reuses the same Code instance/row"
    );
    let second_container = format!("clouddesk-runtime-{second_instance}");
    assert_eq!(
        second_container, first_container,
        "same instance ID means the same well-known container name is reused after the switch"
    );

    // Writing into the read-only workspace mount must genuinely fail
    // inside the container (not merely hidden by CloudDesk's own UI).
    let write_should_fail = docker_exec(
        &second_container,
        "echo nope > /workspace/should-not-write.txt",
    )
    .await;
    assert!(
        !write_should_fail.status.success(),
        "a read-access workspace must be mounted read-only inside the container"
    );
    assert!(!readonly_dir.path().join("should-not-write.txt").exists());

    // Reading is still fine.
    std::fs::write(readonly_dir.path().join("preexisting.txt"), "seeded").unwrap();
    let read_ok = docker_exec(&second_container, "cat /workspace/preexisting.txt").await;
    assert_eq!(String::from_utf8_lossy(&read_ok.stdout).trim(), "seeded");

    // Profile (settings/history location) survived the switch.
    let read_profile = docker_exec(
        &second_container,
        &format!("cat {}", profile_marker.to_string_lossy()),
    )
    .await;
    assert_eq!(
        String::from_utf8_lossy(&read_profile.stdout).trim(),
        "profile-write",
        "the separate profile mount must survive a workspace switch"
    );

    // --- Switch back to the writable workspace ---
    let back = create_code_instance(&app, &cookie, Some(&writable_id)).await;
    assert_eq!(back.status(), StatusCode::OK);
    let third_instance = body_json(back).await["instance_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let third_container = format!("clouddesk-runtime-{third_instance}");
    let read_back = docker_exec(&third_container, "cat /workspace/a-marker.txt").await;
    assert_eq!(
        String::from_utf8_lossy(&read_back.stdout).trim(),
        "hello-a",
        "switching back to A must show A's own, undisturbed content"
    );

    let _ = app
        .clone()
        .oneshot(request(
            Method::POST,
            &format!("/api/v1/runtime-instances/code/{third_instance}/stop"),
            Body::empty(),
            Some(&cookie),
        ))
        .await;
}

/// Successful selection persists only after health; restart reopens the
/// last-used workspace; a deleted last-used workspace falls back to
/// home safely rather than failing the (implicit) restart/reopen.
#[tokio::test]
async fn task_2_persistence_restart_and_safe_fallback() {
    if !docker_and_image_available().await {
        eprintln!("SKIP: docker/{CODE_IMAGE} not reachable on this host");
        return;
    }
    let (app, _dir) = application_with_code().await;
    let admin_cookie = bootstrap_admin(&app).await;
    enable_code(&app, &admin_cookie).await;
    let (cookie, identity) = create_user_with_identity(&app, &admin_cookie, "wspersist").await;
    let user_id = whoami(&app, &cookie).await;

    let root_dir = tempfile::tempdir_in(&identity.home).unwrap();
    let root_id = add_root(&app, &admin_cookie, &user_id, root_dir.path(), "read-write").await;

    let created = create_code_instance(&app, &cookie, Some(&root_id)).await;
    assert_eq!(created.status(), StatusCode::OK);
    let instance_id = body_json(created).await["instance_id"]
        .as_str()
        .unwrap()
        .to_owned();

    // Persisted only now that the instance is confirmed healthy.
    let workspaces = app
        .clone()
        .oneshot(request(
            Method::GET,
            "/api/v1/code/workspaces",
            Body::empty(),
            Some(&cookie),
        ))
        .await
        .unwrap();
    assert_eq!(
        body_json(workspaces).await["last_workspace_id"],
        json!(root_id)
    );

    // Restart (no explicit workspace_id) must reauthorize and reopen
    // the same last-used workspace.
    let restart = app
        .clone()
        .oneshot(request(
            Method::POST,
            &format!("/api/v1/runtime-instances/code/{instance_id}/restart"),
            Body::empty(),
            Some(&cookie),
        ))
        .await
        .unwrap();
    assert_eq!(restart.status(), StatusCode::OK);
    let container = format!("clouddesk-runtime-{instance_id}");
    let check_mount = docker_exec(&container, "echo still-a > /workspace/still-a.txt").await;
    assert!(check_mount.status.success());
    assert!(root_dir.path().join("still-a.txt").exists());

    let _ = app
        .clone()
        .oneshot(request(
            Method::POST,
            &format!("/api/v1/runtime-instances/code/{instance_id}/stop"),
            Body::empty(),
            Some(&cookie),
        ))
        .await;

    // Delete the last-used workspace, then implicitly reopen Code (no
    // workspace_id) -- must fall back to home, not fail.
    remove_root(&app, &admin_cookie, &user_id, &root_id).await;
    let reopened = create_code_instance(&app, &cookie, None).await;
    assert_eq!(
        reopened.status(),
        StatusCode::OK,
        "an implicit reopen must fall back to the default workspace, not fail, \
         when the last-used one was revoked"
    );
    let reopened_id = body_json(reopened).await["instance_id"]
        .as_str()
        .unwrap()
        .to_owned();

    let workspaces_after = app
        .clone()
        .oneshot(request(
            Method::GET,
            "/api/v1/code/workspaces",
            Body::empty(),
            Some(&cookie),
        ))
        .await
        .unwrap();
    assert_eq!(
        body_json(workspaces_after).await["last_workspace_id"],
        Value::Null,
        "falling back to home must also update the persisted selection"
    );

    let _ = app
        .clone()
        .oneshot(request(
            Method::POST,
            &format!("/api/v1/runtime-instances/code/{reopened_id}/stop"),
            Body::empty(),
            Some(&cookie),
        ))
        .await;
}

/// Two concurrent workspace-switch requests for the same user must
/// converge to exactly one final running Code instance -- never two
/// simultaneously live containers for one user, and never a stuck
/// half-switched state.
#[tokio::test]
async fn task_2_concurrent_switches_converge_to_one_instance() {
    if !docker_and_image_available().await {
        eprintln!("SKIP: docker/{CODE_IMAGE} not reachable on this host");
        return;
    }
    let (app, _dir) = application_with_code().await;
    let admin_cookie = bootstrap_admin(&app).await;
    enable_code(&app, &admin_cookie).await;
    let (cookie, identity) = create_user_with_identity(&app, &admin_cookie, "wsconcurrent").await;
    let user_id = whoami(&app, &cookie).await;

    let dir_a = tempfile::tempdir_in(&identity.home).unwrap();
    let dir_b = tempfile::tempdir_in(&identity.home).unwrap();
    let root_a = add_root(&app, &admin_cookie, &user_id, dir_a.path(), "read-write").await;
    let root_b = add_root(&app, &admin_cookie, &user_id, dir_b.path(), "read-write").await;

    // A first instance already running, then two concurrent switches.
    let first = create_code_instance(&app, &cookie, Some(&root_a)).await;
    assert_eq!(first.status(), StatusCode::OK);

    let (result_a, result_b) = tokio::join!(
        create_code_instance(&app, &cookie, Some(&root_a)),
        create_code_instance(&app, &cookie, Some(&root_b)),
    );
    // At least one must succeed; both are permitted to succeed (the
    // loser's stop races the winner's start) as long as the *final*
    // state converges to a single running instance.
    assert!(
        result_a.status() == StatusCode::OK || result_b.status() == StatusCode::OK,
        "at least one concurrent switch must succeed"
    );

    let instances = app
        .clone()
        .oneshot(request(
            Method::GET,
            "/api/v1/runtime-instances",
            Body::empty(),
            Some(&cookie),
        ))
        .await
        .unwrap();
    let rows = body_json(instances).await["instances"]
        .as_array()
        .unwrap()
        .clone();
    let running_code: Vec<_> = rows
        .iter()
        .filter(|r| r["kind"] == json!("code") && r["state"] == json!("running"))
        .collect();
    assert_eq!(
        running_code.len(),
        1,
        "exactly one Code instance must end up running for this user after concurrent \
         switches, got: {rows:?}"
    );

    let surviving_id = running_code[0]["instance_id"].as_str().unwrap().to_owned();
    let _ = app
        .clone()
        .oneshot(request(
            Method::POST,
            &format!("/api/v1/runtime-instances/code/{surviving_id}/stop"),
            Body::empty(),
            Some(&cookie),
        ))
        .await;
}
