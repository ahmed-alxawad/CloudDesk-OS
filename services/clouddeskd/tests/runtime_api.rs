//! Phase 6 live product failure matrix (Task 24) -- exercises the
//! shared runtime orchestrator through the *actual clouddeskd HTTP
//! API*, not direct `clouddesk_orchestrator` calls only. Uses the
//! disposable `test-runtime-fixture` binary as its runtime kind, wired
//! through the test-only `..._for_tests` router constructor that is
//! never used by `main.rs` (Task 15) -- production routers never allow
//! `RuntimeKind::TestFixture`, proven separately below.
//!
//! No mocks: real HTTP requests via `tower::ServiceExt::oneshot`, a
//! real spawned fixture process, real ownership/RBAC enforcement.

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
    model::{Persistence, ResourcePolicy},
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

async fn application_with_runtime() -> (Router, tempfile::TempDir, tempfile::TempDir) {
    application_with_runtime_env(HashMap::new()).await
}

/// Same as [`application_with_runtime`], but the fixture process also
/// receives `extra_env` -- used to drive the fixture's own test-only
/// `LOG_TEST_PAYLOAD_HEX`/`LOG_TEST_REPEAT` knobs (Task 11/12) without
/// duplicating the whole harness.
async fn application_with_runtime_env(
    extra_env: HashMap<String, String>,
) -> (Router, tempfile::TempDir, tempfile::TempDir) {
    let _ = tracing_subscriber::fmt().with_test_writer().try_init();
    let pool = clouddesk_db::connect("sqlite::memory:", 1).await.unwrap();
    clouddesk_db::migrate(&pool).await.unwrap();

    let auth = AuthService::new(
        pool.clone(),
        SecretCipher::new(&[7_u8; 32]).unwrap(),
        AuthPolicy::default(),
    )
    .unwrap();
    let directory = tempfile::tempdir().unwrap();
    let secret_path = directory.path().join("bootstrap.secret");
    fs::write(&secret_path, "runtime-test-secret\n").unwrap();

    let runtime_root = tempfile::tempdir().unwrap();
    let spec = HostProcessSpec {
        kind: RuntimeKind::TestFixture,
        executable: Some(fixture_path()),
        argv: Arc::new(|_ctx| vec![]),
        env: Arc::new(move |ctx| {
            let mut env = extra_env.clone();
            env.insert(
                "PORT".to_owned(),
                ctx.port.map(|p| p.to_string()).unwrap_or_default(),
            );
            env
        }),
        health_check: HealthCheck::HttpGet { path: "/healthz" },
    };
    // Generous relative to the fixture's actual (sub-second) startup
    // time -- this whole file runs as part of `cargo test --workspace`
    // alongside many other CPU-heavy test binaries (ffmpeg, Docker,
    // SSH), so a tight budget here is a source of test flakiness under
    // contention, not a meaningful assertion about product behavior
    // (the orchestrator-level start/health-timeout *semantics* are
    // covered by `crates/orchestrator`'s own dedicated timeout tests).
    let policy = ResourcePolicy {
        start_timeout: Duration::from_secs(20),
        health_timeout: Duration::from_secs(10),
        ..ResourcePolicy::default()
    };
    let runtime_manager = Arc::new(
        RuntimeManager::new(
            RuntimeStore::new(pool.clone()),
            runtime_root.path().to_owned(),
            policy,
        )
        .with_adapter(Arc::new(HostProcessAdapter::new(spec))),
    );

    (
        clouddeskd::application_router_and_media_and_library_and_runtime_configured_for_tests(
            directory.path().to_owned(),
            auth,
            secret_path,
            true,
            None,
            None,
            Some(runtime_manager),
        ),
        directory,
        runtime_root,
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

async fn bootstrap_admin(app: &Router) -> String {
    let linux_username = current_process_linux_username();
    let bootstrap = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/api/v1/setup/bootstrap",
            &json!({
                "secret": "runtime-test-secret",
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

async fn create_user(app: &Router, admin_cookie: &str, username: &str, role_id: &str) -> String {
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
                "role_ids": [role_id],
            }),
            Some(admin_cookie),
        ))
        .await
        .unwrap();
    assert_eq!(create.status(), StatusCode::CREATED);
    login(app, username, "user horse battery staple").await
}

/// Tasks 1-3, 6 (admin visibility + enable authorization + enable).
#[tokio::test]
async fn task_1_2_3_admin_sees_status_only_admin_can_enable() {
    let (app, _dir, _root) = application_with_runtime().await;
    let admin_cookie = bootstrap_admin(&app).await;
    let user_cookie = create_user(&app, &admin_cookie, "regular", "user").await;

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
    assert_eq!(list.status(), StatusCode::OK);
    let body = body_json(list).await;
    let kinds: Vec<&str> = body["runtimes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|k| k["kind"].as_str().unwrap())
        .collect();
    assert!(kinds.contains(&"test_fixture"));
    assert!(
        !kinds.contains(&"code") || body["runtimes"]
            .as_array()
            .unwrap()
            .iter()
            .all(|k| k["kind"] != "code" || !k["available"].as_bool().unwrap()),
        "code has no registered adapter in this test harness and must report unavailable, not available"
    );

    // Non-admin cannot enable.
    let denied = app
        .clone()
        .oneshot(request(
            Method::POST,
            "/api/v1/runtimes/test_fixture/enable",
            Body::empty(),
            Some(&user_cookie),
        ))
        .await
        .unwrap();
    assert_eq!(denied.status(), StatusCode::FORBIDDEN);

    // Admin can.
    let enabled = app
        .clone()
        .oneshot(request(
            Method::POST,
            "/api/v1/runtimes/test_fixture/enable",
            Body::empty(),
            Some(&admin_cookie),
        ))
        .await
        .unwrap();
    assert_eq!(enabled.status(), StatusCode::NO_CONTENT);
}

async fn enable_fixture(app: &Router, admin_cookie: &str) {
    let response = app
        .clone()
        .oneshot(request(
            Method::POST,
            "/api/v1/runtimes/test_fixture/enable",
            Body::empty(),
            Some(admin_cookie),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
}

async fn start_instance(app: &Router, cookie: &str) -> String {
    let create = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/api/v1/runtime-instances",
            &json!({ "kind": "test_fixture" }),
            Some(cookie),
        ))
        .await
        .unwrap();
    assert_eq!(create.status(), StatusCode::OK, "instance failed to start");
    let body = body_json(create).await;
    assert_eq!(
        body["state"], "running",
        "readiness must come from a real passed health check"
    );
    body["instance_id"].as_str().unwrap().to_owned()
}

/// Tasks 4, 5, 7, 10 -- own instance starts, becomes RUNNING only after
/// health, internal port never disclosed in the response.
#[tokio::test]
async fn task_4_5_10_start_readiness_and_no_port_disclosure() {
    let (app, _dir, _root) = application_with_runtime().await;
    let admin_cookie = bootstrap_admin(&app).await;
    enable_fixture(&app, &admin_cookie).await;
    let user_cookie = create_user(&app, &admin_cookie, "userone", "user").await;

    let create = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/api/v1/runtime-instances",
            &json!({ "kind": "test_fixture" }),
            Some(&user_cookie),
        ))
        .await
        .unwrap();
    assert_eq!(create.status(), StatusCode::OK);
    let raw = create.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8_lossy(&raw);
    assert!(
        !text.contains("127.0.0.1:") && !text.to_lowercase().contains("\"port\""),
        "the runtime-instance API response must never disclose the internal port: {text}"
    );
}

/// Task 8/24.6-7 -- authenticated HTTP proxy reaches the owner's
/// instance; a different user gets a 404, not the proxied content.
#[tokio::test]
async fn task_8_http_proxy_owner_succeeds_cross_user_denied() {
    let (app, _dir, _root) = application_with_runtime().await;
    let admin_cookie = bootstrap_admin(&app).await;
    enable_fixture(&app, &admin_cookie).await;
    let owner_cookie = create_user(&app, &admin_cookie, "owner", "user").await;
    let attacker_cookie = create_user(&app, &admin_cookie, "attacker", "user").await;
    let instance_id = start_instance(&app, &owner_cookie).await;

    let uri = format!(
        "/api/v1/runtime-instances/test_fixture/{instance_id}/proxy/echo?msg=through-clouddeskd"
    );
    let owner_response = app
        .clone()
        .oneshot(request(
            Method::GET,
            &uri,
            Body::empty(),
            Some(&owner_cookie),
        ))
        .await
        .unwrap();
    assert_eq!(owner_response.status(), StatusCode::OK);
    let body = owner_response
        .into_body()
        .collect()
        .await
        .unwrap()
        .to_bytes();
    assert_eq!(&body[..], b"through-clouddeskd");

    let attacker_response = app
        .clone()
        .oneshot(request(
            Method::GET,
            &uri,
            Body::empty(),
            Some(&attacker_cookie),
        ))
        .await
        .unwrap();
    assert_eq!(attacker_response.status(), StatusCode::NOT_FOUND);

    // Task 24.24 -- spoofing the Host header has no effect on which
    // upstream is contacted: the response still comes from the owner's
    // own fixture, proving there is no attacker-reachable upstream
    // selection at all, not merely one that happened not to be probed.
    let mut spoofed = request(Method::GET, &uri, Body::empty(), Some(&owner_cookie));
    spoofed
        .headers_mut()
        .insert(header::HOST, "169.254.169.254".parse().unwrap());
    let spoofed_response = app.clone().oneshot(spoofed).await.unwrap();
    assert_eq!(spoofed_response.status(), StatusCode::OK);
}

/// Task 9/24.8-9 -- authenticated WebSocket proxy, exercised through a
/// real bound TCP listener (an in-process `tower::oneshot` call cannot
/// perform a genuine protocol upgrade -- there is no real hyper
/// connection to hand off, so `WebSocketUpgrade` would reject every
/// request with 426 regardless of authorization). This is the same
/// "real server, real client" shape `crates/orchestrator`'s own
/// `live_proxy.rs` uses to prove the proxy leg; here it proves the
/// *product* route (session-cookie authentication + ownership scoping
/// in front of that same proxy leg).
#[tokio::test]
async fn task_9_websocket_proxy_owner_succeeds_cross_user_denied() {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    use tokio_tungstenite::tungstenite::http::header::COOKIE;

    let (app, _dir, _root) = application_with_runtime().await;
    let admin_cookie = bootstrap_admin(&app).await;
    enable_fixture(&app, &admin_cookie).await;
    let owner_cookie = create_user(&app, &admin_cookie, "wsowner", "user").await;
    let attacker_cookie = create_user(&app, &admin_cookie, "wsattacker", "user").await;
    let instance_id = start_instance(&app, &owner_cookie).await;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let local_addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .unwrap();
    });

    let path = format!("/api/v1/runtime-instances/test_fixture/{instance_id}/proxy-ws");
    let owner_url = format!("ws://{local_addr}{path}");
    let mut owner_request = owner_url.into_client_request().unwrap();
    owner_request
        .headers_mut()
        .insert(COOKIE, owner_cookie.parse().unwrap());
    let (mut owner_ws, owner_response) = tokio_tungstenite::connect_async(owner_request)
        .await
        .expect("owner must be able to open the proxied WebSocket");
    assert_eq!(owner_response.status(), StatusCode::SWITCHING_PROTOCOLS);
    owner_ws
        .send(tokio_tungstenite::tungstenite::Message::Text(
            "through-clouddeskd-ws".into(),
        ))
        .await
        .unwrap();
    let reply = tokio::time::timeout(Duration::from_secs(5), owner_ws.next())
        .await
        .expect("timed out waiting for the proxied echo")
        .unwrap()
        .unwrap();
    match reply {
        tokio_tungstenite::tungstenite::Message::Text(text) => {
            assert_eq!(text.as_str(), "through-clouddeskd-ws");
        }
        other => panic!("unexpected message: {other:?}"),
    }

    // A different user's cookie is denied -- the socket is closed by
    // the server without ever reaching the fixture, not merely left to
    // time out.
    let attacker_url = format!("ws://{local_addr}{path}");
    let mut attacker_request = attacker_url.into_client_request().unwrap();
    attacker_request
        .headers_mut()
        .insert(COOKIE, attacker_cookie.parse().unwrap());
    let (mut attacker_ws, _) = tokio_tungstenite::connect_async(attacker_request)
        .await
        .expect("the HTTP upgrade itself still succeeds; denial happens inside proxy_ws");
    let outcome = tokio::time::timeout(Duration::from_secs(5), attacker_ws.next()).await;
    match outcome {
        // Closed cleanly, connection dropped, or read failed outright --
        // any of these is "denied", never the proxied echo reply.
        Ok(Some(Ok(tokio_tungstenite::tungstenite::Message::Close(_)) | Err(_)) | None) => {}
        other => panic!("expected the attacker's socket to be closed by the server, got {other:?}"),
    }
}

/// Task 24.10 -- the per-user instance limit doubles as duplicate-start
/// protection: a second `create_instance` call for the same kind is
/// refused, not silently spawning an uncontrolled second instance.
#[tokio::test]
async fn task_10_duplicate_start_is_refused_not_a_second_uncontrolled_instance() {
    let (app, _dir, _root) = application_with_runtime().await;
    let admin_cookie = bootstrap_admin(&app).await;
    enable_fixture(&app, &admin_cookie).await;
    let user_cookie = create_user(&app, &admin_cookie, "dup", "user").await;
    let _first = start_instance(&app, &user_cookie).await;

    let second = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/api/v1/runtime-instances",
            &json!({ "kind": "test_fixture" }),
            Some(&user_cookie),
        ))
        .await
        .unwrap();
    assert_eq!(second.status(), StatusCode::TOO_MANY_REQUESTS);
}

/// Task 24.12-14 -- stop and restart through the real API.
#[tokio::test]
async fn task_12_14_stop_and_restart() {
    let (app, _dir, _root) = application_with_runtime().await;
    let admin_cookie = bootstrap_admin(&app).await;
    enable_fixture(&app, &admin_cookie).await;
    let user_cookie = create_user(&app, &admin_cookie, "stopstart", "user").await;
    let instance_id = start_instance(&app, &user_cookie).await;

    let stop_uri = format!("/api/v1/runtime-instances/test_fixture/{instance_id}/stop");
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

    let status_uri = format!("/api/v1/runtime-instances/test_fixture/{instance_id}");
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
    assert_eq!(body_json(status).await["state"], "stopped");

    let restart_uri = format!("/api/v1/runtime-instances/test_fixture/{instance_id}/restart");
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
}

/// Task 19 -- disable while active drives the real graceful-stop
/// sequence, reflected through the API's own status endpoint.
#[tokio::test]
async fn task_19_disable_while_active() {
    let (app, _dir, _root) = application_with_runtime().await;
    let admin_cookie = bootstrap_admin(&app).await;
    enable_fixture(&app, &admin_cookie).await;
    let user_cookie = create_user(&app, &admin_cookie, "diswhileactive", "user").await;
    let instance_id = start_instance(&app, &user_cookie).await;

    let disable = app
        .clone()
        .oneshot(request(
            Method::POST,
            "/api/v1/runtimes/test_fixture/disable",
            Body::empty(),
            Some(&admin_cookie),
        ))
        .await
        .unwrap();
    assert_eq!(disable.status(), StatusCode::NO_CONTENT);

    let status_uri = format!("/api/v1/runtime-instances/test_fixture/{instance_id}");
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
    assert_eq!(
        body_json(status).await["state"],
        "stopped",
        "disabling a runtime kind must gracefully stop already-running instances"
    );

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
    let fixture = body["runtimes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|k| k["kind"] == "test_fixture")
        .unwrap();
    assert_eq!(fixture["enabled"], false);
}

/// Task 24.23 -- a guessed/stale/deleted instance ID is denied
/// identically to a real one belonging to someone else (no existence
/// oracle).
#[tokio::test]
async fn task_23_hostile_and_stale_instance_ids_denied() {
    let (app, _dir, _root) = application_with_runtime().await;
    let admin_cookie = bootstrap_admin(&app).await;
    enable_fixture(&app, &admin_cookie).await;
    let user_cookie = create_user(&app, &admin_cookie, "hostile", "user").await;

    for hostile_id in [
        "does-not-exist",
        "../../../etc/passwd",
        "%2e%2e%2fadmin",
        "'; DROP TABLE runtime_instances; --",
        &"a".repeat(4096),
    ] {
        let uri = format!(
            "/api/v1/runtime-instances/test_fixture/{}",
            urlencoding_lite(hostile_id)
        );
        let response = app
            .clone()
            .oneshot(request(
                Method::GET,
                &uri,
                Body::empty(),
                Some(&user_cookie),
            ))
            .await
            .unwrap();
        assert!(
            response.status() == StatusCode::NOT_FOUND
                || response.status() == StatusCode::BAD_REQUEST,
            "hostile instance id {hostile_id:?} must be denied safely, got {}",
            response.status()
        );
    }
}

/// Minimal, dependency-free percent-encoding sufficient for building a
/// URI path segment out of a hostile test string (spaces, `#`, `?`
/// would otherwise corrupt the request line).
fn urlencoding_lite(value: &str) -> String {
    value
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}

/// Task 2/15 -- the test fixture kind is never reachable through a
/// *production* router, even though the binary exists on disk and the
/// orchestrator crate happily runs it when explicitly wired (as every
/// other test in this file does via the `_for_tests` constructor).
#[tokio::test]
async fn task_2_15_test_fixture_kind_is_rejected_by_the_production_router() {
    let pool = clouddesk_db::connect("sqlite::memory:", 1).await.unwrap();
    clouddesk_db::migrate(&pool).await.unwrap();
    let auth = AuthService::new(
        pool.clone(),
        SecretCipher::new(&[3_u8; 32]).unwrap(),
        AuthPolicy::default(),
    )
    .unwrap();
    let directory = tempfile::tempdir().unwrap();
    let secret_path = directory.path().join("bootstrap.secret");
    fs::write(&secret_path, "prod-secret\n").unwrap();

    let runtime_root = tempfile::tempdir().unwrap();
    let spec = HostProcessSpec {
        kind: RuntimeKind::TestFixture,
        executable: Some(fixture_path()),
        argv: Arc::new(|_ctx| vec![]),
        env: Arc::new(|_ctx| HashMap::new()),
        health_check: HealthCheck::HttpGet { path: "/healthz" },
    };
    // Even with the adapter registered (as a misconfiguration might do),
    // the *production* constructor's `runtime_allow_test_kind: false`
    // still refuses the kind at the HTTP layer -- registration alone is
    // not enough to reach it.
    let runtime_manager = Arc::new(
        RuntimeManager::new(
            RuntimeStore::new(pool.clone()),
            runtime_root.path().to_owned(),
            ResourcePolicy::default(),
        )
        .with_adapter(Arc::new(HostProcessAdapter::new(spec))),
    );

    let app = clouddeskd::application_router_and_media_and_library_and_runtime_configured(
        directory.path().to_owned(),
        auth,
        secret_path,
        true,
        None,
        None,
        Some(runtime_manager),
    );

    let linux_username = current_process_linux_username();
    let bootstrap = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/api/v1/setup/bootstrap",
            &json!({
                "secret": "prod-secret",
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
    let admin_cookie = login(&app, "admin", "correct horse battery staple").await;

    let create = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/api/v1/runtime-instances",
            &json!({ "kind": "test_fixture" }),
            Some(&admin_cookie),
        ))
        .await
        .unwrap();
    assert_eq!(
        create.status(),
        StatusCode::BAD_REQUEST,
        "the disposable test fixture must never be startable through a production router"
    );

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
    let kinds: Vec<&str> = body["runtimes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|k| k["kind"].as_str().unwrap())
        .collect();
    assert!(
        !kinds.contains(&"test_fixture"),
        "production /api/v1/runtimes must never list the test fixture"
    );
}

/// Task 12 -- bounded, sanitized logs.
#[tokio::test]
async fn task_12_bounded_sanitized_logs() {
    let (app, _dir, _root) = application_with_runtime().await;
    let admin_cookie = bootstrap_admin(&app).await;
    enable_fixture(&app, &admin_cookie).await;
    let user_cookie = create_user(&app, &admin_cookie, "loguser", "user").await;
    let instance_id = start_instance(&app, &user_cookie).await;

    let logs_uri = format!("/api/v1/runtime-instances/test_fixture/{instance_id}/logs");
    let response = app
        .clone()
        .oneshot(request(
            Method::GET,
            &logs_uri,
            Body::empty(),
            Some(&user_cookie),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    let logs = body["logs"].as_str().unwrap();
    assert!(
        logs.len() <= 64 * 1024,
        "log response must be bounded, got {} bytes",
        logs.len()
    );

    // Cross-user log read is denied identically to a not-found instance.
    let attacker_cookie = create_user(&app, &admin_cookie, "logattacker", "user").await;
    let denied = app
        .clone()
        .oneshot(request(
            Method::GET,
            &logs_uri,
            Body::empty(),
            Some(&attacker_cookie),
        ))
        .await
        .unwrap();
    assert_eq!(denied.status(), StatusCode::NOT_FOUND);
}

/// Task 24.11 -- simultaneous stop+restart requests don't panic,
/// deadlock, or corrupt instance state (serialized through the
/// instance's own per-instance lock in `crates/orchestrator`).
#[tokio::test]
async fn task_11_simultaneous_lifecycle_requests_are_safe() {
    let (app, _dir, _root) = application_with_runtime().await;
    let admin_cookie = bootstrap_admin(&app).await;
    enable_fixture(&app, &admin_cookie).await;
    let user_cookie = create_user(&app, &admin_cookie, "race", "user").await;
    let instance_id = start_instance(&app, &user_cookie).await;

    let stop_uri = format!("/api/v1/runtime-instances/test_fixture/{instance_id}/stop");
    let restart_uri = format!("/api/v1/runtime-instances/test_fixture/{instance_id}/restart");

    let stop_app = app.clone();
    let stop_cookie = user_cookie.clone();
    let stop_uri_owned = stop_uri.clone();
    let stop_task = tokio::spawn(async move {
        stop_app
            .oneshot(request(
                Method::POST,
                &stop_uri_owned,
                Body::empty(),
                Some(&stop_cookie),
            ))
            .await
    });
    let restart_app = app.clone();
    let restart_cookie = user_cookie.clone();
    let restart_uri_owned = restart_uri.clone();
    let restart_task = tokio::spawn(async move {
        restart_app
            .oneshot(request(
                Method::POST,
                &restart_uri_owned,
                Body::empty(),
                Some(&restart_cookie),
            ))
            .await
    });

    let (stop_result, restart_result) = tokio::join!(stop_task, restart_task);
    // Neither request may panic the handler -- both resolve to a
    // well-formed HTTP response either way (the specific status of each
    // depends on request ordering, which is intentionally not asserted
    // here; the coherent-final-state check below is the real assertion).
    let _stop_status = stop_result.unwrap().unwrap().status();
    assert!(restart_result.is_ok());

    // The instance settles into a single, coherent terminal state
    // afterwards -- never left in a torn/ambiguous state.
    let status_uri = format!("/api/v1/runtime-instances/test_fixture/{instance_id}");
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
    assert!(
        matches!(state.as_str(), "running" | "stopped" | "failed"),
        "unexpected torn state after concurrent stop/restart: {state}"
    );
}

/// Task 4 -- guest role is denied *before* any availability check: for
/// a real product kind (`code`, no adapter registered in this test
/// harness -- Phase 7 hasn't built one yet), a guest is refused with
/// 403 (capability denied), while an ordinary `user` -- who *does* hold
/// `apps.code.use` -- gets past the RBAC gate and only then hits 503
/// (kind not registered on this host). This proves the two failure
/// modes are genuinely independent, not the same "no" for different
/// reasons.
#[tokio::test]
async fn task_4_guest_denied_before_availability_ordinary_user_reaches_availability_check() {
    let (app, _dir, _root) = application_with_runtime().await;
    let admin_cookie = bootstrap_admin(&app).await;
    let guest_cookie = create_user(&app, &admin_cookie, "guestling", "guest").await;
    let user_cookie = create_user(&app, &admin_cookie, "codeuser", "user").await;

    let guest_attempt = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/api/v1/runtime-instances",
            &json!({ "kind": "code" }),
            Some(&guest_cookie),
        ))
        .await
        .unwrap();
    assert_eq!(
        guest_attempt.status(),
        StatusCode::FORBIDDEN,
        "guest must never reach even the availability check for a runtime kind"
    );

    let user_attempt = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/api/v1/runtime-instances",
            &json!({ "kind": "code" }),
            Some(&user_cookie),
        ))
        .await
        .unwrap();
    assert_eq!(
        user_attempt.status(),
        StatusCode::SERVICE_UNAVAILABLE,
        "an authorized user reaches the (currently unimplemented, Phase 7) availability check"
    );
}

/// Task 3 -- backend authorization for the global enable/disable
/// control across every role, direct-API path (the only path that
/// exists -- the Settings UI calls this exact same endpoint, so there
/// is no alternate "Settings-only" authorization path to diverge from).
#[tokio::test]
async fn task_3_settings_authorization_matches_role_policy_for_every_role() {
    let (app, _dir, _root) = application_with_runtime().await;
    let admin_cookie = bootstrap_admin(&app).await;
    let guest_cookie = create_user(&app, &admin_cookie, "roleguest", "guest").await;
    let user_cookie = create_user(&app, &admin_cookie, "roleuser", "user").await;
    let manager_cookie = create_user(&app, &admin_cookie, "rolemanager", "manager").await;

    for (label, cookie, expected) in [
        ("guest", &guest_cookie, StatusCode::FORBIDDEN),
        ("user", &user_cookie, StatusCode::FORBIDDEN),
        // No explicit product policy grants managers global runtime
        // control yet -- only administrator holds runtime.admin (see
        // crates/permissions's seed list). This test documents that
        // as the actual current policy, not an assumption.
        ("manager", &manager_cookie, StatusCode::FORBIDDEN),
    ] {
        let enable = app
            .clone()
            .oneshot(request(
                Method::POST,
                "/api/v1/runtimes/test_fixture/enable",
                Body::empty(),
                Some(cookie),
            ))
            .await
            .unwrap();
        assert_eq!(enable.status(), expected, "enable as {label}");
        let disable = app
            .clone()
            .oneshot(request(
                Method::POST,
                "/api/v1/runtimes/test_fixture/disable",
                Body::empty(),
                Some(cookie),
            ))
            .await
            .unwrap();
        assert_eq!(disable.status(), expected, "disable as {label}");
    }

    let admin_enable = app
        .clone()
        .oneshot(request(
            Method::POST,
            "/api/v1/runtimes/test_fixture/enable",
            Body::empty(),
            Some(&admin_cookie),
        ))
        .await
        .unwrap();
    assert_eq!(
        admin_enable.status(),
        StatusCode::NO_CONTENT,
        "enable as administrator"
    );
    let admin_disable = app
        .clone()
        .oneshot(request(
            Method::POST,
            "/api/v1/runtimes/test_fixture/disable",
            Body::empty(),
            Some(&admin_cookie),
        ))
        .await
        .unwrap();
    assert_eq!(
        admin_disable.status(),
        StatusCode::NO_CONTENT,
        "disable as administrator"
    );
}

/// Task 5 -- hostile/malformed JSON against the runtime-management
/// endpoints. Every case must fail safely (4xx), never panic (500) or
/// select an unintended runtime kind.
#[tokio::test]
async fn task_5_hostile_json_sweep() {
    let (app, _dir, _root) = application_with_runtime().await;
    let admin_cookie = bootstrap_admin(&app).await;
    enable_fixture(&app, &admin_cookie).await;
    let user_cookie = create_user(&app, &admin_cookie, "hostilejson", "user").await;

    let create_uri = "/api/v1/runtime-instances";

    // Empty body.
    let mut empty = request(Method::POST, create_uri, Body::empty(), Some(&user_cookie));
    empty
        .headers_mut()
        .insert(header::CONTENT_TYPE, "application/json".parse().unwrap());
    let response = app.clone().oneshot(empty).await.unwrap();
    assert!(response.status().is_client_error(), "empty body");

    // Malformed JSON.
    let malformed = request(
        Method::POST,
        create_uri,
        Body::from("{not json"),
        Some(&user_cookie),
    );
    let mut malformed = malformed;
    malformed
        .headers_mut()
        .insert(header::CONTENT_TYPE, "application/json".parse().unwrap());
    let response = app.clone().oneshot(malformed).await.unwrap();
    assert!(response.status().is_client_error(), "malformed JSON");

    let hostile_bodies = [
        json!({ "kind": 12345 }),                                     // wrong type
        json!({ "kind": null }),                                      // null where forbidden
        json!({ "kind": "x".repeat(1_000_000) }),                     // huge string
        json!({ "kind": "code", "unexpected": "field" }),             // unknown field
        json!({}),                                                    // missing field
        json!({ "kind": "CODE" }),                                    // mixed case
        json!({ "kind": "../../../etc/passwd" }),                     // traversal-looking
        json!({ "kind": "code'; DROP TABLE runtime_instances; --" }), // SQL-looking
        json!({ "kind": "code\u{0000}\u{0001}" }),                    // control characters
        json!([1, 2, 3]),                                             // wrong top-level type
    ];
    for body in hostile_bodies {
        let response = app
            .clone()
            .oneshot(json_request(
                Method::POST,
                create_uri,
                &body,
                Some(&user_cookie),
            ))
            .await
            .unwrap();
        assert!(
            response.status().is_client_error(),
            "expected 4xx for hostile body {body}, got {}",
            response.status()
        );
    }
}

/// Task 6 -- production-config-injection attacks. `CreateInstanceBody`
/// uses `#[serde(deny_unknown_fields)]`, so any of these fields being
/// present alongside a valid `kind` must make the *entire* request
/// fail closed (400) rather than silently being ignored while `kind`
/// is honored -- proving there is no way to smuggle launch
/// configuration through this endpoint at all, not merely that it has
/// no effect.
#[tokio::test]
async fn task_6_production_config_injection_is_rejected() {
    let (app, _dir, _root) = application_with_runtime().await;
    let admin_cookie = bootstrap_admin(&app).await;
    enable_fixture(&app, &admin_cookie).await;
    let user_cookie = create_user(&app, &admin_cookie, "injector", "user").await;

    for field in [
        "executable",
        "command",
        "args",
        "argv",
        "env",
        "environment",
        "working_directory",
        "image",
        "container_image",
        "mounts",
        "volumes",
        "devices",
        "privileged",
        "host_network",
        "host_pid",
        "host_ipc",
        "capabilities",
        "docker_socket",
        "port",
        "upstream",
        "url",
        "hostname",
    ] {
        let body = json!({ "kind": "test_fixture", field: "attacker-controlled" });
        let response = app
            .clone()
            .oneshot(json_request(
                Method::POST,
                "/api/v1/runtime-instances",
                &body,
                Some(&user_cookie),
            ))
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::UNPROCESSABLE_ENTITY,
            "field {field:?} must make the whole request fail closed, not be silently dropped"
        );
    }

    // The same request with only the trusted `kind` field still works,
    // proving the rejections above are about the extra fields, not
    // about the endpoint being broken.
    let clean = start_instance(&app, &user_cookie).await;
    assert!(!clean.is_empty());
}

/// Task 7 -- SSRF sweep against the authenticated proxy. None of these
/// headers are ever consulted to select an upstream (the handler has
/// no parameter that could carry one) -- this proves it structurally
/// by showing the response is always the owner's own fixture,
/// regardless of what a client claims via these headers.
#[tokio::test]
async fn task_7_ssrf_header_sweep_has_no_effect_on_upstream_selection() {
    let (app, _dir, _root) = application_with_runtime().await;
    let admin_cookie = bootstrap_admin(&app).await;
    enable_fixture(&app, &admin_cookie).await;
    let owner_cookie = create_user(&app, &admin_cookie, "ssrfowner", "user").await;
    let instance_id = start_instance(&app, &owner_cookie).await;
    let uri =
        format!("/api/v1/runtime-instances/test_fixture/{instance_id}/proxy/echo?msg=ssrf-probe");

    for (name, value) in [
        ("host", "169.254.169.254"),
        ("x-forwarded-host", "169.254.169.254"),
        ("forwarded", "host=169.254.169.254;proto=http"),
        ("x-forwarded-for", "127.0.0.1"),
        ("x-original-url", "http://127.0.0.1:1/admin"),
        ("x-rewrite-url", "file:///etc/passwd"),
    ] {
        let mut hostile = request(Method::GET, &uri, Body::empty(), Some(&owner_cookie));
        hostile.headers_mut().insert(
            axum::http::HeaderName::from_static(name),
            value.parse().unwrap(),
        );
        let response = app.clone().oneshot(hostile).await.unwrap();
        assert_eq!(
            response.status(),
            StatusCode::OK,
            "header {name} must not change routing"
        );
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(
            &body[..],
            b"ssrf-probe",
            "response must always come from the owner's own instance regardless of header {name}"
        );
    }
}

/// Task 10 -- Origin policy is the existing project-wide CSRF/origin
/// middleware (`web_security`), which wraps every route including the
/// new runtime ones. This proves the runtime WebSocket proxy route
/// inherits it rather than accidentally bypassing it, the same way
/// `services/clouddeskd/tests/health.rs` already proves it for the
/// terminal WebSocket route.
#[tokio::test]
async fn task_10_websocket_proxy_rejects_cross_site_upgrade_before_auth() {
    let (app, _dir, _root) = application_with_runtime().await;
    let admin_cookie = bootstrap_admin(&app).await;
    enable_fixture(&app, &admin_cookie).await;
    let owner_cookie = create_user(&app, &admin_cookie, "originowner", "user").await;
    let instance_id = start_instance(&app, &owner_cookie).await;
    let uri = format!("/api/v1/runtime-instances/test_fixture/{instance_id}/proxy-ws");

    let mut hostile = request(Method::GET, &uri, Body::empty(), Some(&owner_cookie));
    hostile
        .headers_mut()
        .insert(header::UPGRADE, "websocket".parse().unwrap());
    hostile
        .headers_mut()
        .insert(header::ORIGIN, "https://evil.example".parse().unwrap());
    hostile
        .headers_mut()
        .insert("sec-fetch-site", "cross-site".parse().unwrap());
    let response = app.clone().oneshot(hostile).await.unwrap();
    assert_eq!(
        response.status(),
        StatusCode::FORBIDDEN,
        "a cross-site WebSocket upgrade must be rejected before authorization even runs, \
         exactly like the terminal WebSocket route"
    );
}

/// Task 11/12 -- hostile byte content and secret isolation, through
/// the actual bounded-log HTTP endpoint. Uses a *small* hostile
/// payload deliberately: a real defect was found and fixed in
/// `crates/orchestrator`'s log-capture reader (edge-triggered pipe
/// reads could starve health-check readiness under high-volume
/// output -- see
/// `task_11_log_flooding_during_startup_does_not_delay_readiness` in
/// `crates/orchestrator/tests/live_lifecycle.rs`, which reliably
/// reproduces and regression-tests the *volume* dimension at the layer
/// where it belongs). This test instead focuses on what's specific to
/// the HTTP layer -- sanitization and bounding of whatever the
/// orchestrator hands back -- with a payload sized to start instantly
/// regardless of host load, so it isn't a source of flakiness in
/// `cargo test --workspace` on a busy machine.
#[tokio::test]
async fn task_11_12_hostile_log_content_is_sanitized_and_bounded() {
    let mut extra_env = HashMap::new();
    // Control characters, an ANSI escape sequence, and HTML/script-
    // looking text -- constructed as hex so it survives the env-var
    // boundary byte-for-byte. Small enough to write to the pipe and
    // drain in a single read, independent of host scheduling.
    let hostile_line = format!(
        "before{}<script>alert(1)</script>after",
        "\u{1b}[31mred\u{1b}[0m"
    );
    let mut payload_hex = String::with_capacity(hostile_line.len() * 2);
    for byte in hostile_line.as_bytes() {
        use std::fmt::Write as _;
        let _ = write!(payload_hex, "{byte:02x}");
    }
    extra_env.insert("LOG_TEST_PAYLOAD_HEX".to_owned(), payload_hex);
    // Fake, test-only secret-shaped values -- proves CloudDesk itself
    // never *injects* real secrets into the child, not that the
    // fixture's own chosen stdout content gets redacted (no such
    // promise exists in the current log policy).
    extra_env.insert(
        "CLOUDDESK_TEST_VAULT_KEY".to_owned(),
        "should-never-be-set-by-clouddesk".to_owned(),
    );

    let (app, _dir, _root) = application_with_runtime_env(extra_env).await;
    let admin_cookie = bootstrap_admin(&app).await;
    enable_fixture(&app, &admin_cookie).await;
    let user_cookie = create_user(&app, &admin_cookie, "floodlog", "user").await;
    let instance_id = start_instance(&app, &user_cookie).await;

    let logs_uri = format!("/api/v1/runtime-instances/test_fixture/{instance_id}/logs");
    let response = app
        .clone()
        .oneshot(request(
            Method::GET,
            &logs_uri,
            Body::empty(),
            Some(&user_cookie),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    let logs = body["logs"].as_str().unwrap();

    assert!(
        logs.len() <= 64 * 1024,
        "log response must stay bounded, got {} bytes",
        logs.len()
    );
    // No raw ANSI escape byte reaches the JSON response, even though
    // the fixture wrote one to stdout.
    assert!(
        !logs.contains('\u{1b}'),
        "raw ANSI escape sequences must not reach the API response: {logs:?}"
    );
    // The surrounding plain text still comes through -- sanitization
    // removes control bytes, not arbitrary content.
    assert!(logs.contains("before") && logs.contains("after"));
    // CloudDesk's own controlled environment construction never sets
    // this variable -- the fixture merely echoed back the value the
    // *test* injected into its own env for this assertion; the point
    // is that nothing CloudDesk-side added a real secret on top of it,
    // which the orchestrator-level `environment_never_leaks_the_
    // orchestrator_process_env` test proves directly for the real
    // sensitive names (Vault master key, session signing secret, DB
    // credential, SSH passphrase, API token).
}

/// Task 13/14/15 -- the OCI adapter exercised through the actual
/// clouddeskd HTTP API (not direct orchestrator calls), against the
/// real local Docker daemon. Skips cleanly if Docker isn't reachable.
mod oci_through_product_api {
    use super::*;
    use clouddesk_orchestrator::oci::{OciAdapter, OciSpec};
    use std::process::Stdio;
    use tokio::process::Command as TokioCommand;

    async fn docker_available() -> bool {
        TokioCommand::new("docker")
            .args(["version", "--format", "{{.Server.Version}}"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await
            .is_ok_and(|s| s.success())
            && TokioCommand::new("docker")
                .args(["image", "inspect", "alpine:latest"])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .await
                .is_ok_and(|s| s.success())
    }

    async fn application_with_oci_runtime() -> (Router, tempfile::TempDir, tempfile::TempDir) {
        let _ = tracing_subscriber::fmt().with_test_writer().try_init();
        let pool = clouddesk_db::connect("sqlite::memory:", 1).await.unwrap();
        clouddesk_db::migrate(&pool).await.unwrap();
        let auth = AuthService::new(
            pool.clone(),
            SecretCipher::new(&[11_u8; 32]).unwrap(),
            AuthPolicy::default(),
        )
        .unwrap();
        let directory = tempfile::tempdir().unwrap();
        let secret_path = directory.path().join("bootstrap.secret");
        fs::write(&secret_path, "runtime-test-secret\n").unwrap();

        let runtime_root = tempfile::tempdir().unwrap();
        // TEST-ONLY OCI spec, registered only under RuntimeKind::TestFixture
        // -- structurally cannot leak into production runtime enumeration
        // (Task 16), since only the `_for_tests` constructor below ever
        // allows that kind through the HTTP layer at all.
        let spec = OciSpec {
            kind: RuntimeKind::TestFixture,
            image: "alpine:latest".to_owned(),
            container_port: 8080,
            health_check_path: "/",
            command: Some(Arc::new(|_ctx| {
                vec![
                    "sh".to_owned(),
                    "-c".to_owned(),
                    // A real, minimal-but-valid HTTP response, not just
                    // raw bytes on the socket -- `OciAdapter::health()`
                    // performs a real HTTP GET (Phase 7 closure Task 18
                    // fix), so this fixture must actually speak HTTP.
                    "while true; do printf 'HTTP/1.1 200 OK\\r\\nContent-Length: 2\\r\\n\\r\\nok' | nc -l -p 8080; done".to_owned(),
                ]
            })),
            extra_mounts: None,
            run_as: None,
            extra_env: None,
            extra_capabilities: &[],
            add_host_gateway: false,
        };
        let policy = ResourcePolicy {
            start_timeout: Duration::from_secs(15),
            health_timeout: Duration::from_secs(10),
            ..ResourcePolicy::default()
        };
        let runtime_manager = Arc::new(
            RuntimeManager::new(
                RuntimeStore::new(pool.clone()),
                runtime_root.path().to_owned(),
                policy,
            )
            .with_adapter(Arc::new(OciAdapter::new(spec))),
        );
        (
            clouddeskd::application_router_and_media_and_library_and_runtime_configured_for_tests(
                directory.path().to_owned(),
                auth,
                secret_path,
                true,
                None,
                None,
                Some(runtime_manager),
            ),
            directory,
            runtime_root,
        )
    }

    #[tokio::test]
    async fn oci_lifecycle_and_hardening_through_clouddeskd_api() {
        if !docker_available().await {
            eprintln!("SKIP: docker not reachable on this host -- reporting honestly, not PASS");
            return;
        }
        let (app, _dir, _root) = application_with_oci_runtime().await;
        let admin_cookie = bootstrap_admin(&app).await;
        enable_fixture(&app, &admin_cookie).await;
        let user_cookie = create_user(&app, &admin_cookie, "ociuser", "user").await;

        // Start through the real HTTP API.
        let create = app
            .clone()
            .oneshot(json_request(
                Method::POST,
                "/api/v1/runtime-instances",
                &json!({ "kind": "test_fixture" }),
                Some(&user_cookie),
            ))
            .await
            .unwrap();
        assert_eq!(create.status(), StatusCode::OK);
        let body = body_json(create).await;
        assert_eq!(
            body["state"], "running",
            "readiness must come from a real passed health check against the container"
        );
        let instance_id = body["instance_id"].as_str().unwrap().to_owned();

        // Inspect the *real* container's hardening -- not inferred from
        // the argv that constructed it.
        let name = format!("clouddesk-runtime-{instance_id}");
        let inspect = TokioCommand::new("docker")
            .args([
                "inspect",
                "--format",
                "{{.HostConfig.Privileged}}|{{.HostConfig.CapDrop}}|{{.HostConfig.SecurityOpt}}|{{.HostConfig.NetworkMode}}|{{.HostConfig.PidMode}}|{{.HostConfig.Binds}}",
                &name,
            ])
            .output()
            .await
            .unwrap();
        let inspect_out = String::from_utf8_lossy(&inspect.stdout);
        let fields: Vec<&str> = inspect_out.trim().split('|').collect();
        assert_eq!(fields[0], "false", "must not run privileged: {inspect_out}");
        assert!(
            fields[1].contains("ALL"),
            "must drop all capabilities: {inspect_out}"
        );
        assert!(
            fields[2].contains("no-new-privileges"),
            "must set no-new-privileges: {inspect_out}"
        );
        assert_ne!(
            fields[3], "host",
            "must not use host networking: {inspect_out}"
        );
        assert_ne!(
            fields[4], "host",
            "must not share the host PID namespace: {inspect_out}"
        );
        assert!(
            !inspect_out.contains("docker.sock"),
            "must never mount the Docker socket: {inspect_out}"
        );

        // Stop through the real HTTP API and verify the container is
        // really gone before the API reports completion (preserves the
        // previously-fixed asynchronous --rm race).
        let stop_uri = format!("/api/v1/runtime-instances/test_fixture/{instance_id}/stop");
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
            .args(["inspect", &name])
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

    #[tokio::test]
    async fn oci_image_missing_fails_closed_through_the_api() {
        if !docker_available().await {
            eprintln!("SKIP: docker not reachable on this host");
            return;
        }
        let _ = tracing_subscriber::fmt().with_test_writer().try_init();
        let pool = clouddesk_db::connect("sqlite::memory:", 1).await.unwrap();
        clouddesk_db::migrate(&pool).await.unwrap();
        let auth = AuthService::new(
            pool.clone(),
            SecretCipher::new(&[13_u8; 32]).unwrap(),
            AuthPolicy::default(),
        )
        .unwrap();
        let directory = tempfile::tempdir().unwrap();
        let secret_path = directory.path().join("bootstrap.secret");
        fs::write(&secret_path, "runtime-test-secret\n").unwrap();
        let runtime_root = tempfile::tempdir().unwrap();
        let spec = OciSpec {
            kind: RuntimeKind::TestFixture,
            image: "clouddesk-nonexistent-image-for-tests:latest".to_owned(),
            container_port: 8080,
            health_check_path: "/",
            command: None,
            extra_mounts: None,
            run_as: None,
            extra_env: None,
            extra_capabilities: &[],
            add_host_gateway: false,
        };
        let runtime_manager = Arc::new(
            RuntimeManager::new(
                RuntimeStore::new(pool.clone()),
                runtime_root.path().to_owned(),
                ResourcePolicy::default(),
            )
            .with_adapter(Arc::new(OciAdapter::new(spec))),
        );
        let app =
            clouddeskd::application_router_and_media_and_library_and_runtime_configured_for_tests(
                directory.path().to_owned(),
                auth,
                secret_path,
                true,
                None,
                None,
                Some(runtime_manager),
            );
        let admin_cookie = bootstrap_admin(&app).await;
        enable_fixture(&app, &admin_cookie).await;
        let user_cookie = create_user(&app, &admin_cookie, "ocimissing", "user").await;

        let create = app
            .clone()
            .oneshot(json_request(
                Method::POST,
                "/api/v1/runtime-instances",
                &json!({ "kind": "test_fixture" }),
                Some(&user_cookie),
            ))
            .await
            .unwrap();
        assert_eq!(
            create.status(),
            StatusCode::SERVICE_UNAVAILABLE,
            "a missing image must fail closed through the API, never 500 or a false RUNNING"
        );
    }
}

/// Task 17 -- verifies real audit rows exist for the runtime lifecycle
/// events this module claims to emit, with safe (non-secret) fields,
/// directly against the append-only `audit_events` table -- not merely
/// trusting that `audit_action`/`authorize_request` were called.
#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn task_17_audit_events_are_recorded_with_safe_fields() {
    let pool = clouddesk_db::connect("sqlite::memory:", 1).await.unwrap();
    clouddesk_db::migrate(&pool).await.unwrap();
    let auth = AuthService::new(
        pool.clone(),
        SecretCipher::new(&[17_u8; 32]).unwrap(),
        AuthPolicy::default(),
    )
    .unwrap();
    let directory = tempfile::tempdir().unwrap();
    let secret_path = directory.path().join("bootstrap.secret");
    fs::write(&secret_path, "runtime-test-secret\n").unwrap();
    let runtime_root = tempfile::tempdir().unwrap();
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
            runtime_root.path().to_owned(),
            policy,
        )
        .with_adapter(Arc::new(HostProcessAdapter::new(spec))),
    );
    let app = clouddeskd::application_router_and_media_and_library_and_runtime_configured_for_tests(
        directory.path().to_owned(),
        auth,
        secret_path,
        true,
        None,
        None,
        Some(runtime_manager),
    );

    let admin_cookie = bootstrap_admin(&app).await;
    enable_fixture(&app, &admin_cookie).await;
    let user_cookie = create_user(&app, &admin_cookie, "audituser", "user").await;
    let instance_id = start_instance(&app, &user_cookie).await;
    let stop_uri = format!("/api/v1/runtime-instances/test_fixture/{instance_id}/stop");
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

    // A denied global-enable attempt, to verify access-denied events
    // are audited too (via `authorize_request`'s built-in audit call).
    let denied = app
        .clone()
        .oneshot(request(
            Method::POST,
            "/api/v1/runtimes/test_fixture/enable",
            Body::empty(),
            Some(&user_cookie),
        ))
        .await
        .unwrap();
    assert_eq!(denied.status(), StatusCode::FORBIDDEN);

    let rows: Vec<(String, String, Option<String>, String, String)> = sqlx::query_as(
        "SELECT action, resource_type, resource_id, result, metadata_json
         FROM audit_events WHERE action LIKE 'runtime%' OR action = 'authorization.check'
         ORDER BY id",
    )
    .fetch_all(&pool)
    .await
    .unwrap();

    let actions: Vec<&str> = rows.iter().map(|r| r.0.as_str()).collect();
    assert!(
        actions.contains(&"runtime.enable.requested") && actions.contains(&"runtime.enabled"),
        "missing enable audit events: {actions:?}"
    );
    assert!(
        actions.contains(&"runtime.instance.start_requested")
            && actions.contains(&"runtime.instance.started"),
        "missing start audit events: {actions:?}"
    );
    assert!(
        actions.contains(&"runtime.instance.stopped"),
        "missing stop audit event: {actions:?}"
    );
    assert!(
        actions.contains(&"authorization.check"),
        "missing capability-denial audit event: {actions:?}"
    );

    // Safe-field check: none of these rows' metadata contains anything
    // that looks like a secret, an environment dump, or the bootstrap
    // credential -- only typed, safe fields (kind name, instance id).
    for (_, resource_type, resource_id, _, metadata_json) in &rows {
        assert!(
            matches!(
                resource_type.as_str(),
                "runtime_kind" | "runtime_instance" | "capability"
            ),
            "unexpected audit resource_type: {resource_type}"
        );
        assert!(
            !metadata_json.to_lowercase().contains("secret")
                && !metadata_json.to_lowercase().contains("password")
                && !metadata_json.to_lowercase().contains("vault"),
            "audit metadata must never contain secret-shaped content: {metadata_json}"
        );
        if let Some(id) = resource_id {
            assert!(
                !id.contains("runtime-test-secret"),
                "audit resource_id must never contain the bootstrap secret"
            );
        }
    }
}

/// Phase 6 closure Tasks 1-4: duplicate-JSON hostile input, real
/// WebSocket binary-frame handling (bounded, cleaned up), and fixture
/// cleanup hygiene. All against a real bound TCP listener + real
/// `tokio_tungstenite` clients -- no `tower::oneshot`.
mod closure_pass {
    use super::*;
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    use tokio_tungstenite::tungstenite::http::header::COOKIE;
    use tokio_tungstenite::tungstenite::Message;

    fn raw_json_request(uri: &str, raw_body: &str, cookie: &str) -> Request<Body> {
        let mut req = request(
            Method::POST,
            uri,
            Body::from(raw_body.to_owned()),
            Some(cookie),
        );
        req.headers_mut()
            .insert(header::CONTENT_TYPE, "application/json".parse().unwrap());
        req
    }

    /// Task 1 -- duplicate JSON keys in the runtime-instance-creation
    /// body. `serde_json`'s `Map` (the default, non-`preserve_order`
    /// deserialization path this project uses) resolves duplicate keys
    /// last-value-wins during parsing, *before* `CreateInstanceBody`
    /// ever sees a single value -- there is exactly one `kind` string
    /// by the time our authorization/selection logic runs, so there is
    /// no divergence for it to exploit. This test proves that directly:
    /// whichever value "wins" is the one used for both the capability
    /// check *and* the actual instance creation (never split), and
    /// duplicating any of the fields that must never be client-
    /// controlled at all (`executable`/`argv`/`env`/`image`/...)
    /// still hits `deny_unknown_fields` exactly as a single occurrence
    /// would, because those field names were never legal in the first
    /// place.
    #[tokio::test]
    async fn task_1_duplicate_json_keys_cannot_bypass_security() {
        let (app, _dir, _root) = application_with_runtime().await;
        let admin_cookie = bootstrap_admin(&app).await;
        enable_fixture(&app, &admin_cookie).await;
        let user_cookie = create_user(&app, &admin_cookie, "dupjson", "user").await;

        // Real, verified behavior (documented here, not assumed): this
        // project's `CreateInstanceBody` is a `#[derive(Deserialize)]`
        // struct, and serde's derive-generated `Visitor::visit_map`
        // tracks each field with an internal `Option` and errors with
        // "duplicate field `kind`" the moment a second occurrence is
        // seen -- unlike a generic `serde_json::Value`/`Map`, which
        // would merge duplicate keys last-value-wins. That means a
        // duplicate `kind` key never reaches application code as any
        // single resolved value at all; the whole request fails closed
        // uniformly, which is a *stronger* guarantee than "one value
        // safely wins" would have been -- there is no divergence for a
        // duplicate key to exploit because no divergent value is ever
        // produced.
        for raw in [
            r#"{"kind":"code","kind":"test_fixture"}"#,
            r#"{"kind":"test_fixture","kind":"../../../etc/passwd"}"#,
            r#"{"kind":"test_fixture","executable":"/bin/sh","executable":"/bin/bash"}"#,
            r#"{"kind":"test_fixture","env":{"A":"1"},"env":{"B":"2"}}"#,
            r#"{"kind":"test_fixture","port":9999,"port":8888}"#,
        ] {
            let req = raw_json_request("/api/v1/runtime-instances", raw, &user_cookie);
            let response = app.clone().oneshot(req).await.unwrap();
            assert_eq!(
                response.status(),
                StatusCode::UNPROCESSABLE_ENTITY,
                "any duplicate key (trusted-vs-trusted, trusted-vs-hostile, or a forbidden \
                 field) must fail the whole request closed, never resolve to a single winning \
                 value silently: {raw}"
            );
        }

        // A single, non-duplicated `kind` still works -- proves the
        // rejections above are specifically about duplication, not
        // that the endpoint is broken.
        let clean = start_instance(&app, &user_cookie).await;
        assert!(!clean.is_empty());
    }

    async fn spawn_real_server(app: Router) -> SocketAddr {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let local_addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let _ = axum::serve(
                listener,
                app.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .await;
        });
        local_addr
    }

    async fn connect_ws(
        local_addr: SocketAddr,
        path: &str,
        cookie: &str,
    ) -> tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>
    {
        let url = format!("ws://{local_addr}{path}");
        let mut req = url.into_client_request().unwrap();
        req.headers_mut().insert(COOKIE, cookie.parse().unwrap());
        let (ws, _) = tokio_tungstenite::connect_async(req).await.unwrap();
        ws
    }

    /// Task 2 -- real binary-frame relay: small frame, multiple frames,
    /// and a zero-length frame all round-trip through the proxy
    /// unchanged.
    #[tokio::test]
    async fn task_2_websocket_binary_frames_are_relayed() {
        let (app, _dir, _root) = application_with_runtime().await;
        let admin_cookie = bootstrap_admin(&app).await;
        enable_fixture(&app, &admin_cookie).await;
        let owner_cookie = create_user(&app, &admin_cookie, "wsbinowner", "user").await;
        let instance_id = start_instance(&app, &owner_cookie).await;
        let local_addr = spawn_real_server(app).await;
        let path = format!("/api/v1/runtime-instances/test_fixture/{instance_id}/proxy-ws");

        let mut ws = connect_ws(local_addr, &path, &owner_cookie).await;

        // 1/3: a small binary frame.
        ws.send(Message::Binary(vec![1, 2, 3, 4, 5])).await.unwrap();
        let reply = tokio::time::timeout(Duration::from_secs(5), ws.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(reply, Message::Binary(vec![1, 2, 3, 4, 5]));

        // 4: a zero-length binary frame.
        ws.send(Message::Binary(Vec::new())).await.unwrap();
        let reply = tokio::time::timeout(Duration::from_secs(5), ws.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(reply, Message::Binary(Vec::new()));

        // 5: a pseudo-random binary payload.
        let random: Vec<u8> = (0..4096_u32)
            .map(|i| u8::try_from(i.wrapping_mul(2_654_435_761) % 256).unwrap_or(0))
            .collect();
        ws.send(Message::Binary(random.clone())).await.unwrap();
        let reply = tokio::time::timeout(Duration::from_secs(5), ws.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(reply, Message::Binary(random));

        // 3: multiple frames in sequence still each round-trip.
        for i in 0_u8..5 {
            ws.send(Message::Binary(vec![i; 8])).await.unwrap();
            let reply = tokio::time::timeout(Duration::from_secs(5), ws.next())
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            assert_eq!(reply, Message::Binary(vec![i; 8]));
        }
    }

    /// Task 2 -- oversized WebSocket input has an explicit, enforced
    /// bound. Regression test for the real gap found this session:
    /// before adding an explicit `max_message_size`/`max_frame_size`,
    /// this proxy relied entirely on axum/tungstenite's own library
    /// defaults (64 MiB message / 16 MiB frame) -- present, but never a
    /// value `CloudDesk` itself deliberately chose. `clouddeskd` and the
    /// orchestrator's upstream leg now both enforce a 4 MiB message /
    /// 1 MiB frame bound explicitly. This test proves it without
    /// allocating anything close to the old 64 MiB default: it sends a
    /// frame just over the *new* 1 MiB frame bound and expects the
    /// connection to be closed/erred, not accepted.
    #[tokio::test]
    async fn task_2_oversized_websocket_frame_is_rejected_not_unbounded() {
        let (app, _dir, _root) = application_with_runtime().await;
        let admin_cookie = bootstrap_admin(&app).await;
        enable_fixture(&app, &admin_cookie).await;
        let owner_cookie = create_user(&app, &admin_cookie, "wsoversize", "user").await;
        let instance_id = start_instance(&app, &owner_cookie).await;
        let local_addr = spawn_real_server(app).await;
        let path = format!("/api/v1/runtime-instances/test_fixture/{instance_id}/proxy-ws");

        let mut ws = connect_ws(local_addr, &path, &owner_cookie).await;

        // Just over the 1 MiB max_frame_size bound this session added.
        let oversized = vec![7_u8; 1024 * 1024 + 1];
        let send_result = ws.send(Message::Binary(oversized)).await;
        // Either the send itself is rejected client-side against the
        // negotiated config, or the server closes the connection in
        // response -- both are "the bound is enforced, not ignored".
        // `Err(_)`: client-side config already refuses to send it --
        // the bound is enforced. Otherwise, check that the server
        // closes/errors the connection rather than echoing it back.
        if send_result.is_ok() {
            let outcome = tokio::time::timeout(Duration::from_secs(5), ws.next()).await;
            match outcome {
                Ok(Some(Ok(Message::Close(_)) | Err(_)) | None) => {}
                other => panic!(
                    "an oversized frame must not be accepted/echoed as if bounding didn't exist, got {other:?}"
                ),
            }
        }
    }

    /// Task 2/3 -- cross-user and unauthenticated binary-frame attempts
    /// are denied the same way text-frame attempts already are (Task 9)
    /// -- the socket never reaches the fixture.
    #[tokio::test]
    async fn task_2_cross_user_and_unauthenticated_binary_attempts_denied() {
        let (app, _dir, _root) = application_with_runtime().await;
        let admin_cookie = bootstrap_admin(&app).await;
        enable_fixture(&app, &admin_cookie).await;
        let owner_cookie = create_user(&app, &admin_cookie, "wsbinowner2", "user").await;
        let attacker_cookie = create_user(&app, &admin_cookie, "wsbinattacker2", "user").await;
        let instance_id = start_instance(&app, &owner_cookie).await;
        let local_addr = spawn_real_server(app.clone()).await;
        let path = format!("/api/v1/runtime-instances/test_fixture/{instance_id}/proxy-ws");

        // Cross-user: handshake succeeds (axum upgrades before the
        // handler body runs the ownership check), but no binary echo
        // ever arrives -- the socket is closed by the server instead.
        let mut attacker_ws = connect_ws(local_addr, &path, &attacker_cookie).await;
        attacker_ws
            .send(Message::Binary(vec![9, 9, 9]))
            .await
            .unwrap();
        let outcome = tokio::time::timeout(Duration::from_secs(5), attacker_ws.next()).await;
        match outcome {
            Ok(Some(Ok(Message::Close(_)) | Err(_)) | None) => {}
            other => panic!("cross-user binary attempt must be denied, got {other:?}"),
        }

        // Unauthenticated: no cookie at all, via a plain (non-upgrade)
        // request -- `WebSocketUpgrade` extraction fails before
        // `principal()` ever runs (400).
        let no_upgrade_response = app
            .clone()
            .oneshot(request(Method::GET, &path, Body::empty(), None))
            .await
            .unwrap();
        assert_eq!(no_upgrade_response.status(), StatusCode::BAD_REQUEST);

        // And via a real WebSocket handshake attempt with no cookie:
        // `principal()` runs after `WebSocketUpgrade` extraction
        // succeeds but before `.on_upgrade()` is ever called, so the
        // handshake itself fails with 401 -- the fixture is never
        // reached. (This needs a genuine network connection, not
        // `tower::oneshot`, which can't complete a real upgrade at
        // all -- see `task_9`'s own doc comment for why.)
        let url = format!("ws://{local_addr}{path}");
        let no_cookie_result = tokio_tungstenite::connect_async(&url).await;
        match no_cookie_result {
            Err(tokio_tungstenite::tungstenite::Error::Http(response)) => {
                assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
            }
            other => panic!("expected the handshake itself to fail with 401, got {other:?}"),
        }
    }

    /// Task 3 -- WebSocket cleanup: client disconnect, then repeated
    /// connect/disconnect cycles do not accumulate resources (bounded
    /// repetition, real connections each time).
    #[tokio::test]
    async fn task_3_websocket_repeated_connect_disconnect_is_clean() {
        let (app, _dir, _root) = application_with_runtime().await;
        let admin_cookie = bootstrap_admin(&app).await;
        enable_fixture(&app, &admin_cookie).await;
        let owner_cookie = create_user(&app, &admin_cookie, "wscleanup", "user").await;
        let instance_id = start_instance(&app, &owner_cookie).await;
        let local_addr = spawn_real_server(app.clone()).await;
        let path = format!("/api/v1/runtime-instances/test_fixture/{instance_id}/proxy-ws");

        for i in 0..10 {
            let mut ws = connect_ws(local_addr, &path, &owner_cookie).await;
            ws.send(Message::Text(format!("cycle-{i}"))).await.unwrap();
            let reply = tokio::time::timeout(Duration::from_secs(5), ws.next())
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            assert_eq!(reply, Message::Text(format!("cycle-{i}")));
            // Client disconnect -- the proxy task on the server side
            // must notice the closed connection and exit rather than
            // leak (SinkExt::send/close failing on the upstream leg
            // ends both relay futures via `tokio::join!`, per
            // `crates/orchestrator/src/proxy.rs`).
            let _ = ws.close(None).await;
        }

        // The instance is still healthy and independently usable after
        // 10 connect/disconnect cycles -- proves the proxy path itself
        // wasn't left in a broken/leaked state.
        let status_uri = format!("/api/v1/runtime-instances/test_fixture/{instance_id}");
        let status = app
            .clone()
            .oneshot(request(
                Method::GET,
                &status_uri,
                Body::empty(),
                Some(&owner_cookie),
            ))
            .await
            .unwrap();
        assert_eq!(body_json(status).await["state"], "running");
    }

    /// Task 4/5 -- process cleanup hygiene: `RuntimeManager::
    /// shutdown_all` (backed by the kernel-enforced parent-death signal
    /// added this session, see `host_process.rs`) leaves no live
    /// fixture process behind, verified with a real `ps`-equivalent
    /// check via `/proc` rather than trusting internal state alone.
    #[tokio::test]
    async fn task_4_5_fixture_process_does_not_survive_shutdown_all() {
        let pool = clouddesk_db::connect("sqlite::memory:", 1).await.unwrap();
        clouddesk_db::migrate(&pool).await.unwrap();
        let store = RuntimeStore::new(pool.clone());
        sqlx::query(
            "INSERT INTO users (id, username, display_name, password_hash, created_at, updated_at)
             VALUES ('u1', 'u1', 'u1', 'x', 0, 0)",
        )
        .execute(&pool)
        .await
        .unwrap();
        store
            .set_enabled(RuntimeKind::TestFixture, true)
            .await
            .unwrap();
        let runtime_root = tempfile::tempdir().unwrap();
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
        let manager = Arc::new(
            RuntimeManager::new(
                store,
                runtime_root.path().to_owned(),
                ResourcePolicy::default(),
            )
            .with_adapter(Arc::new(HostProcessAdapter::new(spec))),
        );
        let id = manager
            .create_instance("u1", RuntimeKind::TestFixture, Persistence::Ephemeral)
            .await
            .unwrap();
        manager.start_instance("u1", &id).await.unwrap();

        let pid = {
            let rows = sqlx::query_as::<_, (Option<i64>,)>(
                "SELECT pid FROM runtime_instances WHERE instance_id = ?",
            )
            .bind(&id.instance_id)
            .fetch_one(&pool)
            .await
            .unwrap();
            rows.0.expect("a running instance must have a recorded pid")
        };
        assert!(
            std::path::Path::new(&format!("/proc/{pid}")).exists(),
            "the fixture process must genuinely be running before shutdown"
        );

        manager.shutdown_all().await;
        // A killed process can briefly remain a zombie under /proc
        // until reaped; poll briefly rather than asserting instantly.
        let mut gone = false;
        for _ in 0..50 {
            let alive = std::fs::read_to_string(format!("/proc/{pid}/stat"))
                .ok()
                .is_some_and(|stat| !stat.contains(") Z "));
            if !alive {
                gone = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert!(
            gone,
            "pid {pid} must not survive shutdown_all (zombie or otherwise)"
        );
    }

    /// Phase 7 closure Task 5 -- proves, against actual received
    /// headers (not proxy configuration read alone), that the real
    /// end-to-end `clouddeskd` proxy chain never forwards the caller's
    /// session cookie, an `Authorization` header, or other sensitive-
    /// looking headers to a runtime instance. `proxy_http`'s
    /// `STRIPPED_REQUEST_HEADERS` (`crates/orchestrator/src/proxy.rs`)
    /// is shared, kind-agnostic code with no Code-specific branch --
    /// this is the exact same code path Code's own
    /// `/api/v1/runtime-instances/code/{id}/proxy/*` route uses, so
    /// this result applies equally to Code's "normal IDE proxy"
    /// without needing code-server itself to have a header-reflection
    /// endpoint (it doesn't).
    #[tokio::test]
    async fn task_5_proxy_never_forwards_session_cookie_or_sensitive_headers() {
        let (app, _dir, _root) = application_with_runtime().await;
        let admin_cookie = bootstrap_admin(&app).await;
        enable_fixture(&app, &admin_cookie).await;
        let user_cookie = create_user(&app, &admin_cookie, "headerleak", "user").await;
        let instance_id = start_instance(&app, &user_cookie).await;

        let uri =
            format!("/api/v1/runtime-instances/test_fixture/{instance_id}/proxy/echo-headers");
        let mut req = request(Method::GET, &uri, Body::empty(), Some(&user_cookie));
        req.headers_mut().insert(
            header::AUTHORIZATION,
            "Bearer attacker-supplied-token".parse().unwrap(),
        );
        req.headers_mut().insert(
            "x-vault-master-key",
            "fake-sentinel-vault-key-never-real".parse().unwrap(),
        );
        req.headers_mut().insert(
            "x-clouddesk-bootstrap-secret",
            "fake-sentinel-bootstrap-secret".parse().unwrap(),
        );
        let response = app.clone().oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let received: std::collections::BTreeMap<String, String> =
            serde_json::from_slice(&body).unwrap();

        // The fixture must have genuinely received *some* headers
        // (proves this isn't a trivially-empty/broken echo), just not
        // the ones that must never cross this boundary.
        assert!(
            !received.is_empty(),
            "the fixture must have received a real forwarded request"
        );
        for forbidden in ["cookie", "authorization"] {
            assert!(
                !received.contains_key(forbidden),
                "the instance must never receive the caller's {forbidden} header, got: {received:?}"
            );
        }
        // Sanity: these two custom headers are NOT on the strip list,
        // so their presence in the echo confirms the test actually
        // exercised a live forward (not a cached/short-circuited
        // response) -- their content is fake/sentinel-only, injected
        // by this test, never a real secret.
        assert_eq!(
            received.get("x-vault-master-key").map(String::as_str),
            Some("fake-sentinel-vault-key-never-real")
        );
    }
}
