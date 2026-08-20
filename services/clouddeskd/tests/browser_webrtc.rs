//! Phase 9 Pass 3A-3 Blocker 3: real WebRTC network-leakage evidence
//! through the actual product path. A controlled fixture page creates
//! a real `RTCPeerConnection` (no STUN/TURN server -- host candidates
//! only) and reports every real ICE candidate it gathers back to a
//! server-side log via a real `fetch()` POST. A real Browser instance
//! navigates to it; the candidates' revealed addresses are checked
//! against the container's own dedicated-network subnet, never the
//! real host's physical LAN.

use axum::extract::State;
use axum::http::Method;
use axum::response::IntoResponse;
use clouddesk_auth::{AuthPolicy, AuthService};
use clouddesk_secrets::SecretCipher;
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::sync::Arc;
use tokio::process::Command as TokioCommand;
use tokio::sync::Mutex as TokioMutex;
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

async fn application() -> (String, tempfile::TempDir) {
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
    std::fs::write(&secret_path, "browser-webrtc-test-secret\n").unwrap();

    let runtime_root = tempfile::tempdir().unwrap();
    std::mem::forget(runtime_root);
    let runtime_manager = std::sync::Arc::new(
        clouddesk_orchestrator::RuntimeManager::new(
            clouddesk_orchestrator::store::RuntimeStore::new(pool.clone()),
            std::env::temp_dir().join(format!(
                "clouddesk-browser-webrtc-test-{}",
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
            "secret": "browser-webrtc-test-secret",
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

#[derive(Default)]
struct CandidateLog {
    candidates: Vec<String>,
    reported_done: bool,
}

const WEBRTC_FIXTURE_HTML: &str = r#"<!doctype html><html><body>
<script>
const pc = new RTCPeerConnection({ iceServers: [] });
pc.createDataChannel("probe");
const candidates = [];
pc.onicecandidate = (e) => {
  if (e.candidate) {
    candidates.push(e.candidate.candidate);
  } else {
    fetch("/report", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ candidates }),
    });
  }
};
pc.createOffer().then((offer) => pc.setLocalDescription(offer));
setTimeout(() => {
  fetch("/report", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ candidates, timedOut: true }),
  });
}, 4000);
</script>
</body></html>"#;

async fn fixture_root() -> impl IntoResponse {
    axum::response::Html(WEBRTC_FIXTURE_HTML)
}

async fn fixture_report(
    State(log): State<Arc<TokioMutex<CandidateLog>>>,
    body: axum::Json<Value>,
) -> impl IntoResponse {
    let mut log = log.lock().await;
    if let Some(candidates) = body.0.get("candidates").and_then(Value::as_array) {
        log.candidates = candidates
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect();
    }
    log.reported_done = true;
    axum::http::StatusCode::NO_CONTENT
}

async fn spawn_webrtc_fixture_site() -> (String, Arc<TokioMutex<CandidateLog>>) {
    let log = Arc::new(TokioMutex::new(CandidateLog::default()));
    let router = axum::Router::new()
        .route("/", axum::routing::get(fixture_root))
        .route("/report", axum::routing::post(fixture_report))
        .with_state(log.clone());
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
    (format!("http://REPLACE_WITH_GATEWAY:{port}"), log)
}

async fn bridge_gateway_ip(network: &str) -> String {
    let output = TokioCommand::new("docker")
        .args([
            "network",
            "inspect",
            network,
            "--format",
            "{{(index .IPAM.Config 0).Gateway}}",
        ])
        .output()
        .await
        .unwrap();
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

/// Tasks 15-17 (Phase 9 Pass 3A-3, Blocker 3): a real WebRTC ICE
/// candidate-gathering fixture, no STUN/TURN server (host candidates
/// only -- the exact mechanism that can leak a container's real
/// network interfaces to a hostile page). A real Browser instance
/// navigates to it through the real product API; the candidates the
/// fixture's own server-side log actually received are checked
/// against the container's own dedicated-network subnet
/// (`clouddesk-browser-net`, `172.20.0.0/16` at the time of writing --
/// looked up live, not hardcoded), never the real host's physical LAN
/// address.
#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn task_15_16_17_webrtc_reveals_only_container_network() {
    if !docker_and_image_available().await {
        eprintln!("SKIP: docker/{BROWSER_IMAGE} not available (build docker/brave first)");
        return;
    }
    let (base, _dir) = application().await;
    let _cross_process_guard = tokio::task::spawn_blocking(acquire_cross_process_browser_lock)
        .await
        .unwrap();
    let _brave_container_guard = BraveContainerGuard::new();

    let (fixture_url_template, log) = spawn_webrtc_fixture_site().await;
    let admin_cookie = bootstrap_admin(&base).await;
    enable_browser(&base, &admin_cookie).await;
    let user_cookie = create_user(&base, &admin_cookie, "webrtcuser", "user").await;
    let instance_id = open_browser_instance(&base, &user_cookie).await;

    let gateway = bridge_gateway_ip("clouddesk-browser-net").await;
    assert!(
        !gateway.is_empty(),
        "expected the dedicated Browser network to already exist (created by the adapter on instance start)"
    );
    let fixture_url = fixture_url_template.replace("REPLACE_WITH_GATEWAY", &gateway);

    let (mut tx, mut rx) = connect_browser_ws(&base, &user_cookie, &instance_id).await;
    let _ = recv_json_matching(
        &mut rx,
        |v| v["type"] == "connected",
        std::time::Duration::from_secs(10),
    )
    .await;
    tx.send(WsMessage::Text(
        json!({"type": "navigate", "url": fixture_url}).to_string(),
    ))
    .await
    .unwrap();
    let _ = recv_json_matching(
        &mut rx,
        |v| v["type"] == "page_state" && v.get("url").is_some(),
        std::time::Duration::from_secs(15),
    )
    .await;

    // Real ICE gathering takes a moment; the fixture itself also
    // force-reports after 4s if `onicecandidate`'s null-candidate
    // "gathering complete" signal is slow, so this bound is generous.
    let mut reported = false;
    for _ in 0..20 {
        if log.lock().await.reported_done {
            reported = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    let _ = tx.close().await;
    assert!(
        reported,
        "the fixture must receive a real ICE-gathering report from the real Brave instance"
    );

    let candidates = log.lock().await.candidates.clone();
    // Every discovered candidate's address must fall within the
    // dedicated Browser network's own subnet (or be a loopback/mDNS
    // `.local` obfuscated host candidate, Chromium's own default
    // privacy mitigation) -- never the real host's physical LAN
    // address, which this project's own test-safety rules forbid
    // hardcoding/probing directly, so this is checked structurally by
    // asserting no candidate's address falls outside the known-safe
    // container/loopback ranges instead.
    let container_subnet_prefix = gateway
        .rsplit_once('.')
        .map(|(prefix, _)| format!("{prefix}."))
        .unwrap_or_default();
    for candidate in &candidates {
        let is_safe = candidate.contains(".local")
            || candidate.contains("127.0.0.1")
            || candidate.contains(&container_subnet_prefix);
        assert!(
            is_safe,
            "WEBRTC LEAKAGE: candidate must reveal only the container's own network or an mDNS-obfuscated host, got: {candidate} (safe container prefix: {container_subnet_prefix})"
        );
    }
    eprintln!(
        "WebRTC leakage review: {} real ICE candidate(s) observed, all within the container's own network: {:?}",
        candidates.len(),
        candidates
    );
}
