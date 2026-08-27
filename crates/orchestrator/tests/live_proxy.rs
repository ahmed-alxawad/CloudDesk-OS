//! Live tests for the authenticated HTTP/WebSocket proxy foundation
//! (Task 5/6/19/20/35) against the disposable `test-runtime-fixture`.
//! TEST FIXTURE ONLY -- proves the proxy plumbing, not Code/Office/
//! Browser.

use axum::http::{HeaderMap, Method};
use clouddesk_orchestrator::host_process::{HealthCheck, HostProcessAdapter, HostProcessSpec};
use clouddesk_orchestrator::manager::RuntimeManager;
use clouddesk_orchestrator::model::{Persistence, ResourcePolicy};
use clouddesk_orchestrator::proxy::{proxy_http, proxy_ws, ProxyError};
use clouddesk_orchestrator::store::RuntimeStore;
use clouddesk_orchestrator::{InstanceId, RuntimeKind};
use futures_util::StreamExt;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

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

async fn manager_with_running_instance() -> (Arc<RuntimeManager>, InstanceId, tempfile::TempDir) {
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .connect("sqlite::memory:")
        .await
        .unwrap();
    sqlx::migrate!("../../migrations").run(&pool).await.unwrap();
    for user in ["u1", "u2"] {
        sqlx::query(
            "INSERT INTO users (id, username, display_name, password_hash, created_at, updated_at)
             VALUES (?, ?, ?, 'x', 0, 0)",
        )
        .bind(user)
        .bind(user)
        .bind(user)
        .execute(&pool)
        .await
        .unwrap();
    }
    let store = RuntimeStore::new(pool);
    store
        .set_enabled(RuntimeKind::TestFixture, true)
        .await
        .unwrap();
    let root = tempfile::tempdir().unwrap();
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
        start_timeout: Duration::from_secs(5),
        health_timeout: Duration::from_secs(3),
        ..ResourcePolicy::default()
    };
    let manager = Arc::new(
        RuntimeManager::new(store, root.path().to_owned(), policy)
            .with_adapter(Arc::new(HostProcessAdapter::new(spec))),
    );
    let id = manager
        .create_instance("u1", RuntimeKind::TestFixture, Persistence::Ephemeral)
        .await
        .unwrap();
    manager.start_instance("u1", &id).await.unwrap();
    (manager, id, root)
}

#[tokio::test]
async fn task_5_authenticated_http_proxy_reaches_the_owned_instance() {
    let (manager, id, _root) = manager_with_running_instance().await;
    let response = proxy_http(
        &manager,
        "u1",
        &id,
        Method::GET,
        "/echo?msg=hello-from-the-proxy",
        &HeaderMap::new(),
        Vec::new(),
    )
    .await
    .unwrap();
    assert_eq!(response.status(), axum::http::StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .unwrap();
    assert_eq!(&body[..], b"hello-from-the-proxy");
}

#[tokio::test]
async fn task_21_35_cross_user_proxy_access_is_denied_not_ssrf_capable() {
    let (manager, id, _root) = manager_with_running_instance().await;

    // A different user cannot reach u1's instance through the proxy at
    // all -- possession of the InstanceId is not authorization.
    let result = proxy_http(
        &manager,
        "u2",
        &id,
        Method::GET,
        "/echo?msg=stolen",
        &HeaderMap::new(),
        Vec::new(),
    )
    .await;
    assert!(matches!(result, Err(ProxyError::NotFound)));

    // A stale/nonexistent instance ID is denied the same way, not
    // distinguished from a real one (no existence oracle).
    let fake_id = InstanceId {
        kind: RuntimeKind::TestFixture,
        owner_user_id: "u1".to_owned(),
        instance_id: "does-not-exist".to_owned(),
    };
    let result = proxy_http(
        &manager,
        "u1",
        &fake_id,
        Method::GET,
        "/echo?msg=x",
        &HeaderMap::new(),
        Vec::new(),
    )
    .await;
    assert!(matches!(result, Err(ProxyError::NotFound)));

    // There is no parameter anywhere in `proxy_http`'s signature that
    // accepts a caller-chosen host/port/URL -- the only address it can
    // ever reach is derived from `RuntimeManager::instance_port`, which
    // is itself ownership-scoped. This is what makes it structurally
    // non-SSRF-capable rather than merely "not tested to be."
    let owner_response = proxy_http(
        &manager,
        "u1",
        &id,
        Method::GET,
        "/echo?msg=owner-ok",
        &HeaderMap::new(),
        Vec::new(),
    )
    .await
    .unwrap();
    assert_eq!(owner_response.status(), axum::http::StatusCode::OK);
}

#[tokio::test]
async fn task_6_authenticated_websocket_proxy_echoes_through_to_the_instance() {
    use axum::extract::ws::WebSocketUpgrade;
    use axum::routing::get;
    use axum::Router;
    use futures_util::{SinkExt, StreamExt};

    let (manager, id, _root) = manager_with_running_instance().await;

    // A tiny local axum server stands in for "the client-facing
    // CloudDesk route" -- it does the WS upgrade and hands the socket
    // to `proxy_ws`, exactly as a real clouddeskd handler would after
    // its own session-cookie authentication (not exercised here; that
    // boundary is covered by the existing services/clouddeskd auth
    // tests -- this proves the *proxy* leg only).
    let manager_for_route = manager.clone();
    let id_for_route = id.clone();
    let app = Router::new().route(
        "/proxy-test",
        get(move |ws: WebSocketUpgrade| {
            let manager = manager_for_route.clone();
            let id = id_for_route.clone();
            async move {
                ws.on_upgrade(move |socket| async move {
                    proxy_ws(&manager, "u1", &id, socket).await;
                })
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let local_addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let (mut ws_stream, _) =
        tokio_tungstenite::connect_async(format!("ws://{local_addr}/proxy-test"))
            .await
            .unwrap();

    ws_stream
        .send(tokio_tungstenite::tungstenite::Message::Text(
            "ping-through-proxy".into(),
        ))
        .await
        .unwrap();
    let reply = tokio::time::timeout(Duration::from_secs(5), ws_stream.next())
        .await
        .expect("timed out waiting for the proxied echo")
        .unwrap()
        .unwrap();
    match reply {
        tokio_tungstenite::tungstenite::Message::Text(text) => {
            assert_eq!(text.as_str(), "ping-through-proxy");
        }
        other => panic!("unexpected message: {other:?}"),
    }
}

/// Phase 7D: `proxy_ws_path` (`crates/orchestrator/src/proxy.rs`)
/// previously discarded every WebSocket Ping/Pong control frame in both
/// directions (`continue`, dropping the frame entirely) instead of
/// relaying it. Confirmed against the vendored `axum`/`tungstenite`
/// source that BOTH the client-facing and upstream-facing legs already
/// auto-reply to an incoming Ping with a Pong at the library level,
/// transparently, regardless of what this proxy's own code does --
/// which is exactly why a naive "I sent a Ping, I got a Pong back"
/// assertion from the client's own perspective would pass whether or
/// not the fix exists (Part 5's warning). This test instead proves the
/// frame actually reaches the *other* real endpoint: `test-runtime-
/// fixture`'s `/ws` handler replies with a distinguishable
/// `FIXTURE_SAW_PING:<payload>` / `FIXTURE_SAW_PONG:<payload>` Text
/// message when it genuinely receives a Ping/Pong -- something no
/// library's automatic per-hop reply could ever produce -- and, for the
/// reverse direction, `SEND_PING:<payload>` / `SEND_PONG:<payload>`
/// trigger the fixture to originate a control frame for the proxy to
/// relay back to this test's own client socket.
type TestWs =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn next_message(ws: &mut TestWs) -> tokio_tungstenite::tungstenite::Message {
    tokio::time::timeout(Duration::from_secs(5), ws.next())
        .await
        .expect("timed out waiting for a message")
        .unwrap()
        .unwrap()
}

/// Poll until a Text message matching `predicate` arrives, silently
/// consuming any automatic per-hop Pong (or other control frame) that
/// isn't what we're looking for. This is the deliberate, documented
/// (Part 5) distinction between the library-managed frame (never
/// asserted on directly) and the application-visible outcome the test
/// actually cares about.
async fn next_text_matching(ws: &mut TestWs, predicate: impl Fn(&str) -> bool) -> String {
    for _ in 0..10 {
        if let tokio_tungstenite::tungstenite::Message::Text(text) = next_message(ws).await {
            if predicate(text.as_str()) {
                return text.clone();
            }
        }
    }
    panic!("expected Text message was not observed within 10 frames");
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn task_phase7d_websocket_ping_pong_control_frames_are_relayed() {
    use axum::extract::ws::WebSocketUpgrade;
    use axum::routing::get;
    use axum::Router;
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message;

    let (manager, id, _root) = manager_with_running_instance().await;

    let manager_for_route = manager.clone();
    let id_for_route = id.clone();
    let app = Router::new().route(
        "/proxy-test",
        get(move |ws: WebSocketUpgrade| {
            let manager = manager_for_route.clone();
            let id = id_for_route.clone();
            async move {
                ws.on_upgrade(move |socket| async move {
                    proxy_ws(&manager, "u1", &id, socket).await;
                })
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let local_addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{local_addr}/proxy-test"))
        .await
        .unwrap();

    // CLIENT -> UPSTREAM: Ping with a unique payload must reach the real
    // upstream fixture, not just trigger this hop's own automatic Pong.
    ws.send(Message::Ping(b"client-ping-payload-a1b2".to_vec()))
        .await
        .unwrap();
    let seen = next_text_matching(&mut ws, |t| t.starts_with("FIXTURE_SAW_PING:")).await;
    assert_eq!(seen, "FIXTURE_SAW_PING:client-ping-payload-a1b2");

    // CLIENT -> UPSTREAM: Pong (unsolicited, as a liveness beacon) must
    // also reach the upstream -- tungstenite/axum do not auto-generate
    // a reply to an incoming Pong (only to Ping), so there is no
    // automatic frame to filter out here.
    ws.send(Message::Pong(b"client-pong-payload-c3d4".to_vec()))
        .await
        .unwrap();
    let seen = next_text_matching(&mut ws, |t| t.starts_with("FIXTURE_SAW_PONG:")).await;
    assert_eq!(seen, "FIXTURE_SAW_PONG:client-pong-payload-c3d4");

    // UPSTREAM -> CLIENT: ask the fixture to originate a Ping/Pong of
    // its own; the proxy must relay it through to this test's socket
    // rather than swallowing it (the prior, now-fixed defect's other
    // direction).
    ws.send(Message::Text("SEND_PING:upstream-ping-payload-e5f6".into()))
        .await
        .unwrap();
    let mut saw_upstream_ping = false;
    for _ in 0..10 {
        if let Message::Ping(data) = next_message(&mut ws).await {
            assert_eq!(data, b"upstream-ping-payload-e5f6".to_vec());
            saw_upstream_ping = true;
            break;
        }
    }
    assert!(
        saw_upstream_ping,
        "the upstream-originated Ping was never relayed to the client"
    );

    ws.send(Message::Text("SEND_PONG:upstream-pong-payload-g7h8".into()))
        .await
        .unwrap();
    let mut relayed_upstream_pong = false;
    for _ in 0..10 {
        if let Message::Pong(data) = next_message(&mut ws).await {
            assert_eq!(data, b"upstream-pong-payload-g7h8".to_vec());
            relayed_upstream_pong = true;
            break;
        }
    }
    assert!(
        relayed_upstream_pong,
        "the upstream-originated Pong was never relayed to the client"
    );

    // Text and Binary must still be entirely unaffected by the control-
    // frame change, with unique payloads (never inferred from a
    // previous frame in this same test). Skip up to a bound rather than
    // asserting on the very next frame: relaying the fixture's Ping
    // above (upstream -> proxy) causes tokio-tungstenite's own
    // automatic per-hop Pong-on-Ping reply to reach the fixture's own
    // `socket.recv()` loop, which (per this test's own instrumentation,
    // reacting to *any* Pong it observes) emits one extra, harmless
    // `FIXTURE_SAW_PONG:...` Text frame -- a real, documented side
    // effect of manually relaying control frames (see `proxy.rs`'s own
    // comment), not a defect in the fix or in Text/Binary handling.
    ws.send(Message::Text("phase7d-text-check-i9j0".into()))
        .await
        .unwrap();
    let mut saw_text_check = false;
    for _ in 0..10 {
        if next_message(&mut ws).await == Message::Text("phase7d-text-check-i9j0".into()) {
            saw_text_check = true;
            break;
        }
    }
    assert!(saw_text_check, "text echo was not observed");

    ws.send(Message::Binary(vec![9, 7, 5, 3, 1, 0, 2, 4, 6, 8]))
        .await
        .unwrap();
    let mut saw_binary_check = false;
    for _ in 0..10 {
        if next_message(&mut ws).await == Message::Binary(vec![9, 7, 5, 3, 1, 0, 2, 4, 6, 8]) {
            saw_binary_check = true;
            break;
        }
    }
    assert!(saw_binary_check, "binary echo was not observed");

    // Close: the client-initiated close must propagate through the
    // proxy to the upstream and back -- the stream ends (`None`) rather
    // than hanging.
    ws.close(None).await.unwrap();
    let after_close = tokio::time::timeout(Duration::from_secs(5), ws.next())
        .await
        .expect("timed out waiting for the connection to close");
    assert!(
        after_close.is_none() || matches!(after_close, Some(Ok(Message::Close(_)))),
        "expected the connection to close cleanly, got: {after_close:?}"
    );
}
