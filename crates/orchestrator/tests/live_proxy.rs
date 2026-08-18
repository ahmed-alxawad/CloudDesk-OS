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
