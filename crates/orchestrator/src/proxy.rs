//! Authenticated reverse-proxy foundation (Task 19/20).
//!
//! The only thing a caller ever supplies is an `InstanceId` and a
//! request/path -- never a host, port, or URL. The upstream address is
//! always derived from `RuntimeManager::instance_port`, which is itself
//! ownership-scoped and only returns a port for a `Running` instance the
//! caller owns. There is no code path here that can be pointed at an
//! arbitrary host: this is not a general SSRF-capable proxy, it is a
//! narrow "reach this one instance you own, nothing else" relay.

use crate::manager::RuntimeManager;
use crate::model::InstanceId;
use axum::body::Body;
use axum::extract::ws::{Message as AxumMessage, WebSocket};
use axum::http::{HeaderMap, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::Message as UpstreamMessage;

/// Explicit WebSocket size bounds for the upstream (runtime-facing) leg
/// of the proxy (Phase 6 closure Task 2). `services/clouddeskd` applies
/// the matching bound to the client-facing leg.
const MAX_WS_MESSAGE_BYTES: usize = 4 * 1024 * 1024;
const MAX_WS_FRAME_BYTES: usize = 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum ProxyError {
    #[error("instance not found or not owned by the caller")]
    NotFound,
    #[error("instance is not currently running")]
    NotRunning,
    #[error("upstream request failed: {0}")]
    Upstream(String),
}

impl IntoResponse for ProxyError {
    fn into_response(self) -> Response {
        let status = match self {
            Self::NotFound => StatusCode::NOT_FOUND,
            Self::NotRunning => StatusCode::SERVICE_UNAVAILABLE,
            Self::Upstream(_) => StatusCode::BAD_GATEWAY,
        };
        (status, self.to_string()).into_response()
    }
}

/// Headers that must never be blindly forwarded in either direction
/// (hop-by-hop headers, or headers that would let the upstream response
/// impersonate a different origin/length than what we actually send).
const STRIPPED_REQUEST_HEADERS: &[&str] = &[
    "host",
    "connection",
    "content-length",
    "cookie", // the caller's CloudDesk session cookie must never reach the instance
    "authorization",
];
const STRIPPED_RESPONSE_HEADERS: &[&str] = &["connection", "content-length", "transfer-encoding"];

/// Resolves `id` (already ownership-checked by the caller against the
/// authenticated session -- this function does the *second*,
/// independent check via `instance_port`, which itself re-verifies
/// ownership) to its current loopback address, or a typed error.
async fn resolve_upstream(
    manager: &RuntimeManager,
    owner_user_id: &str,
    id: &InstanceId,
) -> Result<u16, ProxyError> {
    manager
        .instance_port(owner_user_id, id)
        .await
        .ok_or(ProxyError::NotFound)
}

/// Proxies one HTTP request/response to `id`'s instance. `upstream_path`
/// is the path+query to request from the instance -- constructed by the
/// caller from its own fixed route prefix stripping, not taken verbatim
/// from a client-supplied absolute URL.
pub async fn proxy_http(
    manager: &RuntimeManager,
    owner_user_id: &str,
    id: &InstanceId,
    method: Method,
    upstream_path: &str,
    request_headers: &HeaderMap,
    body: Vec<u8>,
) -> Result<Response, ProxyError> {
    let port = resolve_upstream(manager, owner_user_id, id).await?;
    let _ = manager.touch_activity(owner_user_id, id).await;
    let url = format!("http://127.0.0.1:{port}{upstream_path}");

    let client = reqwest::Client::new();
    let mut builder = client.request(
        reqwest::Method::from_bytes(method.as_str().as_bytes())
            .map_err(|e| ProxyError::Upstream(e.to_string()))?,
        &url,
    );
    for (name, value) in request_headers {
        if STRIPPED_REQUEST_HEADERS.contains(&name.as_str().to_ascii_lowercase().as_str()) {
            continue;
        }
        if let Ok(value) = value.to_str() {
            builder = builder.header(name.as_str(), value);
        }
    }
    let response = builder
        .body(body)
        .send()
        .await
        .map_err(|e| ProxyError::Upstream(e.to_string()))?;

    let status =
        StatusCode::from_u16(response.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    let mut out = Response::builder().status(status);
    for (name, value) in response.headers() {
        if STRIPPED_RESPONSE_HEADERS.contains(&name.as_str().to_ascii_lowercase().as_str()) {
            continue;
        }
        if let Some(headers) = out.headers_mut() {
            if let Ok(value) = axum::http::HeaderValue::from_bytes(value.as_bytes()) {
                headers.insert(
                    axum::http::HeaderName::from_bytes(name.as_str().as_bytes())
                        .unwrap_or(axum::http::header::CONTENT_TYPE),
                    value,
                );
            }
        }
    }
    let bytes = response
        .bytes()
        .await
        .map_err(|e| ProxyError::Upstream(e.to_string()))?;
    Ok(out.body(Body::from(bytes)).unwrap_or_else(|_| {
        (StatusCode::BAD_GATEWAY, "proxy response build failed").into_response()
    }))
}

/// Relays an already-upgraded client `WebSocket` to `id`'s instance's
/// own `/ws` endpoint. Ownership is re-verified (via `instance_port`)
/// before ever dialing the upstream -- a caller cannot reach any
/// instance, running process, or port other than the one their own
/// session is authorized for.
pub async fn proxy_ws(
    manager: &RuntimeManager,
    owner_user_id: &str,
    id: &InstanceId,
    client_socket: WebSocket,
) {
    proxy_ws_path(manager, owner_user_id, id, "/ws", client_socket).await;
}

/// Same as [`proxy_ws`], but relays to `upstream_path` (path+query)
/// instead of a fixed `/ws`. Real Collabora's WebSocket endpoint is
/// per-document and per-session (`/cool/{docKey}/ws?WOPISrc=...`,
/// constructed client-side from the editor bootstrap page), not a fixed
/// path the way code-server's is -- `office_ws_proxy` uses this so the
/// browser's own real WebSocket URL is honoured rather than assumed.
pub async fn proxy_ws_path(
    manager: &RuntimeManager,
    owner_user_id: &str,
    id: &InstanceId,
    upstream_path: &str,
    mut client_socket: WebSocket,
) {
    let Ok(port) = resolve_upstream(manager, owner_user_id, id).await else {
        let _ = client_socket.close().await;
        return;
    };
    let upstream_url = format!("ws://127.0.0.1:{port}{upstream_path}");
    // Explicit, deliberate bounds (Phase 6 closure Task 2) rather than
    // tungstenite's library defaults (64 MiB message / 16 MiB frame) --
    // matches the bound the client-facing leg enforces in clouddeskd,
    // so a misbehaving runtime can't force unbounded buffering on this
    // side of the proxy either.
    let config = tokio_tungstenite::tungstenite::protocol::WebSocketConfig {
        max_message_size: Some(MAX_WS_MESSAGE_BYTES),
        max_frame_size: Some(MAX_WS_FRAME_BYTES),
        ..Default::default()
    };
    let Ok((upstream, _)) =
        tokio_tungstenite::connect_async_with_config(&upstream_url, Some(config), false).await
    else {
        let _ = client_socket.close().await;
        return;
    };

    // A live proxy connection counts as activity for idle-shutdown
    // purposes (Task 12) -- recorded once per connection, not per
    // message, to avoid write amplification on a chatty session.
    let _ = manager.touch_activity(owner_user_id, id).await;

    let (mut upstream_write, mut upstream_read) = futures_util::StreamExt::split(upstream);
    let (mut client_write, mut client_read) = client_socket.split();

    let client_to_upstream = async {
        use futures_util::SinkExt;
        while let Some(Ok(msg)) = client_read.next().await {
            let forwarded = match msg {
                AxumMessage::Text(text) => Some(UpstreamMessage::Text(text.to_string())),
                AxumMessage::Binary(data) => Some(UpstreamMessage::Binary(data.to_vec())),
                AxumMessage::Close(_) => None,
                AxumMessage::Ping(_) | AxumMessage::Pong(_) => continue,
            };
            match forwarded {
                Some(m) => {
                    if upstream_write.send(m).await.is_err() {
                        break;
                    }
                }
                None => break,
            }
        }
        let _ = upstream_write.close().await;
    };

    let upstream_to_client = async {
        use futures_util::StreamExt;
        while let Some(Ok(msg)) = upstream_read.next().await {
            let forwarded = match msg {
                UpstreamMessage::Text(text) => Some(AxumMessage::Text(text.clone().into())),
                UpstreamMessage::Binary(data) => Some(AxumMessage::Binary(data.into())),
                UpstreamMessage::Close(_) => None,
                UpstreamMessage::Ping(_) | UpstreamMessage::Pong(_) | UpstreamMessage::Frame(_) => {
                    continue
                }
            };
            match forwarded {
                Some(m) => {
                    if client_write.send(m).await.is_err() {
                        break;
                    }
                }
                None => break,
            }
        }
        let _ = client_write.close().await;
    };

    tokio::join!(client_to_upstream, upstream_to_client);
}
