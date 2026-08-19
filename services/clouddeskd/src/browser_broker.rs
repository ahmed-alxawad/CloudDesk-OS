//! Phase 9 Pass 2 (see `PHASE9_BROWSER_EVIDENCE.md`): the trusted,
//! typed Browser broker.
//!
//! ## What this is
//!
//! The only path from an authenticated `CloudDesk` user to the real,
//! server-side Brave container's CDP (Chrome `DevTools` Protocol)
//! surface. Raw CDP is used internally, on the loopback-only relayed
//! port already proven private in the Phase 9 foundation pass (see
//! `browser_runtime.rs`) -- but nothing here ever hands a caller a
//! `DevTools` WebSocket URL, a debugging port, a container IP, or a
//! generic `send_cdp(method, params)` capability. The wire protocol
//! exposed to the frontend (`ClientMessage`/outbound JSON below) is a
//! small, fixed, typed set: navigate, resize, mouse, keyboard in;
//! frame, page state, connection state, error out. Every operation is
//! bound to the authenticated owner, the specific runtime instance,
//! and that instance's generation (Task 1/2) -- a session survives
//! exactly as long as the underlying container does, never longer.
//!
//! ## What this is NOT (yet)
//!
//! Tabs/popups (Task 28), downloads, uploads, clipboard, audio, and
//! the internal-network-isolation attack matrix are not implemented
//! here -- see `PHASE9_BROWSER_EVIDENCE.md` for the honest accounting.
//! This module is a genuine one-page vertical slice: one CDP target
//! per `CloudDesk` Browser WebSocket connection.

use axum::extract::ws::{Message as AxumMessage, WebSocket};
use clouddesk_orchestrator::{InstanceId, RuntimeManager};
use futures_util::stream::{SplitSink, SplitStream};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpStream;
use tokio::sync::{mpsc, oneshot, watch, Mutex};
use tokio_tungstenite::tungstenite::Message as CdpMessage;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

/// Task 13: clamped, never trusted verbatim from the client. A
/// hostile/broken client requesting a 50000x50000 viewport must not be
/// able to force Brave into an unbounded encode/memory cost.
const MIN_VIEWPORT_WIDTH: u32 = 200;
const MIN_VIEWPORT_HEIGHT: u32 = 150;
const MAX_VIEWPORT_WIDTH: u32 = 1920;
const MAX_VIEWPORT_HEIGHT: u32 = 1080;
const DEFAULT_VIEWPORT_WIDTH: u32 = 1024;
const DEFAULT_VIEWPORT_HEIGHT: u32 = 768;

/// Task 12: a hostile client sending an oversized message must not be
/// allowed to force unbounded buffering/parsing cost.
const MAX_CLIENT_MESSAGE_BYTES: usize = 64 * 1024;

const SCREENCAST_JPEG_QUALITY: u8 = 70;

type CdpWsStream = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// Task 11: the fixed, typed set of operations a `CloudDesk` Browser
/// client may request. Never a generic CDP passthrough (Task 3) --
/// anything not in this enum is rejected before it can reach Brave.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum ClientMessage {
    Navigate {
        url: String,
    },
    Resize {
        width: u32,
        height: u32,
    },
    MouseMove {
        x: f64,
        y: f64,
    },
    MouseDown {
        x: f64,
        y: f64,
        button: MouseButton,
    },
    MouseUp {
        x: f64,
        y: f64,
        button: MouseButton,
    },
    MouseWheel {
        x: f64,
        y: f64,
        delta_x: f64,
        delta_y: f64,
    },
    KeyDown {
        key: String,
        #[serde(default)]
        text: Option<String>,
    },
    KeyUp {
        key: String,
    },
}

#[derive(Debug, Deserialize, Clone, Copy)]
#[serde(rename_all = "snake_case")]
enum MouseButton {
    Left,
    Middle,
    Right,
}

impl MouseButton {
    fn as_cdp(self) -> &'static str {
        match self {
            MouseButton::Left => "left",
            MouseButton::Middle => "middle",
            MouseButton::Right => "right",
        }
    }
}

/// A minimal, backend-only JSON-RPC-over-WebSocket client for one real
/// CDP target. Never constructed from, or exposed to, a `CloudDesk`
/// API caller (Task 3).
struct CdpClient {
    sink: Mutex<SplitSink<CdpWsStream, CdpMessage>>,
    next_id: AtomicU64,
    pending: Arc<Mutex<HashMap<u64, oneshot::Sender<Value>>>>,
}

impl CdpClient {
    async fn call(&self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(id, tx);
        let payload = json!({"id": id, "method": method, "params": params}).to_string();
        if self
            .sink
            .lock()
            .await
            .send(CdpMessage::Text(payload))
            .await
            .is_err()
        {
            self.pending.lock().await.remove(&id);
            return Err("cdp connection closed".to_owned());
        }
        rx.await.map_err(|_| "cdp connection closed".to_owned())
    }
}

/// Reads real CDP frames off the wire, resolves pending method calls by
/// `id`, and forwards unsolicited events (screencast frames, page
/// lifecycle) to the session driver. Runs for the lifetime of one CDP
/// target connection.
async fn cdp_reader_loop(
    mut stream: SplitStream<CdpWsStream>,
    pending: Arc<Mutex<HashMap<u64, oneshot::Sender<Value>>>>,
    events_tx: mpsc::UnboundedSender<(String, Value)>,
) {
    while let Some(Ok(msg)) = stream.next().await {
        let CdpMessage::Text(text) = msg else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<Value>(&text) else {
            continue;
        };
        if let Some(id) = value.get("id").and_then(Value::as_u64) {
            if let Some(tx) = pending.lock().await.remove(&id) {
                let _ = tx.send(value.get("result").cloned().unwrap_or(Value::Null));
            }
        } else if let Some(method) = value.get("method").and_then(Value::as_str) {
            let params = value.get("params").cloned().unwrap_or(Value::Null);
            let _ = events_tx.send((method.to_owned(), params));
        }
    }
    // The CDP connection is gone (container crashed/restarted/stopped).
    // Drop every still-pending call so an in-flight `.await` resolves
    // with an error instead of hanging forever.
    pending.lock().await.clear();
}

/// Owns the client-facing `WebSocket` sink exclusively. Frame delivery
/// uses a `watch` channel (Task 10): a slow client only ever sees the
/// *latest* frame, never an unbounded backlog -- Chromium itself will
/// not emit another `Page.screencastFrame` until the previous one is
/// acknowledged (done immediately in `handle_cdp_event`, independent of
/// whether the client has actually consumed the prior frame yet), so
/// server-side memory for frames is bounded to one in flight plus one
/// pending delivery, never more, no matter how slow the client is.
/// Page-state/error/connection messages use a small, ordinary
/// unbounded channel -- they are infrequent, so unlike frames there is
/// no meaningful backpressure concern.
async fn outbound_writer(
    mut client_tx: SplitSink<WebSocket, AxumMessage>,
    mut frame_rx: watch::Receiver<Option<String>>,
    mut misc_rx: mpsc::UnboundedReceiver<String>,
) {
    loop {
        tokio::select! {
            changed = frame_rx.changed() => {
                if changed.is_err() {
                    break;
                }
                let Some(text) = frame_rx.borrow_and_update().clone() else {
                    continue;
                };
                if client_tx.send(AxumMessage::Text(text.into())).await.is_err() {
                    break;
                }
            }
            msg = misc_rx.recv() => {
                match msg {
                    Some(text) => {
                        if client_tx.send(AxumMessage::Text(text.into())).await.is_err() {
                            break;
                        }
                    }
                    None => break,
                }
            }
        }
    }
}

/// Task 7: a conservative allowlist, not a blacklist. Anything other
/// than `http`/`https` (and the fixed trusted start page) is rejected
/// -- `file:` (container-filesystem exfiltration risk), `javascript:`/
/// `devtools:` (script/DevTools-surface injection), and `data:`/
/// `blob:`/`chrome:`/`brave:` (internal-page/blob-exfiltration risk,
/// never independently investigated and cleared this pass) are all
/// refused, not silently rewritten.
fn validate_navigation_url(url: &str) -> Result<String, &'static str> {
    let trimmed = url.trim();
    if trimmed.is_empty() {
        return Err("empty URL");
    }
    let lower = trimmed.to_ascii_lowercase();
    if trimmed == "about:blank" || lower.starts_with("http://") || lower.starts_with("https://") {
        Ok(trimmed.to_owned())
    } else {
        Err("navigation scheme not permitted")
    }
}

async fn apply_viewport(cdp: &CdpClient, width: u32, height: u32) {
    let _ = cdp
        .call(
            "Emulation.setDeviceMetricsOverride",
            json!({
                "width": width,
                "height": height,
                "deviceScaleFactor": 1,
                "mobile": false,
            }),
        )
        .await;
}

async fn start_screencast(cdp: &CdpClient, width: u32, height: u32) {
    let _ = cdp
        .call(
            "Page.startScreencast",
            json!({
                "format": "jpeg",
                "quality": SCREENCAST_JPEG_QUALITY,
                "maxWidth": width,
                "maxHeight": height,
                "everyNthFrame": 1,
            }),
        )
        .await;
}

async fn dispatch_mouse(
    cdp: &CdpClient,
    event_type: &str,
    x: f64,
    y: f64,
    button: Option<MouseButton>,
) {
    let params = json!({
        "type": event_type,
        "x": x,
        "y": y,
        "button": button.map_or("none", MouseButton::as_cdp),
        "clickCount": 1,
    });
    let _ = cdp.call("Input.dispatchMouseEvent", params).await;
}

async fn dispatch_key_down(cdp: &CdpClient, key: &str, text: Option<&str>) {
    let _ = cdp
        .call(
            "Input.dispatchKeyEvent",
            json!({"type": "keyDown", "key": key}),
        )
        .await;
    if let Some(text) = text {
        if !text.is_empty() {
            let _ = cdp
                .call(
                    "Input.dispatchKeyEvent",
                    json!({"type": "char", "key": key, "text": text, "unmodifiedText": text}),
                )
                .await;
        }
    }
}

async fn dispatch_key_up(cdp: &CdpClient, key: &str) {
    let _ = cdp
        .call(
            "Input.dispatchKeyEvent",
            json!({"type": "keyUp", "key": key}),
        )
        .await;
}

async fn handle_client_message(
    cdp: &CdpClient,
    msg: ClientMessage,
    width: &mut u32,
    height: &mut u32,
    misc_tx: &mpsc::UnboundedSender<String>,
) {
    match msg {
        ClientMessage::Navigate { url } => match validate_navigation_url(&url) {
            Ok(u) => {
                let _ = cdp.call("Page.navigate", json!({"url": u})).await;
            }
            Err(reason) => {
                let _ = misc_tx.send(json!({"type": "error", "message": reason}).to_string());
            }
        },
        ClientMessage::Resize {
            width: req_w,
            height: req_h,
        } => {
            let w = req_w.clamp(MIN_VIEWPORT_WIDTH, MAX_VIEWPORT_WIDTH);
            let h = req_h.clamp(MIN_VIEWPORT_HEIGHT, MAX_VIEWPORT_HEIGHT);
            *width = w;
            *height = h;
            apply_viewport(cdp, w, h).await;
            let _ = cdp.call("Page.stopScreencast", json!({})).await;
            start_screencast(cdp, w, h).await;
        }
        ClientMessage::MouseMove { x, y } => dispatch_mouse(cdp, "mouseMoved", x, y, None).await,
        ClientMessage::MouseDown { x, y, button } => {
            dispatch_mouse(cdp, "mousePressed", x, y, Some(button)).await;
        }
        ClientMessage::MouseUp { x, y, button } => {
            dispatch_mouse(cdp, "mouseReleased", x, y, Some(button)).await;
        }
        ClientMessage::MouseWheel {
            x,
            y,
            delta_x,
            delta_y,
        } => {
            let _ = cdp
                .call(
                    "Input.dispatchMouseEvent",
                    json!({"type": "mouseWheel", "x": x, "y": y, "deltaX": delta_x, "deltaY": delta_y}),
                )
                .await;
        }
        ClientMessage::KeyDown { key, text } => {
            dispatch_key_down(cdp, &key, text.as_deref()).await;
        }
        ClientMessage::KeyUp { key } => dispatch_key_up(cdp, &key).await,
    }
}

/// Task 8: only ever forwards safe fields (URL, loading state). Never
/// the internal container IP, debug port, or raw CDP target metadata.
async fn handle_cdp_event(
    cdp: &CdpClient,
    method: &str,
    params: &Value,
    frame_tx: &watch::Sender<Option<String>>,
    misc_tx: &mpsc::UnboundedSender<String>,
) {
    match method {
        "Page.screencastFrame" => {
            let data = params
                .get("data")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let session_id = params
                .get("sessionId")
                .and_then(Value::as_i64)
                .unwrap_or_default();
            let width = params
                .pointer("/metadata/deviceWidth")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            let height = params
                .pointer("/metadata/deviceHeight")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            let frame = json!({
                "type": "frame",
                "data_base64": data,
                "width": width,
                "height": height,
            })
            .to_string();
            let _ = frame_tx.send(Some(frame));
            // Chromium will not send another screencast frame for this
            // session until this ack arrives -- this is the CDP-native
            // half of Task 10's backpressure (bounded to one
            // outstanding frame at the Brave side; the `watch` channel
            // above bounds the client-delivery side).
            let _ = cdp
                .call("Page.screencastFrameAck", json!({"sessionId": session_id}))
                .await;
        }
        "Page.frameNavigated" => {
            if params.pointer("/frame/parentId").is_none() {
                if let Some(url) = params.pointer("/frame/url").and_then(Value::as_str) {
                    let _ = misc_tx.send(
                        json!({"type": "page_state", "url": url, "loading": true}).to_string(),
                    );
                }
            }
        }
        "Page.loadEventFired" => {
            let _ = misc_tx.send(json!({"type": "page_state", "loading": false}).to_string());
        }
        "Inspector.targetCrashed" => {
            let _ = misc_tx.send(json!({"type": "page_state", "crashed": true}).to_string());
        }
        _ => {}
    }
}

async fn fail(mut client_tx: SplitSink<WebSocket, AxumMessage>, message: &str) {
    let _ = client_tx
        .send(AxumMessage::Text(
            json!({"type": "error", "message": message})
                .to_string()
                .into(),
        ))
        .await;
}

/// Task 1/2/4: the trusted broker's single public entry point. Ties one
/// `CloudDesk` Browser `WebSocket` connection to one real CDP target on
/// the caller's own, already-ownership-checked runtime instance
/// (`id.owner_user_id` is derived from the authenticated session by the
/// caller -- see `instance_id_from_path` in `lib.rs` -- never accepted
/// from the request itself). The session is implicitly bound to the
/// instance's generation at connect time: if the underlying container
/// is replaced (restart/crash-recovery), either the CDP socket itself
/// dies (detected by `cdp_reader_loop` exiting) or the periodic
/// generation check below notices first -- either way the client
/// receives an explicit `closed` message rather than silently hanging.
#[allow(clippy::too_many_lines)]
pub async fn run_browser_session(
    runtime: Arc<RuntimeManager>,
    owner_user_id: String,
    id: InstanceId,
    socket: WebSocket,
) {
    let (client_tx, mut client_rx) = socket.split();

    let Some(port) = runtime.instance_port(&owner_user_id, &id).await else {
        fail(client_tx, "browser runtime is not running").await;
        return;
    };
    let generation = runtime
        .store()
        .get(&id)
        .await
        .ok()
        .flatten()
        .map(|row| row.generation);

    let base = format!("http://127.0.0.1:{port}");
    let http = reqwest::Client::new();

    let Ok(target_response) = http
        .put(format!("{base}/json/new?about:blank"))
        .send()
        .await
    else {
        fail(client_tx, "failed to open a browser target").await;
        return;
    };
    if !target_response.status().is_success() {
        fail(client_tx, "failed to open a browser target").await;
        return;
    }
    let Ok(target) = target_response.json::<Value>().await else {
        fail(client_tx, "invalid response opening browser target").await;
        return;
    };
    let target_id = target
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let ws_url = target
        .get("webSocketDebuggerUrl")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    if ws_url.is_empty() {
        fail(client_tx, "browser target missing debugger endpoint").await;
        return;
    }

    let Ok((cdp_stream, _)) = tokio_tungstenite::connect_async(&ws_url).await else {
        let _ = http
            .get(format!("{base}/json/close/{target_id}"))
            .send()
            .await;
        fail(client_tx, "failed to connect to the browser").await;
        return;
    };
    let (cdp_sink, cdp_read) = cdp_stream.split();
    let pending: Arc<Mutex<HashMap<u64, oneshot::Sender<Value>>>> =
        Arc::new(Mutex::new(HashMap::new()));
    let cdp = Arc::new(CdpClient {
        sink: Mutex::new(cdp_sink),
        next_id: AtomicU64::new(1),
        pending: pending.clone(),
    });
    let (events_tx, mut events_rx) = mpsc::unbounded_channel::<(String, Value)>();
    tokio::spawn(cdp_reader_loop(cdp_read, pending, events_tx));

    let _ = cdp.call("Page.enable", json!({})).await;
    let _ = cdp.call("Inspector.enable", json!({})).await;

    let mut width = DEFAULT_VIEWPORT_WIDTH;
    let mut height = DEFAULT_VIEWPORT_HEIGHT;
    apply_viewport(&cdp, width, height).await;
    start_screencast(&cdp, width, height).await;

    let (frame_tx, frame_rx) = watch::channel::<Option<String>>(None);
    let (misc_tx, misc_rx) = mpsc::unbounded_channel::<String>();
    tokio::spawn(outbound_writer(client_tx, frame_rx, misc_rx));

    let _ = misc_tx.send(json!({"type": "connected"}).to_string());

    let mut generation_check = tokio::time::interval(Duration::from_secs(5));
    generation_check.tick().await; // first tick fires immediately; skip it

    loop {
        tokio::select! {
            _ = generation_check.tick() => {
                let still_running = runtime.instance_port(&owner_user_id, &id).await.is_some();
                let current_generation = runtime.store().get(&id).await.ok().flatten().map(|row| row.generation);
                if !still_running {
                    let _ = misc_tx.send(json!({"type": "closed", "reason": "runtime_stopped"}).to_string());
                    break;
                }
                if generation.is_some() && current_generation != generation {
                    let _ = misc_tx.send(json!({"type": "closed", "reason": "stale_generation"}).to_string());
                    break;
                }
            }
            event = events_rx.recv() => {
                if let Some((method, params)) = event {
                    handle_cdp_event(&cdp, &method, &params, &frame_tx, &misc_tx).await;
                } else {
                    let _ = misc_tx.send(json!({"type": "closed", "reason": "browser_disconnected"}).to_string());
                    break;
                }
            }
            incoming = client_rx.next() => {
                match incoming {
                    Some(Ok(AxumMessage::Text(text))) => {
                        if text.len() > MAX_CLIENT_MESSAGE_BYTES {
                            let _ = misc_tx.send(json!({"type": "error", "message": "message too large"}).to_string());
                            continue;
                        }
                        match serde_json::from_str::<ClientMessage>(&text) {
                            Ok(client_message) => {
                                handle_client_message(&cdp, client_message, &mut width, &mut height, &misc_tx).await;
                            }
                            Err(_) => {
                                let _ = misc_tx.send(json!({"type": "error", "message": "malformed message"}).to_string());
                            }
                        }
                    }
                    Some(Ok(AxumMessage::Close(_)) | Err(_)) | None => break,
                    Some(Ok(_)) => {}
                }
            }
        }
    }

    let _ = cdp.call("Page.stopScreencast", json!({})).await;
    let _ = http
        .get(format!("{base}/json/close/{target_id}"))
        .send()
        .await;
}
