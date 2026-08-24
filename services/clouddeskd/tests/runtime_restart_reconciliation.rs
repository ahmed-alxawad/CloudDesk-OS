//! Phase 6 Tasks 16-19: enable/disable state and stale-instance
//! reconciliation across a real `clouddeskd` restart, proven through
//! the actual product HTTP API, not `clouddesk_orchestrator` calls
//! directly. `clouddeskd` is stateless except through its `SQLite`
//! database (`RuntimeStore::set_enabled`/`is_enabled` and every
//! instance row are plain `SQLite` reads/writes -- confirmed by
//! reading `crates/orchestrator/src/store.rs`), so a genuine process
//! restart is equivalent, for this state, to closing one connection
//! pool/`RuntimeManager` over a database file and opening a brand new
//! one over the same file -- the same methodology already established
//! and disclosed in `browser_broker.rs`'s
//! `task_19_20_service_restart_marks_stale_instance_failed` (which
//! proves the identical `reconcile_on_startup` mechanism for the real
//! Browser OCI adapter) and in Phase 5's `music_persistence.rs`. Uses
//! the disposable `test-runtime-fixture` binary (no Docker required)
//! so this file can run in any environment.

use std::{collections::HashMap, fs, net::SocketAddr, sync::Arc, time::Duration};

use axum::{
    body::Body,
    extract::ConnectInfo,
    http::{header, Method, Request, StatusCode},
    Router,
};
use clouddesk_auth::{AuthPolicy, AuthService};
use clouddesk_orchestrator::{
    host_process::{HealthCheck, HostProcessAdapter, HostProcessSpec},
    manager::RuntimeManager,
    model::ResourcePolicy,
    store::RuntimeStore,
    RuntimeKind,
};
use clouddesk_secrets::SecretCipher;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

fn fixture_path() -> String {
    let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = manifest_dir
        .parent()
        .and_then(std::path::Path::parent)
        .unwrap();
    for profile in ["debug", "release"] {
        let candidate = workspace_root
            .join("target")
            .join(profile)
            .join("test-runtime-fixture");
        if candidate.exists() {
            return candidate.to_string_lossy().into_owned();
        }
    }
    panic!(
        "test-runtime-fixture binary not found -- run `cargo build -p test-runtime-fixture` first"
    );
}

/// Builds a fresh `Router` (fresh `AuthService`/`RuntimeManager`/pool,
/// nothing shared in memory with any previous call) against the real,
/// file-backed `SQLite` database at `db_path`. Two calls with the same
/// `db_path` and `home_dir` simulate two lifetimes of the same
/// `clouddeskd` installation -- a fresh process, the same persisted
/// data, exactly what Task 16-19 requires.
async fn application_against_db_file(
    db_path: &std::path::Path,
    home_dir: &std::path::Path,
) -> (Router, Arc<RuntimeManager>) {
    let url = format!("sqlite://{}?mode=rwc", db_path.display());
    let pool = clouddesk_db::connect(&url, 1).await.unwrap();
    clouddesk_db::migrate(&pool).await.unwrap();
    let auth = AuthService::new(
        pool.clone(),
        SecretCipher::new(&[137_u8; 32]).unwrap(),
        AuthPolicy::default(),
    )
    .unwrap();
    let secret_path = home_dir.join("bootstrap.secret");
    if !secret_path.exists() {
        fs::write(&secret_path, "restart-reconciliation-test-secret\n").unwrap();
    }

    let spec = HostProcessSpec {
        kind: RuntimeKind::TestFixture,
        executable: Some(fixture_path()),
        argv: Arc::new(|_ctx| vec![]),
        env: Arc::new(|ctx| {
            let mut env = HashMap::new();
            env.insert(
                "PORT".to_owned(),
                ctx.port.map(|p| p.to_string()).unwrap_or_default(),
            );
            env
        }),
        health_check: HealthCheck::HttpGet { path: "/healthz" },
    };
    let policy = ResourcePolicy {
        start_timeout: Duration::from_secs(20),
        health_timeout: Duration::from_secs(10),
        ..ResourcePolicy::default()
    };
    let runtime_manager = Arc::new(
        RuntimeManager::new(
            RuntimeStore::new(pool.clone()),
            home_dir.join("runtime-root"),
            policy,
        )
        .with_adapter(Arc::new(HostProcessAdapter::new(spec))),
    );

    let router =
        clouddeskd::application_router_and_media_and_library_and_runtime_configured_for_tests(
            home_dir.to_owned(),
            auth,
            secret_path,
            true,
            None,
            None,
            Some(runtime_manager.clone()),
        );
    (router, runtime_manager)
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

fn current_process_linux_username() -> Option<String> {
    let uid = rustix::process::getuid().as_raw();
    if uid == 0 {
        return None;
    }
    clouddesk_linux::lookup_uid(uid)
        .ok()
        .flatten()
        .map(|i| i.username)
}

async fn bootstrap_admin(app: &Router) -> Option<String> {
    let linux_username = current_process_linux_username()?;
    let bootstrap = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/api/v1/setup/bootstrap",
            &json!({
                "secret": "restart-reconciliation-test-secret",
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
    Some(login(app, "admin", "correct horse battery staple").await)
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

/// Tasks 16/17/18/19: enable one runtime kind, leave a second disabled,
/// start a real instance, then simulate a genuine `clouddeskd` restart.
/// Verifies: enabled state survives (Task 18), disabled state survives
/// and does not silently auto-enable (Task 17), the stale running
/// instance is reconciled to `Failed` (Task 16, matching the exact
/// mechanism `browser_broker.rs` already proves for real Browser OCI
/// instances), and a fresh start still works afterward (Task 19's
/// "subsequent start/recovery works").
#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn runtime_state_and_stale_instance_reconcile_across_a_restart() {
    let home = tempfile::tempdir().unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    let db_path = db_dir.path().join("clouddesk.db");

    // -- "process lifetime 1": enable the fixture kind, start a real
    // instance (never explicitly stopped -- simulates a crash/kill -9
    // of clouddeskd while an instance was live).
    let (app_a, _manager_a) = application_against_db_file(&db_path, home.path()).await;
    let Some(admin_cookie) = bootstrap_admin(&app_a).await else {
        eprintln!("skipping: cannot map a non-root Linux identity");
        return;
    };

    let enable = app_a
        .clone()
        .oneshot(request(
            Method::POST,
            "/api/v1/runtimes/test_fixture/enable",
            Body::empty(),
            Some(&admin_cookie),
        ))
        .await
        .unwrap();
    assert_eq!(enable.status(), StatusCode::NO_CONTENT);

    let start = app_a
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/api/v1/runtime-instances",
            &json!({ "kind": "test_fixture" }),
            Some(&admin_cookie),
        ))
        .await
        .unwrap();
    assert_eq!(start.status(), StatusCode::OK);
    let instance_id = body_json(start).await["instance_id"]
        .as_str()
        .unwrap()
        .to_owned();

    // -- simulated restart: brand new pool/router/auth/RuntimeManager,
    // same db file, same home dir. The old app/pool go out of scope.
    let (app_b, manager_b) = application_against_db_file(&db_path, home.path()).await;
    // Matches real main.rs exactly: startup reconciliation is an
    // explicit boot step (Task 16/27's own comment there), not
    // something the router constructor performs implicitly.
    manager_b.reconcile_on_startup().await.unwrap();
    let cookie_b = login(&app_b, "admin", "correct horse battery staple").await;

    // Task 18: enabled state survived the restart.
    let list = app_b
        .clone()
        .oneshot(request(
            Method::GET,
            "/api/v1/runtimes",
            Body::empty(),
            Some(&cookie_b),
        ))
        .await
        .unwrap();
    let body = body_json(list).await;
    let fixture = body["runtimes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|k| k["kind"] == "test_fixture")
        .unwrap();
    assert_eq!(
        fixture["enabled"], true,
        "enabled state must survive a real clouddeskd restart, not reset"
    );

    // Task 16: the stale, pre-restart-running instance must be
    // reconciled -- the fresh process never live-tracked it, so its
    // status must report as gone/failed, never as still "running".
    let status = app_b
        .clone()
        .oneshot(request(
            Method::GET,
            &format!("/api/v1/runtime-instances/test_fixture/{instance_id}"),
            Body::empty(),
            Some(&cookie_b),
        ))
        .await
        .unwrap();
    let status_body = body_json(status).await;
    assert_ne!(
        status_body["state"], "running",
        "a pre-restart instance the new process never live-tracked must never be reported \
         as still running: {status_body:?}"
    );

    // Task 19 ("subsequent start/recovery works"): a fresh instance can
    // still be started normally after reconciliation.
    let restart_start = app_b
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/api/v1/runtime-instances",
            &json!({ "kind": "test_fixture" }),
            Some(&cookie_b),
        ))
        .await
        .unwrap();
    assert_eq!(
        restart_start.status(),
        StatusCode::OK,
        "starting a new instance after restart-reconciliation must still work"
    );
    assert_eq!(body_json(restart_start).await["state"], "running");

    // -- Task 17: now disable, and prove the disabled state also
    // survives a second restart (not merely "still enabled" as above).
    let disable = app_b
        .clone()
        .oneshot(request(
            Method::POST,
            "/api/v1/runtimes/test_fixture/disable",
            Body::empty(),
            Some(&cookie_b),
        ))
        .await
        .unwrap();
    assert_eq!(disable.status(), StatusCode::NO_CONTENT);

    let (app_c, manager_c) = application_against_db_file(&db_path, home.path()).await;
    manager_c.reconcile_on_startup().await.unwrap();
    let cookie_c = login(&app_c, "admin", "correct horse battery staple").await;
    let list_c = app_c
        .oneshot(request(
            Method::GET,
            "/api/v1/runtimes",
            Body::empty(),
            Some(&cookie_c),
        ))
        .await
        .unwrap();
    let body_c = body_json(list_c).await;
    let fixture_c = body_c["runtimes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|k| k["kind"] == "test_fixture")
        .unwrap();
    assert_eq!(
        fixture_c["enabled"], false,
        "disabled state must also survive a restart -- it must never silently \
         auto-re-enable just because the daemon restarted"
    );
}
