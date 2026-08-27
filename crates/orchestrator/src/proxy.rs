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
// `host` is deliberately forwarded, not stripped: Collabora (and, more
// generally, any upstream designed to sit behind a reverse proxy) uses
// the incoming `Host` header to construct the self-referential URLs it
// hands back to the browser (WebSocket endpoint, further asset
// requests). Stripped, the outbound `reqwest` client fills in its own
// default -- the upstream's *real* loopback address and port -- which
// Collabora then echoes straight back to the browser, leaking the raw
// container port and sending the client around CloudDesk's own proxy
// and authorization entirely (discovered only by a real browser
// actually issuing those follow-up requests; no protocol-level test
// ever constructed a Host header to notice this).
const STRIPPED_REQUEST_HEADERS: &[&str] = &[
    "connection",
    "content-length",
    "cookie", // the caller's CloudDesk session cookie must never reach the instance
    "authorization",
];
// `x-frame-options` and `content-security-policy` are stripped too: every
// proxied runtime UI (Code, Office) is deliberately rendered inside a
// same-origin CloudDesk iframe, but the upstream (code-server, Collabora)
// sets its own anti-clickjacking headers for being accessed directly and
// unframed -- `X-Frame-Options: DENY` / `frame-ancestors 'none'` in
// Collabora's case. Forwarded verbatim, those headers make the browser
// refuse to render the iframe at all (`net::ERR_BLOCKED_BY_RESPONSE`),
// which is invisible to any test that never drives a real browser. The
// caller-supplied ownership/authentication check already gates who can
// reach this proxy at all, so it is safe to replace the upstream's
// framing policy with one that permits exactly CloudDesk's own origin.
const STRIPPED_RESPONSE_HEADERS: &[&str] = &[
    "connection",
    "content-length",
    "transfer-encoding",
    "x-frame-options",
    "content-security-policy",
];

/// Replaces (or adds) only the `frame-ancestors` directive in
/// `upstream_csp`, leaving every other directive the upstream set --
/// `script-src`, `style-src`, `base-uri`, etc. -- untouched. `None`
/// (the upstream sent no CSP at all) falls back to the original
/// minimal, `frame-ancestors`-only policy this proxy has always set in
/// that case.
fn merge_frame_ancestors_into_csp(upstream_csp: Option<&str>) -> String {
    const FRAME_ANCESTORS: &str = "frame-ancestors 'self'";
    let Some(upstream_csp) = upstream_csp else {
        return FRAME_ANCESTORS.to_owned();
    };
    let mut directives: Vec<&str> = upstream_csp
        .split(';')
        .map(str::trim)
        .filter(|d| !d.is_empty() && !d.to_ascii_lowercase().starts_with("frame-ancestors"))
        .collect();
    directives.push(FRAME_ANCESTORS);
    directives.join("; ")
}

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
    // Real defect fixed during Phase 7D, found while proving Markdown
    // preview (Part 12): the upstream's own `content-security-policy`
    // was captured here ONLY to be thrown away below in favor of a
    // minimal `frame-ancestors 'self'`-only replacement -- correct for
    // the anti-clickjacking concern this was originally written for
    // (see `STRIPPED_RESPONSE_HEADERS`'s own comment), but it silently
    // discarded every OTHER directive the upstream's own policy
    // carried too. code-server serves its webview content (Markdown
    // preview, and any other webview-based feature) from a distinct
    // `vscode-resource.vscode-cdn.net` pseudo-origin, permitted only by
    // `script-src`/`style-src`/`base-uri` directives IN THAT SAME CSP
    // header -- dropping the whole header broke every one of them,
    // confirmed live via the browser's own CSP violation errors
    // ("Refused to load the script '.../markdown-language-features/
    // media/index.js' because it violates script-src 'self' ...").
    // Merge `frame-ancestors` into the upstream's real policy instead
    // of replacing it outright.
    let upstream_csp = response
        .headers()
        .get(axum::http::header::CONTENT_SECURITY_POLICY)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
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
    if let Some(headers) = out.headers_mut() {
        headers.insert(
            axum::http::header::X_FRAME_OPTIONS,
            axum::http::HeaderValue::from_static("SAMEORIGIN"),
        );
        let merged_csp = merge_frame_ancestors_into_csp(upstream_csp.as_deref());
        if let Ok(value) = axum::http::HeaderValue::from_str(&merged_csp) {
            headers.insert(
                axum::http::HeaderName::from_static("content-security-policy"),
                value,
            );
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

    // Ping/Pong control-frame fidelity (Phase 7D): each leg's underlying
    // library (axum's `WebSocket` and `tokio-tungstenite`, both built on
    // `tungstenite`) already answers incoming Pings with an automatic,
    // RFC 6455-compliant Pong on that same hop, transparently, before
    // the message is ever surfaced to this code -- confirmed directly
    // against the vendored source (`tungstenite`'s own doc comment:
    // "upon receiving ping messages tungstenite queues pong replies
    // automatically"; axum's: "Ping messages will be automatically
    // responded to by the server"). So per-hop liveness already worked
    // even before this fix. What did NOT work: an application-level
    // ping sent by one real endpoint (e.g. code-server verifying the
    // *actual browser*, not just this proxy, is still there) could
    // never reach the other endpoint at all, because both match arms
    // below silently discarded every Ping/Pong instead of relaying it
    // -- `continue`, dropping the frame, forwarding nothing. Relaying
    // them through (Ping stays Ping, Pong stays Pong, never converted
    // to Text/Binary) is what makes end-to-end liveness checks
    // possible. This does mean a manually relayed Ping can trigger a
    // second, independent auto-Pong from the receiving hop's own
    // library on top of the relayed reply -- an unavoidable
    // consequence of automatic per-hop replies neither library exposes
    // a way to disable, and RFC 6455 does not require exactly one Pong
    // per Ping (Pong is a liveness acknowledgement, not a 1:1 RPC
    // response), so an extra, protocol-legal Pong is harmless.
    let client_to_upstream = async {
        use futures_util::SinkExt;
        while let Some(Ok(msg)) = client_read.next().await {
            let forwarded = match msg {
                AxumMessage::Text(text) => Some(UpstreamMessage::Text(text.to_string())),
                AxumMessage::Binary(data) => Some(UpstreamMessage::Binary(data.to_vec())),
                AxumMessage::Ping(data) => Some(UpstreamMessage::Ping(data.to_vec())),
                AxumMessage::Pong(data) => Some(UpstreamMessage::Pong(data.to_vec())),
                AxumMessage::Close(_) => None,
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
                UpstreamMessage::Ping(data) => Some(AxumMessage::Ping(data.into())),
                UpstreamMessage::Pong(data) => Some(AxumMessage::Pong(data.into())),
                UpstreamMessage::Close(_) => None,
                // `Frame` is tungstenite's raw, not-otherwise-categorized
                // frame variant -- its own maintainers' guidance is to
                // ignore it (referenced directly in axum's vendored
                // source, `extract/ws.rs`), not something this proxy
                // can or should relay as a typed message.
                UpstreamMessage::Frame(_) => continue,
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

#[cfg(test)]
mod tests {
    use super::merge_frame_ancestors_into_csp;

    #[test]
    fn no_upstream_csp_falls_back_to_the_minimal_policy() {
        assert_eq!(
            merge_frame_ancestors_into_csp(None),
            "frame-ancestors 'self'"
        );
    }

    #[test]
    fn upstream_csp_with_no_frame_ancestors_keeps_its_other_directives() {
        let upstream =
            "script-src 'self' 'unsafe-inline'; style-src 'self' 'unsafe-inline'; base-uri 'self'";
        let merged = merge_frame_ancestors_into_csp(Some(upstream));
        assert!(merged.contains("script-src 'self' 'unsafe-inline'"));
        assert!(merged.contains("style-src 'self' 'unsafe-inline'"));
        assert!(merged.contains("base-uri 'self'"));
        assert!(merged.contains("frame-ancestors 'self'"));
    }

    #[test]
    fn upstream_frame_ancestors_is_replaced_not_duplicated() {
        // Real scenario this fixes: Collabora sets its own restrictive
        // `frame-ancestors 'none'` (an anti-clickjacking default for
        // being accessed unframed) alongside directives it needs for
        // its own webview resources -- the merge must override
        // `frame-ancestors` specifically while preserving the rest.
        let upstream = "default-src 'self'; frame-ancestors 'none'; img-src 'self' data:";
        let merged = merge_frame_ancestors_into_csp(Some(upstream));
        assert_eq!(merged.matches("frame-ancestors").count(), 1);
        assert!(merged.contains("frame-ancestors 'self'"));
        assert!(!merged.contains("frame-ancestors 'none'"));
        assert!(merged.contains("default-src 'self'"));
        assert!(merged.contains("img-src 'self' data:"));
    }

    #[test]
    fn empty_upstream_csp_still_produces_frame_ancestors() {
        assert_eq!(
            merge_frame_ancestors_into_csp(Some("")),
            "frame-ancestors 'self'"
        );
    }
}
