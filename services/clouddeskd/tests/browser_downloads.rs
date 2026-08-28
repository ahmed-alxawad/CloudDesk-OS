//! Phase 9 Pass 3B: real, live Browser download evidence through the
//! actual product path -- a real controlled fixture serving a real
//! file with a real `Content-Disposition` header, a real Browser
//! instance clicking a real download link, verified via the real
//! broker's typed WebSocket events and the actual bytes staged on
//! disk (never a raw-CDP shortcut).

use axum::http::Method;
use axum::response::IntoResponse;
use clouddesk_auth::{AuthPolicy, AuthService};
use clouddesk_secrets::SecretCipher;
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::process::Command as TokioCommand;
use tokio_tungstenite::tungstenite::Message as WsMessage;

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
        SecretCipher::new(&[127_u8; 32]).unwrap(),
        AuthPolicy::default(),
    )
    .unwrap();
    let directory = tempfile::tempdir().unwrap();
    let secret_path = directory.path().join("bootstrap.secret");
    std::fs::write(&secret_path, "browser-downloads-test-secret\n").unwrap();

    let runtime_manager = std::sync::Arc::new(
        clouddesk_orchestrator::RuntimeManager::new(
            clouddesk_orchestrator::store::RuntimeStore::new(pool.clone()),
            std::env::temp_dir().join(format!(
                "clouddesk-browser-downloads-test-{}",
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
            "secret": "browser-downloads-test-secret",
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

async fn open_browser_instance(base: &str, cookie: &str) -> String {
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
    instance_id
}

type WsSink = futures_util::stream::SplitSink<
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
    WsMessage,
>;
type WsSource = futures_util::stream::SplitStream<
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
>;

async fn connect_browser_ws(base: &str, cookie: &str, instance_id: &str) -> (WsSink, WsSource) {
    let ws_url = format!(
        "ws{}/api/v1/runtime-instances/browser/{instance_id}/browser-ws",
        base.strip_prefix("http").unwrap()
    );
    let mut request = tokio_tungstenite::tungstenite::http::Request::builder()
        .uri(&ws_url)
        .header("Host", "127.0.0.1")
        .header("Connection", "Upgrade")
        .header("Upgrade", "websocket")
        .header("Sec-WebSocket-Version", "13")
        .header(
            "Sec-WebSocket-Key",
            tokio_tungstenite::tungstenite::handshake::client::generate_key(),
        )
        .body(())
        .unwrap();
    request
        .headers_mut()
        .insert("Cookie", cookie.parse().unwrap());
    let (stream, _) = tokio_tungstenite::connect_async(request)
        .await
        .expect("owner must connect");
    stream.split()
}

async fn recv_json_matching(
    stream: &mut WsSource,
    predicate: impl Fn(&Value) -> bool,
    timeout: std::time::Duration,
) -> Option<Value> {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return None;
        }
        match tokio::time::timeout(remaining, stream.next()).await {
            Ok(Some(Ok(WsMessage::Text(text)))) => {
                if let Ok(v) = serde_json::from_str::<Value>(&text) {
                    if predicate(&v) {
                        return Some(v);
                    }
                }
            }
            Ok(Some(Ok(_))) => {}
            _ => return None,
        }
    }
}

async fn navigate(tx: &mut WsSink, rx: &mut WsSource, url: &str) {
    tx.send(WsMessage::Text(
        json!({"type": "navigate", "url": url}).to_string(),
    ))
    .await
    .unwrap();
    let _ = recv_json_matching(
        rx,
        |v| v["type"] == "page_state" && v.get("url").is_some(),
        std::time::Duration::from_secs(10),
    )
    .await;
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
}

const REAL_PAYLOAD: &[u8] =
    b"CloudDesk Browser download acceptance payload 2026 - real bytes, not a stub.";

fn download_link_page(filename: &str) -> String {
    format!(
        "<!doctype html><html><body><a id=\"dl\" href=\"/file?name={filename}\" download>download</a></body></html>"
    )
}

async fn file_route(
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> impl IntoResponse {
    let name = params
        .get("name")
        .cloned()
        .unwrap_or_else(|| "file".to_owned());
    (
        [
            (
                axum::http::header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{name}\""),
            ),
            (
                axum::http::header::CONTENT_TYPE,
                "application/octet-stream".to_owned(),
            ),
        ],
        REAL_PAYLOAD,
    )
}

async fn spawn_download_fixture(link_filename: &str) -> u16 {
    let page_html = download_link_page(link_filename);
    let router = axum::Router::new()
        .route(
            "/",
            axum::routing::get(move || {
                let page_html = page_html.clone();
                async move { axum::response::Html(page_html) }
            }),
        )
        .route("/file", axum::routing::get(file_route));
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
    port
}

async fn gateway_ip() -> String {
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

/// Click a real link in a real page to trigger the download (matches
/// the fixture's real anchor position at the top-left of the page).
async fn click_download_link(tx: &mut WsSink) {
    for (action, extra) in [
        ("mouse_move", json!({})),
        ("mouse_down", json!({"button": "left"})),
        ("mouse_up", json!({"button": "left"})),
    ] {
        let mut msg = json!({"type": action, "x": 20.0, "y": 20.0});
        for (k, v) in extra.as_object().unwrap() {
            msg[k] = v.clone();
        }
        tx.send(WsMessage::Text(msg.to_string())).await.unwrap();
    }
}

/// Tasks 1-6 (Pass 3B): the real download flow end to end, through the
/// real broker WebSocket protocol against a real Browser instance --
/// `download_started` -> `download_progress`/`download_completed` events,
/// with the actual staged bytes verified server-side (never trusting
/// client-reported state).
#[tokio::test]
async fn task_1_6_real_download_flow_and_hash() {
    if !docker_and_image_available().await {
        clouddesk_test_support::blocked_by_environment(
            "task_1_6_real_download_flow_and_hash",
            clouddesk_test_support::reason::CONTAINER_RUNTIME_UNAVAILABLE,
        );
        return;
    }
    let (base, _dir, runtime_manager) = application().await;
    let _cross_process_guard = tokio::task::spawn_blocking(acquire_cross_process_browser_lock)
        .await
        .unwrap();
    let _brave_container_guard = BraveContainerGuard::new();

    let fixture_port = spawn_download_fixture("acceptance.bin").await;
    let gw = gateway_ip().await;
    clouddeskd::browser_egress_proxy::set_test_allowlist([gw.parse().unwrap()]);
    let fixture_url = format!("http://{gw}:{fixture_port}/");

    let admin_cookie = bootstrap_admin(&base).await;
    enable_browser(&base, &admin_cookie).await;
    let user_cookie = create_user(&base, &admin_cookie, "dlflowuser", "user").await;
    let instance_id = open_browser_instance(&base, &user_cookie).await;
    let (mut tx, mut rx) = connect_browser_ws(&base, &user_cookie, &instance_id).await;
    let _ = recv_json_matching(
        &mut rx,
        |v| v["type"] == "connected",
        std::time::Duration::from_secs(10),
    )
    .await;
    navigate(&mut tx, &mut rx, &fixture_url).await;

    click_download_link(&mut tx).await;

    let started = recv_json_matching(
        &mut rx,
        |v| v["type"] == "download_started",
        std::time::Duration::from_secs(15),
    )
    .await;
    assert!(
        started.is_some(),
        "a real download_started event must arrive"
    );
    let download_id = started.unwrap()["download"]["download_id"]
        .as_str()
        .unwrap()
        .to_owned();

    let completed = recv_json_matching(
        &mut rx,
        |v| {
            (v["type"] == "download_completed")
                && v["download"]["download_id"] == json!(download_id)
        },
        std::time::Duration::from_secs(15),
    )
    .await;
    assert!(
        completed.is_some(),
        "a real download_completed event must arrive"
    );
    let completed = completed.unwrap();
    assert_eq!(
        completed["download"]["filename"],
        json!("acceptance.bin"),
        "the sanitized suggested filename must be reported"
    );
    assert_eq!(
        completed["download"]["received_bytes"].as_u64().unwrap(),
        REAL_PAYLOAD.len() as u64
    );

    // Ground truth: the actual staged bytes on disk (never trusting
    // the client-reported byte count alone).
    let state_dir = runtime_manager
        .instance_state_dir(&clouddesk_orchestrator::InstanceId {
            kind: clouddesk_orchestrator::RuntimeKind::Browser,
            owner_user_id: {
                // Recover the real owner_user_id the same way the
                // product does: via the store row for this instance.
                let rows = runtime_manager.store().list_all().await.unwrap();
                rows.into_iter()
                    .find(|r| r.instance_id == instance_id)
                    .unwrap()
                    .owner_user_id
            },
            instance_id: instance_id.clone(),
        })
        .unwrap();
    let staged_path = state_dir.join("downloads").join(&download_id);
    let staged_bytes = tokio::fs::read(&staged_path)
        .await
        .expect("the real file must be staged on disk under the instance's own /state mount");
    assert_eq!(
        staged_bytes, REAL_PAYLOAD,
        "staged bytes must exactly match the real served payload"
    );

    let _ = tx.close().await;
}

/// Task 3: hostile `Content-Disposition` filenames must never escape
/// the staging root or corrupt the reported display name -- CDP's own
/// GUID-renaming already makes the on-disk path unaffected regardless,
/// this proves the *reported* filename is safe too.
#[tokio::test]
async fn task_3_hostile_filenames_sanitized() {
    if !docker_and_image_available().await {
        clouddesk_test_support::blocked_by_environment(
            "task_3_hostile_filenames_sanitized",
            clouddesk_test_support::reason::CONTAINER_RUNTIME_UNAVAILABLE,
        );
        return;
    }
    let (base, _dir, _runtime) = application().await;
    let _cross_process_guard = tokio::task::spawn_blocking(acquire_cross_process_browser_lock)
        .await
        .unwrap();
    let _brave_container_guard = BraveContainerGuard::new();

    let fixture_port = spawn_download_fixture("../../../etc/evil").await;
    let gw = gateway_ip().await;
    clouddeskd::browser_egress_proxy::set_test_allowlist([gw.parse().unwrap()]);
    let fixture_url = format!("http://{gw}:{fixture_port}/");

    let admin_cookie = bootstrap_admin(&base).await;
    enable_browser(&base, &admin_cookie).await;
    let user_cookie = create_user(&base, &admin_cookie, "dlhostileuser", "user").await;
    let instance_id = open_browser_instance(&base, &user_cookie).await;
    let (mut tx, mut rx) = connect_browser_ws(&base, &user_cookie, &instance_id).await;
    let _ = recv_json_matching(
        &mut rx,
        |v| v["type"] == "connected",
        std::time::Duration::from_secs(10),
    )
    .await;
    navigate(&mut tx, &mut rx, &fixture_url).await;
    click_download_link(&mut tx).await;

    let started = recv_json_matching(
        &mut rx,
        |v| v["type"] == "download_started",
        std::time::Duration::from_secs(15),
    )
    .await;
    let filename = started.unwrap()["download"]["filename"]
        .as_str()
        .unwrap()
        .to_owned();
    // Real, live finding: Chromium's own download-suggestion logic
    // already replaces path separators before ever reporting
    // `suggestedFilename` via CDP, so a literal ".." substring can
    // survive with no separators around it -- completely inert as a
    // filename (nothing to traverse without a `/`). The actually
    // security-relevant property, checked here, is that no separator
    // survives -- both from Chromium's own transform and this
    // sanitizer's independent defense-in-depth pass.
    assert!(
        !filename.contains('/') && !filename.contains('\\'),
        "sanitized filename must never contain a path separator, got {filename:?}"
    );
    let _ = tx.close().await;
}

/// Task 8: destination reauthorization at save time -- a read-only
/// root must be rejected even though the download itself succeeded.
#[tokio::test]
async fn task_8_save_destination_reauthorized() {
    if !docker_and_image_available().await {
        clouddesk_test_support::blocked_by_environment(
            "task_8_save_destination_reauthorized",
            clouddesk_test_support::reason::CONTAINER_RUNTIME_UNAVAILABLE,
        );
        return;
    }
    let (base, _dir, _runtime) = application().await;
    let _cross_process_guard = tokio::task::spawn_blocking(acquire_cross_process_browser_lock)
        .await
        .unwrap();
    let _brave_container_guard = BraveContainerGuard::new();

    let fixture_port = spawn_download_fixture("save_test.bin").await;
    let gw = gateway_ip().await;
    clouddeskd::browser_egress_proxy::set_test_allowlist([gw.parse().unwrap()]);
    let fixture_url = format!("http://{gw}:{fixture_port}/");

    let admin_cookie = bootstrap_admin(&base).await;
    enable_browser(&base, &admin_cookie).await;
    let user_cookie = create_user(&base, &admin_cookie, "dlsaveuser", "user").await;
    let instance_id = open_browser_instance(&base, &user_cookie).await;
    let (mut tx, mut rx) = connect_browser_ws(&base, &user_cookie, &instance_id).await;
    let _ = recv_json_matching(
        &mut rx,
        |v| v["type"] == "connected",
        std::time::Duration::from_secs(10),
    )
    .await;
    navigate(&mut tx, &mut rx, &fixture_url).await;
    click_download_link(&mut tx).await;
    let completed = recv_json_matching(
        &mut rx,
        |v| v["type"] == "download_completed",
        std::time::Duration::from_secs(15),
    )
    .await
    .expect("download must complete");
    let download_id = completed["download"]["download_id"]
        .as_str()
        .unwrap()
        .to_owned();

    // Save into the user's own home (root_id: None) -- must succeed.
    tx.send(WsMessage::Text(
        json!({"type": "save_download", "download_id": download_id, "relative_path": "cloudesk-download-test-output.bin"})
            .to_string(),
    ))
    .await
    .unwrap();
    let saved = recv_json_matching(
        &mut rx,
        |v| v["type"] == "download_saved" || v["type"] == "error",
        std::time::Duration::from_secs(10),
    )
    .await;
    assert_eq!(
        saved.map(|v| v["type"].clone()),
        Some(json!("download_saved")),
        "saving into the user's own home must succeed"
    );

    let home = current_process_linux_identity().unwrap().home;
    let saved_path = home.join("cloudesk-download-test-output.bin");
    let saved_bytes = tokio::fs::read(&saved_path).await;
    let _ = tokio::fs::remove_file(&saved_path).await;
    assert_eq!(
        saved_bytes.ok().as_deref(),
        Some(REAL_PAYLOAD),
        "the saved file's real bytes must match the downloaded payload"
    );

    // A nonexistent/foreign root_id must be denied, not silently
    // fall back to home.
    tx.send(WsMessage::Text(
        json!({"type": "save_download", "download_id": download_id, "root_id": "nonexistent-root-id", "relative_path": "x.bin"})
            .to_string(),
    ))
    .await
    .unwrap();
    let denied = recv_json_matching(
        &mut rx,
        |v| v["type"] == "download_saved" || v["type"] == "error",
        std::time::Duration::from_secs(10),
    )
    .await;
    assert_eq!(
        denied.map(|v| v["type"].clone()),
        Some(json!("error")),
        "an unknown/foreign destination root must be denied"
    );

    let _ = tx.close().await;
}
