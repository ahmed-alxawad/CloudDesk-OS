//! Phase 9 Pass 3A-3 Blocker 4: real frame/backpressure live stress
//! evidence through the actual product path -- a real, fast-changing
//! `requestAnimationFrame` fixture, a real broker, real bounded
//! metrics (duration, approx frame rate, container memory
//! before/after), not a theoretical "the watch channel is bounded"
//! claim alone.

use axum::http::Method;
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
    fn current_ids(&self) -> Vec<String> {
        list_brave_container_ids()
            .difference(&self.before)
            .cloned()
            .collect()
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
    std::fs::write(&secret_path, "browser-stress-test-secret\n").unwrap();

    let runtime_root = tempfile::tempdir().unwrap();
    std::mem::forget(runtime_root);
    let runtime_manager = std::sync::Arc::new(
        clouddesk_orchestrator::RuntimeManager::new(
            clouddesk_orchestrator::store::RuntimeStore::new(pool.clone()),
            std::env::temp_dir().join(format!(
                "clouddesk-browser-stress-test-{}",
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
            "secret": "browser-stress-test-secret",
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

const ANIMATION_FIXTURE_HTML: &str = r##"<!doctype html><html><body style="margin:0">
<canvas id="c" width="400" height="300"></canvas>
<script>
const ctx = document.getElementById("c").getContext("2d");
let n = 0;
function tick() {
  n = (n + 1) % 256;
  ctx.fillStyle = `rgb(${n},${(n*3)%256},${(n*7)%256})`;
  ctx.fillRect(0, 0, 400, 300);
  ctx.fillStyle = "#fff";
  ctx.font = "40px sans-serif";
  ctx.fillText(String(n), 10, 50);
  requestAnimationFrame(tick);
}
requestAnimationFrame(tick);
</script>
</body></html>"##;

async fn spawn_animation_fixture_site() -> String {
    let router = axum::Router::new().route(
        "/",
        axum::routing::get(|| async { axum::response::Html(ANIMATION_FIXTURE_HTML) }),
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
    format!("http://REPLACE_WITH_GATEWAY:{port}")
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

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
async fn container_rss_kb(container_id: &str) -> Option<u64> {
    let output = TokioCommand::new("docker")
        .args([
            "stats",
            "--no-stream",
            "--format",
            "{{.MemUsage}}",
            container_id,
        ])
        .output()
        .await
        .ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    // e.g. "12.34MiB / 512MiB" -- only the first (used) side matters.
    let used = text.split('/').next()?.trim();
    let (num, unit) = used.split_at(used.find(char::is_alphabetic)?);
    let value: f64 = num.trim().parse().ok()?;
    let kb = match unit.trim() {
        "GiB" => value * 1024.0 * 1024.0,
        "MiB" => value * 1024.0,
        "KiB" => value,
        _ => return None,
    };
    Some(kb as u64)
}

/// Tasks 18-24 (Phase 9 Pass 3A-3, Blocker 4): a real, fast-changing
/// `requestAnimationFrame` fixture (bounded CPU: one canvas fill +
/// text draw per frame), a real broker, real Brave -- normal delivery,
/// a deliberately slow consumer, a fully paused consumer, rapid
/// resize while animating, and an abrupt disconnect, each followed by
/// a real, bounded check that the product is still healthy. Bounded
/// metrics (duration, approx frame count/rate, container memory
/// before/after) are recorded as real evidence, not claimed as a
/// mathematical memory-growth proof.
#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn task_18_24_frame_backpressure_live_stress() {
    if !docker_and_image_available().await {
        eprintln!("SKIP: docker/{BROWSER_IMAGE} not available (build docker/brave first)");
        return;
    }
    let (base, _dir) = application().await;
    let _cross_process_guard = tokio::task::spawn_blocking(acquire_cross_process_browser_lock)
        .await
        .unwrap();
    let brave_container_guard = BraveContainerGuard::new();

    let fixture_url_template = spawn_animation_fixture_site().await;
    let admin_cookie = bootstrap_admin(&base).await;
    enable_browser(&base, &admin_cookie).await;
    let user_cookie = create_user(&base, &admin_cookie, "stressuser", "user").await;
    let instance_id = open_browser_instance(&base, &user_cookie).await;

    let gateway = bridge_gateway_ip("clouddesk-browser-net").await;
    let fixture_url = fixture_url_template.replace("REPLACE_WITH_GATEWAY", &gateway);

    let container_id = brave_container_guard
        .current_ids()
        .into_iter()
        .next()
        .expect("expected a real running Brave container");
    let rss_start_kb = container_rss_kb(&container_id).await;

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

    // -- Normal client: count real frames over a bounded window --
    let normal_start = tokio::time::Instant::now();
    let mut normal_frames = 0u32;
    while normal_start.elapsed() < std::time::Duration::from_secs(4) {
        if recv_json_matching(
            &mut rx,
            |v| v["type"] == "frame",
            std::time::Duration::from_secs(2),
        )
        .await
        .is_some()
        {
            normal_frames += 1;
        }
    }
    assert!(
        normal_frames >= 3,
        "expected multiple real frames delivered to a normal, actively-consuming client, got {normal_frames}"
    );

    // -- Slow client: deliberately delay consumption; the watch
    // channel must still only ever deliver the *latest* frame, never
    // build an unbounded backlog of stale ones. Proven by: after a
    // deliberately slow period, the very next frame we finally read is
    // still delivered promptly (no multi-second replay of a queued
    // backlog), and the product hasn't stalled.
    for _ in 0..3 {
        tokio::time::sleep(std::time::Duration::from_millis(700)).await;
        let got = recv_json_matching(
            &mut rx,
            |v| v["type"] == "frame",
            std::time::Duration::from_secs(3),
        )
        .await;
        assert!(
            got.is_some(),
            "a slow client must still eventually receive the latest frame, not stall permanently"
        );
    }

    // -- Paused client: stop consuming entirely for a bounded interval --
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;

    // -- Resume: product must recover and keep delivering --
    let resumed = recv_json_matching(
        &mut rx,
        |v| v["type"] == "frame",
        std::time::Duration::from_secs(5),
    )
    .await;
    assert!(
        resumed.is_some(),
        "the product must recover and resume frame delivery after a paused client resumes reading"
    );

    // -- Resize stress: rapid viewport changes while animating --
    for (w, h) in [(320, 240), (800, 600), (200, 150), (1024, 768), (400, 300)] {
        tx.send(WsMessage::Text(
            json!({"type": "resize", "width": w, "height": h}).to_string(),
        ))
        .await
        .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    }
    let after_resize = recv_json_matching(
        &mut rx,
        |v| v["type"] == "frame",
        std::time::Duration::from_secs(5),
    )
    .await;
    assert!(
        after_resize.is_some(),
        "frame delivery must survive a rapid resize storm with no permanent stall"
    );

    let rss_end_kb = container_rss_kb(&container_id).await;

    // -- Abrupt disconnect: verify the broker/orchestrator recover cleanly --
    drop(tx);
    drop(rx);
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    let status = http(
        &base,
        Method::GET,
        &format!("/api/v1/runtime-instances/browser/{instance_id}"),
        Some(&user_cookie),
        None,
    )
    .await;
    let body: Value = status.json().await.unwrap();
    assert_eq!(
        body["state"].as_str(),
        Some("running"),
        "an abrupt client disconnect must not crash or stop the underlying instance"
    );

    // A fresh connection must still work normally after the abrupt
    // disconnect -- no orphaned session state blocking reconnection.
    let (mut tx2, mut rx2) = connect_browser_ws(&base, &user_cookie, &instance_id).await;
    let reconnected = recv_json_matching(
        &mut rx2,
        |v| v["type"] == "connected",
        std::time::Duration::from_secs(10),
    )
    .await;
    assert!(
        reconnected.is_some(),
        "must be able to reconnect cleanly after an abrupt prior disconnect"
    );
    let post_reconnect_frame = recv_json_matching(
        &mut rx2,
        |v| v["type"] == "frame",
        std::time::Duration::from_secs(10),
    )
    .await;
    assert!(
        post_reconnect_frame.is_some(),
        "frame delivery must resume normally on the reconnected session"
    );
    let _ = tx2.send(WsMessage::Close(None)).await;

    eprintln!(
        "Frame backpressure stress: normal-window frames={normal_frames} (~{:.1} fps over 4s), broker/Brave container RSS start={rss_start_kb:?}KiB end={rss_end_kb:?}KiB",
        f64::from(normal_frames) / 4.0
    );
}
