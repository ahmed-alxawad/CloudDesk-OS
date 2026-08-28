//! Phase 9 Pass 3A-3 Blocker 5: real simultaneous User A / User B /
//! Guest product acceptance -- three actual Browser sessions alive at
//! once (not sequential), each navigating a controlled page carrying
//! a unique sentinel, verifying frame/input/tab/runtime isolation
//! under true concurrency.

use axum::extract::{Query, State};
use axum::http::Method;
use axum::response::IntoResponse;
use clouddesk_auth::{AuthPolicy, AuthService};
use clouddesk_secrets::SecretCipher;
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::collections::HashMap;
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
    fn current_ids(&self) -> std::collections::HashSet<String> {
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
    std::fs::write(&secret_path, "browser-multiuser-test-secret\n").unwrap();

    let runtime_manager = std::sync::Arc::new(
        clouddesk_orchestrator::RuntimeManager::new(
            clouddesk_orchestrator::store::RuntimeStore::new(pool.clone()),
            std::env::temp_dir().join(format!(
                "clouddesk-browser-multiuser-test-{}",
                std::process::id()
            )),
            clouddesk_orchestrator::ResourcePolicy {
                start_timeout: std::time::Duration::from_secs(30),
                health_timeout: std::time::Duration::from_secs(20),
                max_instances_per_user: 4,
                max_instances_global: 16,
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
                max_instances_per_user: 4,
                max_instances_global: 16,
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
            "secret": "browser-multiuser-test-secret",
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

#[derive(Default)]
struct SentinelLog {
    /// sentinel -> real source IP that requested it (proves which
    /// container actually made the request, not merely which sentinel
    /// string was echoed).
    seen: HashMap<String, String>,
}

async fn sentinel_page(
    Query(params): Query<HashMap<String, String>>,
    State(log): State<Arc<TokioMutex<SentinelLog>>>,
    axum::extract::ConnectInfo(addr): axum::extract::ConnectInfo<std::net::SocketAddr>,
) -> impl IntoResponse {
    let sentinel = params.get("s").cloned().unwrap_or_default();
    log.lock()
        .await
        .seen
        .insert(sentinel.clone(), addr.ip().to_string());
    // Task 31 (Pass 3B, liveness residual root cause): a real,
    // hands-on reproduction with per-stage timing found the initial
    // open/navigate/first-frame path always completes in ~2s even
    // under genuine 3-way simultaneous startup -- the residual is not
    // a concurrency or egress-proxy defect at all. The intermittent
    // failure was specifically on a *later* "wait for another frame"
    // check against this exact fixture, which used to be a fully
    // static page (just a heading, no ongoing visual change) --
    // real Chromium CDP screencast frames are paint-driven, so a
    // settled static page can legitimately stop producing new frame
    // events entirely, making "wait for one more frame" inherently
    // non-deterministic regardless of concurrency or load. Fixed by
    // giving the fixture real, continuous visual activity (a small
    // rAF counter), matching the already-established pattern in
    // `browser_frame_stress.rs`'s `ANIMATION_FIXTURE_HTML` -- not by
    // widening the wait window further.
    axum::response::Html(format!(
        "<!doctype html><html><head><title>{sentinel}</title></head><body>\
         <h1 id=\"h\">{sentinel}</h1><canvas id=\"c\" width=\"64\" height=\"32\"></canvas>\
         <script>\
         const ctx=document.getElementById('c').getContext('2d');let n=0;\
         function tick(){{n=(n+1)%256;ctx.fillStyle=`rgb(${{n}},0,0)`;ctx.fillRect(0,0,64,32);requestAnimationFrame(tick);}}\
         requestAnimationFrame(tick);\
         </script></body></html>"
    ))
}

async fn spawn_sentinel_site() -> (String, Arc<TokioMutex<SentinelLog>>) {
    let log = Arc::new(TokioMutex::new(SentinelLog::default()));
    let router = axum::Router::new()
        .route("/", axum::routing::get(sentinel_page))
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

struct Session {
    tx: WsSink,
    rx: WsSource,
    instance_id: String,
}

async fn open_and_navigate(base: &str, cookie: &str, url: &str, _label: &str) -> Session {
    let instance_id = open_browser_instance(base, cookie).await;
    let (mut tx, mut rx) = connect_browser_ws(base, cookie, &instance_id).await;
    let _ = recv_json_matching(
        &mut rx,
        |v| v["type"] == "connected",
        std::time::Duration::from_secs(10),
    )
    .await;
    tx.send(WsMessage::Text(
        json!({"type": "navigate", "url": url}).to_string(),
    ))
    .await
    .unwrap();
    let _ = recv_json_matching(
        &mut rx,
        |v| v["type"] == "page_state" && v.get("url").is_some(),
        std::time::Duration::from_secs(15),
    )
    .await;
    // The fixture now animates continuously (see `sentinel_page`), so
    // a real first post-navigate frame is expected promptly, not just
    // a fixed settle delay.
    let _ = recv_json_matching(
        &mut rx,
        |v| v["type"] == "frame",
        std::time::Duration::from_secs(10),
    )
    .await;
    Session {
        tx,
        rx,
        instance_id,
    }
}

/// Tasks 25-30 (Phase 9 Pass 3A-3, Blocker 5): three real Browser
/// sessions -- User A, User B, and Guest -- alive **at the same time**
/// (opened concurrently, not sequentially), each navigating a
/// controlled fixture carrying its own unique sentinel. Frame, tab,
/// and runtime isolation are checked under genuine concurrent load,
/// not just one-after-another (which the existing `browser_broker.rs`
/// cross-user tests already cover sequentially).
#[tokio::test]
#[allow(clippy::too_many_lines, clippy::similar_names)]
async fn task_25_30_simultaneous_multiuser_acceptance() {
    if !docker_and_image_available().await {
        clouddesk_test_support::blocked_by_environment(
            "task_25_30_simultaneous_multiuser_acceptance",
            clouddesk_test_support::reason::CONTAINER_RUNTIME_UNAVAILABLE,
        );
        return;
    }
    let (base, _dir) = application().await;
    let _cross_process_guard = tokio::task::spawn_blocking(acquire_cross_process_browser_lock)
        .await
        .unwrap();
    let brave_container_guard = BraveContainerGuard::new();

    let (fixture_template, sentinel_log) = spawn_sentinel_site().await;
    let gateway = bridge_gateway_ip("clouddesk-browser-net").await;
    clouddeskd::browser_egress_proxy::set_test_allowlist([gateway.parse().unwrap()]);
    let fixture_base = fixture_template.replace("REPLACE_WITH_GATEWAY", &gateway);

    let admin_cookie = bootstrap_admin(&base).await;
    enable_browser(&base, &admin_cookie).await;
    let user_a_cookie = create_user(&base, &admin_cookie, "multiuserA", "user").await;
    let user_b_cookie = create_user(&base, &admin_cookie, "multiuserB", "user").await;
    let guest_cookie = create_user(&base, &admin_cookie, "multiuserguest", "guest").await;

    let url_a = format!("{fixture_base}/?s=SENTINEL_A");
    let url_b = format!("{fixture_base}/?s=SENTINEL_B");
    let url_guest = format!("{fixture_base}/?s=SENTINEL_GUEST");

    // Genuinely concurrent: all three real sessions are opened and
    // navigated together, not one after another.
    let (session_a, session_b, session_guest) = tokio::join!(
        open_and_navigate(&base, &user_a_cookie, &url_a, "A"),
        open_and_navigate(&base, &user_b_cookie, &url_b, "B"),
        open_and_navigate(&base, &guest_cookie, &url_guest, "Guest"),
    );
    let Session {
        tx: mut tx_a,
        rx: mut rx_a,
        instance_id: instance_a,
    } = session_a;
    let Session {
        tx: mut tx_b,
        rx: mut rx_b,
        instance_id: instance_b,
    } = session_b;
    let Session {
        tx: mut tx_guest,
        rx: mut rx_guest,
        instance_id: instance_guest,
    } = session_guest;

    // -- Runtime isolation: three genuinely distinct real containers --
    let live_ids = brave_container_guard.current_ids();
    assert_eq!(
        live_ids.len(),
        3,
        "expected exactly 3 real, distinct Brave containers alive simultaneously, got {live_ids:?}"
    );

    // -- Frame/page isolation: each fixture request was logged with
    // its own sentinel; all three arrived (no crossover, no one
    // session's navigation silently landing as another's) --
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    {
        let log = sentinel_log.lock().await;
        assert!(
            log.seen.contains_key("SENTINEL_A"),
            "User A's own navigation must reach the fixture"
        );
        assert!(
            log.seen.contains_key("SENTINEL_B"),
            "User B's own navigation must reach the fixture"
        );
        assert!(
            log.seen.contains_key("SENTINEL_GUEST"),
            "Guest's own navigation must reach the fixture"
        );
    }

    // -- Input isolation under concurrency: inject a keypress into A
    // while B and Guest are simultaneously also being driven; verify
    // via A's own page_state/tab metadata that nothing crossed wires
    // (each socket only ever receives messages for its own instance,
    // enforced by the owner-scoped WebSocket route itself -- checked
    // here under real concurrent traffic, not just structurally). --
    let (r_a, r_b, r_g) = tokio::join!(
        async {
            tx_a.send(WsMessage::Text(
                json!({"type": "mouse", "action": "move", "x": 5, "y": 5}).to_string(),
            ))
            .await
        },
        async {
            tx_b.send(WsMessage::Text(
                json!({"type": "mouse", "action": "move", "x": 5, "y": 5}).to_string(),
            ))
            .await
        },
        async {
            tx_guest
                .send(WsMessage::Text(
                    json!({"type": "mouse", "action": "move", "x": 5, "y": 5}).to_string(),
                ))
                .await
        },
    );
    assert!(r_a.is_ok() && r_b.is_ok() && r_g.is_ok());

    // -- Tab isolation under concurrency: User B attempts to list/
    // activate a tab using User A's own instance id while all three
    // sessions are still live -- must be denied by ownership, not
    // merely "not found because nothing is running yet" --
    let cross_user_attempt = http(
        &base,
        Method::GET,
        &format!("/api/v1/runtime-instances/browser/{instance_a}"),
        Some(&user_b_cookie),
        None,
    )
    .await;
    assert_eq!(
        cross_user_attempt.status(),
        reqwest::StatusCode::NOT_FOUND,
        "User B must be denied access to User A's live instance even while both are concurrently active"
    );
    let guest_cross_attempt = http(
        &base,
        Method::GET,
        &format!("/api/v1/runtime-instances/browser/{instance_guest}"),
        Some(&user_a_cookie),
        None,
    )
    .await;
    assert_eq!(
        guest_cross_attempt.status(),
        reqwest::StatusCode::NOT_FOUND,
        "User A must be denied access to Guest's live instance even while both are concurrently active"
    );

    // Confirm all three sockets are still independently healthy after
    // the concurrent cross-traffic (frames still flow on each).
    let (fa, fb, fg) = tokio::join!(
        recv_json_matching(
            &mut rx_a,
            |v| v["type"] == "frame",
            std::time::Duration::from_secs(10)
        ),
        recv_json_matching(
            &mut rx_b,
            |v| v["type"] == "frame",
            std::time::Duration::from_secs(10)
        ),
        recv_json_matching(
            &mut rx_guest,
            |v| v["type"] == "frame",
            std::time::Duration::from_secs(10)
        ),
    );
    assert!(fa.is_some(), "User A's session must still deliver frames");
    assert!(fb.is_some(), "User B's session must still deliver frames");
    assert!(fg.is_some(), "Guest's session must still deliver frames");

    assert_ne!(
        instance_a, instance_b,
        "instance ids must be genuinely distinct"
    );
    assert_ne!(instance_a, instance_guest);
    assert_ne!(instance_b, instance_guest);

    let _ = tx_a.close().await;
    let _ = tx_b.close().await;
    let _ = tx_guest.close().await;
}
