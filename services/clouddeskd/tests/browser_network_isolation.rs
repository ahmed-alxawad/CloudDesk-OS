//! Phase 9 Pass 3A-3 Blocker 2: real internal-network isolation
//! evidence through the actual product path -- not a URL blacklist,
//! not a mocked network. A real "victim" HTTP fixture runs inside its
//! own container on the OLD shared `bridge` network (simulating
//! another user's runtime service, e.g. another user's Brave/Code/
//! Office container); a real Browser instance (now on the dedicated
//! `clouddesk-browser-net` network, see `crates/orchestrator/src/oci.rs`
//! and `browser_runtime.rs`) attempts to navigate straight to its
//! container IP. The victim's own independent request log is the
//! ground truth: judged by whether traffic actually arrived, never by
//! a client-side error message alone.

use axum::http::Method;
use clouddesk_auth::{AuthPolicy, AuthService};
use clouddesk_secrets::SecretCipher;
use futures_util::StreamExt;
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

/// Removes the real "victim" container this test starts manually,
/// regardless of how the test exits.
struct VictimContainerGuard(String);

impl Drop for VictimContainerGuard {
    fn drop(&mut self) {
        let _ = std::process::Command::new("docker")
            .args(["rm", "-f", &self.0])
            .output();
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

async fn application() -> (String, tempfile::TempDir) {
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
    std::fs::write(&secret_path, "browser-netiso-test-secret\n").unwrap();

    let runtime_manager = std::sync::Arc::new(
        clouddesk_orchestrator::RuntimeManager::new(
            clouddesk_orchestrator::store::RuntimeStore::new(pool.clone()),
            std::env::temp_dir().join(format!(
                "clouddesk-browser-netiso-test-{}",
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
            "secret": "browser-netiso-test-secret",
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

#[allow(clippy::type_complexity)]
async fn connect_browser_ws(
    base: &str,
    cookie: &str,
    instance_id: &str,
) -> (
    futures_util::stream::SplitSink<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
        WsMessage,
    >,
    futures_util::stream::SplitStream<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
    >,
) {
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

#[allow(clippy::type_complexity)]
async fn recv_json_matching(
    stream: &mut futures_util::stream::SplitStream<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
    >,
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

/// Task 6-9 (Phase 9 Pass 3A-3, Blocker 2): a real "victim" HTTP
/// service inside its own real Docker container on the OLD shared
/// `bridge` network -- standing in for another user's runtime
/// container (Code/Office/another Browser instance), all of which
/// still run on `bridge`, unaffected by this pass's Browser-specific
/// fix. A real Browser instance (now on the dedicated,
/// `enable_icc=false` `clouddesk-browser-net`) navigates straight at
/// the victim's real container IP:port. Judged by the victim's own
/// independent request log, not by a client-side error string.
#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn task_6_9_other_user_runtime_unreachable_from_browser() {
    if !docker_and_image_available().await {
        clouddesk_test_support::blocked_by_environment(
            "task_6_9_other_user_runtime_unreachable_from_browser",
            clouddesk_test_support::reason::CONTAINER_RUNTIME_UNAVAILABLE,
        );
        return;
    }
    let (base, _dir) = application().await;
    let _cross_process_guard = tokio::task::spawn_blocking(acquire_cross_process_browser_lock)
        .await
        .unwrap();
    let _brave_container_guard = BraveContainerGuard::new();

    // A real, minimal Python HTTP server as the "victim," on the plain
    // `bridge` network (exactly where Code/Office/other Browser
    // instances still live) -- no CloudDesk code involved on the
    // victim side, just a real listening TCP service.
    let victim_name = format!("clouddesk-netiso-victim-{}", std::process::id());
    let run = TokioCommand::new("docker")
        .args([
            "run",
            "-d",
            "--name",
            &victim_name,
            "--network",
            "bridge",
            "python:3-alpine",
            "python3",
            "-m",
            "http.server",
            "8000",
        ])
        .output()
        .await
        .unwrap();
    assert!(
        run.status.success(),
        "failed to start victim container: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    let _victim_guard = VictimContainerGuard(victim_name.clone());
    tokio::time::sleep(std::time::Duration::from_millis(800)).await;

    let inspect = TokioCommand::new("docker")
        .args([
            "network",
            "inspect",
            "bridge",
            "--format",
            &format!(
                "{{{{range .Containers}}}}{{{{if eq .Name \"{victim_name}\"}}}}{{{{.IPv4Address}}}}{{{{end}}}}{{{{end}}}}"
            ),
        ])
        .output()
        .await
        .unwrap();
    let victim_ip = String::from_utf8_lossy(&inspect.stdout)
        .trim()
        .split('/')
        .next()
        .unwrap_or_default()
        .to_owned();
    assert!(
        !victim_ip.is_empty(),
        "expected a real victim container IP on the bridge network"
    );
    let victim_url = format!("http://{victim_ip}:8000/");

    // Prove the victim is genuinely reachable at all, from the host,
    // before claiming Brave can't reach it (a real, not a vacuous,
    // negative result).
    //
    // Polled rather than attempted once after a fixed sleep
    // (Pre-Phase-10 reliability pass): `python3 -m http.server` inside
    // a freshly created `python:3-alpine` container regularly needs
    // several seconds before it is actually listening, well past the
    // 800ms this waited. The single attempt then hit a connection
    // refusal and failed this precondition -- deterministically on a
    // busy host -- even though the fixture and the isolation property
    // under test were both fine. The assertion's meaning is unchanged:
    // the victim must genuinely be reachable, or the negative result
    // below proves nothing.
    let host_client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(3))
        .build()
        .unwrap();
    let mut reachable_from_host = false;
    for _ in 0..30 {
        if host_client.get(&victim_url).send().await.is_ok() {
            reachable_from_host = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    assert!(
        reachable_from_host,
        "the victim fixture must be genuinely reachable from somewhere, or this test proves nothing"
    );

    // Baseline the victim's request log *after* the host-reachability
    // proof above (which itself logs a request) so the decisive check
    // below only counts requests that happen after this point.
    let baseline_logs = TokioCommand::new("docker")
        .args(["logs", &victim_name])
        .output()
        .await
        .unwrap();
    let baseline_len = baseline_logs.stdout.len() + baseline_logs.stderr.len();

    let admin_cookie = bootstrap_admin(&base).await;
    enable_browser(&base, &admin_cookie).await;
    let user_cookie = create_user(&base, &admin_cookie, "netisouser", "user").await;
    let instance_id = open_browser_instance(&base, &user_cookie).await;
    let (mut tx, mut rx) = connect_browser_ws(&base, &user_cookie, &instance_id).await;
    let _ = recv_json_matching(
        &mut rx,
        |v| v["type"] == "connected",
        std::time::Duration::from_secs(10),
    )
    .await;

    futures_util::SinkExt::send(
        &mut tx,
        WsMessage::Text(json!({"type": "navigate", "url": victim_url}).to_string()),
    )
    .await
    .unwrap();

    // Give Brave real time to attempt the connection and let any
    // TCP-level failure fully resolve (well beyond a typical connect
    // timeout), then ask the victim itself -- ground truth, not a
    // client-side error string -- whether any request ever arrived.
    tokio::time::sleep(std::time::Duration::from_secs(10)).await;
    let _ = futures_util::SinkExt::close(&mut tx).await;
    drop(rx);

    let logs = TokioCommand::new("docker")
        .args(["logs", &victim_name])
        .output()
        .await
        .unwrap();
    let full_log = format!(
        "{}{}",
        String::from_utf8_lossy(&logs.stdout),
        String::from_utf8_lossy(&logs.stderr)
    );
    let new_activity = &full_log[baseline_len.min(full_log.len())..];
    assert!(
        !new_activity.contains("GET / HTTP"),
        "OTHER_USER_RUNTIME ISOLATION: the victim's own request log must show zero NEW requests after Browser's navigation attempt, got new activity:\n{new_activity}\n(full log for context:\n{full_log})"
    );
}
