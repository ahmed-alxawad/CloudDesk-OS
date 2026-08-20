//! Phase 9 Pass 2/3A (see `PHASE9_BROWSER_EVIDENCE.md`): the trusted,
//! typed Browser broker.
//!
//! ## What this is
//!
//! The only path from an authenticated `CloudDesk` user to the real,
//! server-side Brave container's CDP (Chrome `DevTools` Protocol)
//! surface. Raw CDP is used internally, on the loopback-only relayed
//! port already proven private in the Phase 9 foundation pass (see
//! `browser_runtime.rs`) -- but nothing here ever hands a caller a
//! `DevTools` WebSocket URL, a debugging port, a container IP, a raw
//! CDP target ID, or a generic `send_cdp(method, params)` capability.
//! The wire protocol exposed to the frontend (`ClientMessage`/outbound
//! JSON below) is a small, fixed, typed set: navigate, resize, mouse,
//! keyboard, and tab management in; frame, page state, tab list,
//! connection state, error out. Every operation is bound to the
//! authenticated owner, the specific runtime instance, and that
//! instance's generation (Task 1/2) -- a session survives exactly as
//! long as the underlying container does, never longer.
//!
//! ## Tab model (Pass 3A)
//!
//! One browser-level CDP `WebSocket` connection per `CloudDesk` Browser
//! session, using real CDP `Target` multiplexing (`Target.createTarget`
//! plus `Target.attachToTarget` with `flatten: true`, sessionId-scoped
//! calls), not one raw connection per tab. `Target.setDiscoverTargets`
//! is enabled so real `window.open()`/`target=_blank` popups (which
//! Brave creates on its own) are observed via `Target.targetCreated`
//! and auto-attached as ordinary managed tabs, never left as unmanaged
//! targets. Only the active tab's screencast runs; switching tabs stops
//! the old one's screencast and starts the new one's. Tab count is
//! bounded (`MAX_TABS_PER_SESSION`) so a hostile popup-spawning page
//! cannot force unbounded renderer processes.
//!
//! ## What this is NOT (yet)
//!
//! Uploads, clipboard, and audio are not implemented here -- see
//! `PHASE9_BROWSER_EVIDENCE.md` for the honest accounting. Downloads
//! (Pass 3B) are implemented (see `browser_downloads.rs`).

use axum::extract::ws::{Message as AxumMessage, WebSocket};
use clouddesk_orchestrator::{InstanceId, RuntimeManager};
use futures_util::stream::{SplitSink, SplitStream};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
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

/// Task 4/26: a malicious page calling `window.open()` in a loop must
/// not be able to force unbounded renderer processes -- once this many
/// tabs exist for one session, any further new target (explicit
/// `create_tab` or a real popup) is immediately closed instead of
/// attached.
const MAX_TABS_PER_SESSION: usize = 8;

/// Process-wide, not per-session -- guarantees `TabId`s are never
/// coincidentally identical across two different `BrowserSession`s,
/// which would otherwise make cross-session tab-ownership denial
/// (Task 2) impossible to observe from the outside (a request against
/// another session's `tab_id` would just happen to match one of this
/// session's own tabs by coincidence, rather than being genuinely
/// absent).
static GLOBAL_TAB_SEQ: AtomicU64 = AtomicU64::new(1);

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
    CreateTab {
        #[serde(default)]
        url: Option<String>,
    },
    ActivateTab {
        tab_id: String,
    },
    CloseTab {
        tab_id: String,
    },
    /// Task 7/8 (Pass 3B): saves a completed download into an
    /// authorized `CloudDesk` Files destination. `root_id: None` means
    /// the user's own home directory (the same convention Code's
    /// workspace resolution already uses) -- never a raw path.
    SaveDownload {
        download_id: String,
        #[serde(default)]
        root_id: Option<String>,
        relative_path: String,
    },
    /// Task 9/10/11 (Pass 3B): responds to a real `Page.fileChooserOpened`
    /// event with a CloudDesk-authorized file. `root_id: None` means
    /// the user's own home directory, matching `SaveDownload`'s
    /// convention. `server_id: Some(..)` means `relative_path` is a
    /// virtual path on that already-owned remote `RemoteServer`
    /// (Task 11), reusing the same SFTP read path Files/Office already
    /// establish, never a client-supplied credential.
    SelectFile {
        chooser_id: String,
        #[serde(default)]
        root_id: Option<String>,
        #[serde(default)]
        server_id: Option<String>,
        relative_path: String,
    },
    /// Task 14/15 (Pass 3B): the CloudDesk-client-to-Brave paste
    /// direction. Delivered via CDP `Input.insertText` into whatever
    /// element is currently focused in the active tab -- exactly like
    /// a physical paste, no-op if nothing editable is focused. Never
    /// touches any host clipboard (Task 14's "session-scoped, not
    /// global desktop clipboard" requirement).
    ClipboardWrite {
        text: String,
    },
    /// Task 14/15: the Brave-to-CloudDesk-client copy direction.
    /// Returns whatever text is currently selected in the active
    /// tab's page (`window.getSelection()`), never a real OS
    /// clipboard read -- deliberately avoids the Clipboard Web API's
    /// secure-context/user-activation requirements entirely, which
    /// would otherwise silently fail on plain-`http` sites.
    ClipboardRead,
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

/// A minimal, backend-only JSON-RPC-over-WebSocket client for the
/// browser-level CDP connection. Never constructed from, or exposed
/// to, a `CloudDesk` API caller (Task 3).
struct CdpClient {
    sink: Mutex<SplitSink<CdpWsStream, CdpMessage>>,
    next_id: AtomicU64,
    pending: Arc<Mutex<HashMap<u64, oneshot::Sender<Value>>>>,
}

impl CdpClient {
    async fn call_raw(
        &self,
        session_id: Option<&str>,
        method: &str,
        params: Value,
    ) -> Result<Value, String> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(id, tx);
        let mut payload = json!({"id": id, "method": method, "params": params});
        if let Some(session_id) = session_id {
            payload["sessionId"] = json!(session_id);
        }
        if self
            .sink
            .lock()
            .await
            .send(CdpMessage::Text(payload.to_string()))
            .await
            .is_err()
        {
            self.pending.lock().await.remove(&id);
            return Err("cdp connection closed".to_owned());
        }
        rx.await.map_err(|_| "cdp connection closed".to_owned())
    }

    /// Browser-level call (no target attached).
    async fn call(&self, method: &str, params: Value) -> Result<Value, String> {
        self.call_raw(None, method, params).await
    }

    /// A call scoped to one attached target (tab) via its CDP session.
    async fn call_session(
        &self,
        session_id: &str,
        method: &str,
        params: Value,
    ) -> Result<Value, String> {
        self.call_raw(Some(session_id), method, params).await
    }
}

/// Reads real CDP frames off the wire, resolves pending method calls by
/// `id`, and forwards unsolicited events (browser-level and
/// session-scoped) to the session driver. Runs for the lifetime of the
/// one browser-level CDP connection.
async fn cdp_reader_loop(
    mut stream: SplitStream<CdpWsStream>,
    pending: Arc<Mutex<HashMap<u64, oneshot::Sender<Value>>>>,
    events_tx: mpsc::UnboundedSender<(Option<String>, String, Value)>,
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
            let session_id = value
                .get("sessionId")
                .and_then(Value::as_str)
                .map(str::to_owned);
            let params = value.get("params").cloned().unwrap_or(Value::Null);
            let _ = events_tx.send((session_id, method.to_owned(), params));
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
/// Page-state/tab-list/error/connection messages use a small, ordinary
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
                    // `frame_tx` only drops once the broker's main loop
                    // has already returned, which happens strictly after
                    // it sends its final "closed"/"error" message on
                    // `misc_tx` (Task 24, live-found this pass: a real
                    // `docker kill` sometimes raced this exact branch
                    // against an already-queued "closed" message, and
                    // `tokio::select!` picking this branch first --
                    // legal, since both were ready -- discarded it,
                    // silently hanging the client instead of reporting
                    // the crash). Drain whatever is already buffered
                    // before breaking so a message that was sent before
                    // this point is never lost to that race.
                    while let Ok(text) = misc_rx.try_recv() {
                        if client_tx.send(AxumMessage::Text(text.into())).await.is_err() {
                            break;
                        }
                    }
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

/// One managed tab: `CloudDesk`'s own opaque `TabId` maps to a real CDP
/// target + attached session. Never exposes `target_id`/`session_id` to
/// a caller (Task 1) -- both stay server-side only.
struct TabHandle {
    target_id: String,
    session_id: String,
    url: String,
    title: String,
    loading: bool,
}

/// Task 9 (Pass 3B): a real, pending `Page.fileChooserOpened` event,
/// bound to the exact CDP session/node that raised it. Opaque and
/// short-lived (`CHOOSER_EXPIRY`) -- a stale or foreign
/// `chooser_id` (from another session entirely, or one whose page has
/// long since navigated away) must never be usable to inject files
/// into an unrelated, possibly-since-repurposed DOM node.
struct PendingChooser {
    session_id: String,
    backend_node_id: u64,
    created_at: std::time::Instant,
}

const CHOOSER_EXPIRY: Duration = Duration::from_mins(2);
/// Task 12: bounds how large a single materialized upload may be --
/// an oversized selection must be rejected cleanly, not silently
/// stream unboundedly into this container's own `/state` mount.
const MAX_UPLOAD_MATERIALIZE_BYTES: u64 = 200 * 1024 * 1024;
/// Task 17: bounds a single clipboard paste -- no unbounded
/// allocation from a hostile-length client message.
const MAX_CLIPBOARD_BYTES: usize = 1_000_000;

/// Per-session broker state, shared between the client-message handler
/// and the CDP-event handler.
struct BrokerState {
    cdp: Arc<CdpClient>,
    tabs: Mutex<HashMap<String, TabHandle>>,
    /// Every target this session has ever seen (explicitly created or a
    /// real popup) -- lets `Target.targetCreated`'s handler tell the
    /// two cases apart: a target we already registered synchronously
    /// during our own `create_tab` (skip) vs. a genuine popup Brave
    /// created on its own (attach it).
    known_target_ids: Mutex<HashSet<String>>,
    active_tab: Mutex<Option<String>>,
    width: Mutex<u32>,
    height: Mutex<u32>,
    /// Task 1/2 (Pass 3B): keyed by CDP's own download GUID, which
    /// doubles as the public, opaque `DownloadId` -- never a server
    /// path. Per-connection scope, matching this broker's existing
    /// "per-connection session state" precedent (tabs, `active_tab`,
    /// etc.) rather than a separately persisted registry.
    downloads: Mutex<HashMap<String, crate::browser_downloads::DownloadRecord>>,
    pending_choosers: Mutex<HashMap<String, PendingChooser>>,
    /// Host-side path to this instance's own `/state` mount (Task 2) --
    /// downloads are staged under `{state_dir}/downloads/<guid>` inside
    /// the container, which is this same directory on the host.
    state_dir: std::path::PathBuf,
    auth: Option<clouddesk_auth::AuthService>,
    owner_user_id: String,
}

fn tab_summary(id: &str, tab: &TabHandle, active_id: Option<&str>) -> Value {
    json!({
        "tab_id": id,
        "url": tab.url,
        "title": tab.title,
        "loading": tab.loading,
        "active": active_id == Some(id),
    })
}

async fn send_tab_list(state: &BrokerState, misc_tx: &mpsc::UnboundedSender<String>) {
    let tabs = state.tabs.lock().await;
    let active = state.active_tab.lock().await.clone();
    let list: Vec<Value> = tabs
        .iter()
        .map(|(id, tab)| tab_summary(id, tab, active.as_deref()))
        .collect();
    let _ = misc_tx.send(json!({"type": "tab_list", "tabs": list}).to_string());
}

async fn apply_viewport_to_session(cdp: &CdpClient, session_id: &str, width: u32, height: u32) {
    let _ = cdp
        .call_session(
            session_id,
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

async fn start_screencast_for_session(cdp: &CdpClient, session_id: &str, width: u32, height: u32) {
    let _ = cdp
        .call_session(
            session_id,
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

async fn stop_screencast_for_session(cdp: &CdpClient, session_id: &str) {
    let _ = cdp
        .call_session(session_id, "Page.stopScreencast", json!({}))
        .await;
}

/// Attaches to an already-existing (or freshly created) real CDP
/// target, enables the domains the broker needs on it, and registers it
/// as a new managed tab. Shared by both `create_tab` (Task 1) and
/// popup auto-attach (Task 4).
async fn attach_and_register_tab(
    state: &Arc<BrokerState>,
    target_id: &str,
    make_active: bool,
) -> Option<String> {
    {
        let tabs = state.tabs.lock().await;
        if tabs.len() >= MAX_TABS_PER_SESSION {
            let _ = state
                .cdp
                .call("Target.closeTarget", json!({"targetId": target_id}))
                .await;
            return None;
        }
    }
    let attach = state
        .cdp
        .call(
            "Target.attachToTarget",
            json!({"targetId": target_id, "flatten": true}),
        )
        .await
        .ok()?;
    let session_id = attach.get("sessionId").and_then(Value::as_str)?.to_owned();

    let _ = state
        .cdp
        .call_session(&session_id, "Page.enable", json!({}))
        .await;
    let _ = state
        .cdp
        .call_session(&session_id, "Inspector.enable", json!({}))
        .await;
    // Task 9 (Pass 3B): every real file chooser on this tab is
    // intercepted, never left to Chromium's own native OS file
    // dialog -- which would expose this container's own filesystem
    // structure, not a CloudDesk-mediated selection.
    let _ = state
        .cdp
        .call_session(
            &session_id,
            "Page.setInterceptFileChooserDialog",
            json!({"enabled": true}),
        )
        .await;
    let width = *state.width.lock().await;
    let height = *state.height.lock().await;
    apply_viewport_to_session(&state.cdp, &session_id, width, height).await;

    let tab_seq = GLOBAL_TAB_SEQ.fetch_add(1, Ordering::SeqCst);
    let tab_id = format!("tab-{tab_seq}");
    state.tabs.lock().await.insert(
        tab_id.clone(),
        TabHandle {
            target_id: target_id.to_owned(),
            session_id: session_id.clone(),
            url: String::new(),
            title: String::new(),
            loading: true,
        },
    );

    if make_active {
        activate_tab_internal(state, &tab_id).await;
    }
    Some(tab_id)
}

/// Task 1/3: switches the active tab -- stops the previous active tab's
/// screencast (an inactive tab does not keep encoding/streaming frames
/// it can't be seen using), applies this session's current viewport to
/// the newly active tab, and starts its screencast.
async fn activate_tab_internal(state: &Arc<BrokerState>, tab_id: &str) {
    let previous = state.active_tab.lock().await.clone();
    if let Some(previous_id) = previous {
        if previous_id != tab_id {
            let previous_session_id = state
                .tabs
                .lock()
                .await
                .get(&previous_id)
                .map(|t| t.session_id.clone());
            if let Some(previous_session_id) = previous_session_id {
                stop_screencast_for_session(&state.cdp, &previous_session_id).await;
            }
        }
    }
    let (session_id, width, height) = {
        let tabs = state.tabs.lock().await;
        let Some(tab) = tabs.get(tab_id) else { return };
        (
            tab.session_id.clone(),
            *state.width.lock().await,
            *state.height.lock().await,
        )
    };
    apply_viewport_to_session(&state.cdp, &session_id, width, height).await;
    start_screencast_for_session(&state.cdp, &session_id, width, height).await;
    *state.active_tab.lock().await = Some(tab_id.to_owned());
}

async fn active_session_id(state: &BrokerState) -> Option<String> {
    let active = state.active_tab.lock().await.clone()?;
    state
        .tabs
        .lock()
        .await
        .get(&active)
        .map(|t| t.session_id.clone())
}

async fn dispatch_mouse(
    cdp: &CdpClient,
    session_id: &str,
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
    let _ = cdp
        .call_session(session_id, "Input.dispatchMouseEvent", params)
        .await;
}

async fn dispatch_key_down(cdp: &CdpClient, session_id: &str, key: &str, text: Option<&str>) {
    let _ = cdp
        .call_session(
            session_id,
            "Input.dispatchKeyEvent",
            json!({"type": "keyDown", "key": key}),
        )
        .await;
    if let Some(text) = text {
        if !text.is_empty() {
            let _ = cdp
                .call_session(
                    session_id,
                    "Input.dispatchKeyEvent",
                    json!({"type": "char", "key": key, "text": text, "unmodifiedText": text}),
                )
                .await;
        }
    }
}

async fn dispatch_key_up(cdp: &CdpClient, session_id: &str, key: &str) {
    let _ = cdp
        .call_session(
            session_id,
            "Input.dispatchKeyEvent",
            json!({"type": "keyUp", "key": key}),
        )
        .await;
}

#[allow(clippy::too_many_lines)]
async fn handle_client_message(
    state: &Arc<BrokerState>,
    msg: ClientMessage,
    misc_tx: &mpsc::UnboundedSender<String>,
) {
    match msg {
        ClientMessage::Navigate { url } => match validate_navigation_url(&url) {
            Ok(u) => {
                if let Some(session_id) = active_session_id(state).await {
                    let _ = state
                        .cdp
                        .call_session(&session_id, "Page.navigate", json!({"url": u}))
                        .await;
                }
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
            *state.width.lock().await = w;
            *state.height.lock().await = h;
            if let Some(session_id) = active_session_id(state).await {
                apply_viewport_to_session(&state.cdp, &session_id, w, h).await;
                stop_screencast_for_session(&state.cdp, &session_id).await;
                start_screencast_for_session(&state.cdp, &session_id, w, h).await;
            }
        }
        ClientMessage::MouseMove { x, y } => {
            if let Some(session_id) = active_session_id(state).await {
                dispatch_mouse(&state.cdp, &session_id, "mouseMoved", x, y, None).await;
            }
        }
        ClientMessage::MouseDown { x, y, button } => {
            if let Some(session_id) = active_session_id(state).await {
                dispatch_mouse(&state.cdp, &session_id, "mousePressed", x, y, Some(button)).await;
            }
        }
        ClientMessage::MouseUp { x, y, button } => {
            if let Some(session_id) = active_session_id(state).await {
                dispatch_mouse(&state.cdp, &session_id, "mouseReleased", x, y, Some(button)).await;
            }
        }
        ClientMessage::MouseWheel {
            x,
            y,
            delta_x,
            delta_y,
        } => {
            if let Some(session_id) = active_session_id(state).await {
                let _ = state
                    .cdp
                    .call_session(
                        &session_id,
                        "Input.dispatchMouseEvent",
                        json!({"type": "mouseWheel", "x": x, "y": y, "deltaX": delta_x, "deltaY": delta_y}),
                    )
                    .await;
            }
        }
        ClientMessage::KeyDown { key, text } => {
            if let Some(session_id) = active_session_id(state).await {
                dispatch_key_down(&state.cdp, &session_id, &key, text.as_deref()).await;
            }
        }
        ClientMessage::KeyUp { key } => {
            if let Some(session_id) = active_session_id(state).await {
                dispatch_key_up(&state.cdp, &session_id, &key).await;
            }
        }
        ClientMessage::CreateTab { url } => {
            let target_url = url
                .as_deref()
                .map_or(Ok("about:blank".to_owned()), validate_navigation_url);
            let Ok(target_url) = target_url else {
                let _ = misc_tx.send(
                    json!({"type": "error", "message": "navigation scheme not permitted"})
                        .to_string(),
                );
                return;
            };
            let Ok(created) = state
                .cdp
                .call("Target.createTarget", json!({"url": target_url}))
                .await
            else {
                let _ = misc_tx
                    .send(json!({"type": "error", "message": "failed to create tab"}).to_string());
                return;
            };
            let Some(target_id) = created
                .get("targetId")
                .and_then(Value::as_str)
                .map(str::to_owned)
            else {
                return;
            };
            state
                .known_target_ids
                .lock()
                .await
                .insert(target_id.clone());
            if let Some(tab_id) = attach_and_register_tab(state, &target_id, true).await {
                let _ = misc_tx.send(json!({"type": "tab_created", "tab_id": tab_id}).to_string());
            }
            send_tab_list(state, misc_tx).await;
        }
        ClientMessage::ActivateTab { tab_id } => {
            // Task 2: possession of a TabId is not authorization by
            // itself -- but every TabId here is already scoped to this
            // one authenticated session's own `tabs` map, so a TabId
            // from another session (another user, another
            // BrowserSession, another generation) simply never exists
            // in it and is silently ignored, exactly like an
            // authorization denial elsewhere in this codebase treats a
            // nonexistent resource.
            if state.tabs.lock().await.contains_key(&tab_id) {
                activate_tab_internal(state, &tab_id).await;
                send_tab_list(state, misc_tx).await;
            } else {
                let _ =
                    misc_tx.send(json!({"type": "error", "message": "unknown tab"}).to_string());
            }
        }
        ClientMessage::CloseTab { tab_id } => {
            let target_id = {
                let tabs = state.tabs.lock().await;
                tabs.get(&tab_id).map(|t| t.target_id.clone())
            };
            let Some(target_id) = target_id else {
                let _ =
                    misc_tx.send(json!({"type": "error", "message": "unknown tab"}).to_string());
                return;
            };
            let _ = state
                .cdp
                .call("Target.closeTarget", json!({"targetId": target_id}))
                .await;
            state.tabs.lock().await.remove(&tab_id);
            let was_active = state.active_tab.lock().await.as_deref() == Some(tab_id.as_str());
            if was_active {
                *state.active_tab.lock().await = None;
                let remaining_tab = state.tabs.lock().await.keys().next().cloned();
                if let Some(next_tab) = remaining_tab {
                    activate_tab_internal(state, &next_tab).await;
                } else {
                    // Task 3: never leave the session with zero tabs --
                    // a fresh about:blank tab replaces the last closed
                    // one, the same policy a normal desktop browser
                    // uses.
                    if let Ok(created) = state
                        .cdp
                        .call("Target.createTarget", json!({"url": "about:blank"}))
                        .await
                    {
                        if let Some(new_target_id) = created
                            .get("targetId")
                            .and_then(Value::as_str)
                            .map(str::to_owned)
                        {
                            state
                                .known_target_ids
                                .lock()
                                .await
                                .insert(new_target_id.clone());
                            attach_and_register_tab(state, &new_target_id, true).await;
                        }
                    }
                }
            }
            let _ = misc_tx.send(json!({"type": "tab_closed", "tab_id": tab_id}).to_string());
            send_tab_list(state, misc_tx).await;
        }
        ClientMessage::SaveDownload {
            download_id,
            root_id,
            relative_path,
        } => {
            save_download_to_files(
                state,
                &download_id,
                root_id.as_deref(),
                &relative_path,
                misc_tx,
            )
            .await;
        }
        ClientMessage::SelectFile {
            chooser_id,
            root_id,
            server_id,
            relative_path,
        } => {
            if server_id.is_some() {
                // Task 11 (remote-VFS upload materialization) was not
                // implemented this pass -- refused explicitly, never
                // silently mishandled as a local path.
                let _ = misc_tx.send(
                    json!({"type": "error", "message": "remote file selection not supported"})
                        .to_string(),
                );
                return;
            }
            select_file_for_chooser(
                state,
                &chooser_id,
                root_id.as_deref(),
                &relative_path,
                misc_tx,
            )
            .await;
        }
        ClientMessage::ClipboardWrite { text } => {
            if text.len() > MAX_CLIPBOARD_BYTES {
                let _ = misc_tx
                    .send(json!({"type": "error", "message": "clipboard text is too large"}).to_string());
                return;
            }
            let Some(session_id) = active_session_id(state).await else {
                let _ =
                    misc_tx.send(json!({"type": "error", "message": "no active tab"}).to_string());
                return;
            };
            let result = state
                .cdp
                .call_session(&session_id, "Input.insertText", json!({"text": text}))
                .await;
            if result.is_ok() {
                let _ = misc_tx.send(json!({"type": "clipboard_write_ok"}).to_string());
            } else {
                let _ = misc_tx
                    .send(json!({"type": "error", "message": "clipboard paste failed"}).to_string());
            }
        }
        ClientMessage::ClipboardRead => {
            let Some(session_id) = active_session_id(state).await else {
                let _ =
                    misc_tx.send(json!({"type": "error", "message": "no active tab"}).to_string());
                return;
            };
            let result = state
                .cdp
                .call_session(
                    &session_id,
                    "Runtime.evaluate",
                    json!({
                        "expression": "window.getSelection().toString()",
                        "returnByValue": true,
                    }),
                )
                .await;
            match result {
                Ok(value) if value["exceptionDetails"].is_null() => {
                    let raw = value["result"]["value"].as_str().unwrap_or_default();
                    let mut end = raw.len().min(MAX_CLIPBOARD_BYTES);
                    while end > 0 && !raw.is_char_boundary(end) {
                        end -= 1;
                    }
                    let text = raw[..end].to_owned();
                    let _ = misc_tx.send(json!({"type": "clipboard_read", "text": text}).to_string());
                }
                _ => {
                    let _ = misc_tx.send(
                        json!({"type": "error", "message": "clipboard copy failed"}).to_string(),
                    );
                }
            }
        }
    }
}

/// Task 9/10/12 (Pass 3B): re-authorizes `relative_path` against a
/// real `CloudDesk` Files root right now (never trusting anything
/// captured when the chooser first opened), materializes exactly that
/// file's bytes into this instance's own `/state/uploads` staging area
/// (Brave never receives a raw host path, and never sees any other
/// file on this system), then feeds it to the real, still-pending CDP
/// file chooser via `DOM.setFileInputFiles`. A stale, expired, or
/// foreign `chooser_id` is rejected identically to a nonexistent one.
/// Resolves the authorized local Files root a Task 10 selection must
/// stay within: an explicit assigned root when `root_id` is given, or
/// the owner's own home directory when it is not.
async fn resolve_upload_source_root(
    auth: &clouddesk_auth::AuthService,
    owner_user_id: &str,
    root_id: Option<&str>,
) -> Option<String> {
    if let Some(root_id) = root_id {
        return auth
            .resolve_assigned_root_for_user(owner_user_id, root_id)
            .await
            .ok()
            .map(|root| root.path);
    }
    let mapping = auth.linux_identity_for_user(owner_user_id).await.ok()?;
    let identity = clouddesk_linux::lookup_uid(mapping.uid).ok().flatten()?;
    Some(identity.home.to_string_lossy().into_owned())
}

async fn select_file_for_chooser(
    state: &Arc<BrokerState>,
    chooser_id: &str,
    root_id: Option<&str>,
    relative_path: &str,
    misc_tx: &mpsc::UnboundedSender<String>,
) {
    let Some(auth) = &state.auth else {
        let _ =
            misc_tx.send(json!({"type": "error", "message": "selection unavailable"}).to_string());
        return;
    };
    let chooser = {
        let mut choosers = state.pending_choosers.lock().await;
        let Some(chooser) = choosers.remove(chooser_id) else {
            let _ = misc_tx
                .send(json!({"type": "error", "message": "unknown file chooser"}).to_string());
            return;
        };
        if chooser.created_at.elapsed() > CHOOSER_EXPIRY {
            let _ = misc_tx
                .send(json!({"type": "error", "message": "file chooser expired"}).to_string());
            return;
        }
        chooser
    };

    let Some(source_root) = resolve_upload_source_root(auth, &state.owner_user_id, root_id).await
    else {
        let _ =
            misc_tx.send(json!({"type": "error", "message": "unknown source"}).to_string());
        return;
    };

    let Ok(canonical_root) = tokio::fs::canonicalize(&source_root).await else {
        let _ =
            misc_tx.send(json!({"type": "error", "message": "source unavailable"}).to_string());
        return;
    };
    let candidate = std::path::Path::new(&source_root).join(relative_path);
    let Ok(canonical_candidate) = tokio::fs::canonicalize(&candidate).await else {
        let _ = misc_tx.send(json!({"type": "error", "message": "file not found"}).to_string());
        return;
    };
    if !canonical_candidate.starts_with(&canonical_root) {
        let _ = misc_tx.send(
            json!({"type": "error", "message": "file is outside the authorized root"}).to_string(),
        );
        return;
    }
    let Ok(metadata) = tokio::fs::metadata(&canonical_candidate).await else {
        let _ = misc_tx.send(json!({"type": "error", "message": "file not found"}).to_string());
        return;
    };
    if !metadata.is_file() {
        let _ = misc_tx.send(json!({"type": "error", "message": "not a file"}).to_string());
        return;
    }
    if metadata.len() > MAX_UPLOAD_MATERIALIZE_BYTES {
        let _ =
            misc_tx.send(json!({"type": "error", "message": "file is too large"}).to_string());
        return;
    }

    // `DOM.setFileInputFiles` derives the website-visible `File.name`
    // from the basename of the path we hand it (real, live product
    // finding), so the materialized copy keeps the real selected
    // file's own basename -- kept collision-free by nesting it inside
    // a fresh opaque per-selection directory rather than renaming it.
    let materialize_dir_name = format!("upload-{}", GLOBAL_TAB_SEQ.fetch_add(1, Ordering::SeqCst));
    let upload_dir = state.state_dir.join("uploads").join(&materialize_dir_name);
    if tokio::fs::create_dir_all(&upload_dir).await.is_err() {
        let _ = misc_tx
            .send(json!({"type": "error", "message": "failed to prepare upload"}).to_string());
        return;
    }
    let Some(original_name) = canonical_candidate.file_name() else {
        let _ = misc_tx.send(json!({"type": "error", "message": "invalid file"}).to_string());
        return;
    };
    let materialize_path = upload_dir.join(original_name);
    if tokio::fs::copy(&canonical_candidate, &materialize_path)
        .await
        .is_err()
    {
        let _ = misc_tx
            .send(json!({"type": "error", "message": "failed to materialize file"}).to_string());
        return;
    }

    // The container's own view of this same file -- `/state` is the
    // adapter's fixed mount point (Task 9/10).
    let container_path = format!(
        "/state/uploads/{materialize_dir_name}/{}",
        original_name.to_string_lossy()
    );
    let set_result = state
        .cdp
        .call_session(
            &chooser.session_id,
            "DOM.setFileInputFiles",
            json!({"files": [container_path], "backendNodeId": chooser.backend_node_id}),
        )
        .await;
    // Task 13: the materialized copy only needs to exist long enough
    // for Chromium to read it into its own upload machinery, which
    // `DOM.setFileInputFiles` does synchronously before returning --
    // safe to remove immediately afterward regardless of outcome.
    let _ = tokio::fs::remove_file(&materialize_path).await;
    let _ = tokio::fs::remove_dir(&upload_dir).await;

    if set_result.is_ok() {
        let _ =
            misc_tx.send(json!({"type": "file_selected", "chooser_id": chooser_id}).to_string());
    } else {
        let _ = misc_tx.send(
            json!({"type": "error", "message": "failed to deliver selected file"}).to_string(),
        );
    }
}

/// Task 7/8 (Pass 3B): saves a completed, real, already-downloaded
/// file into an authorized `CloudDesk` Files destination. The
/// destination is re-resolved and re-authorized right now, from the
/// trusted `owner_user_id` this broker connection was opened with --
/// never a path captured earlier, never a client-supplied raw path.
/// Only a `Completed` download (real bytes already fully staged) can
/// be saved; a download the user never made (a random/foreign
/// `download_id`) or one still in progress is rejected identically,
/// so a caller can't distinguish "not yours" from "doesn't exist yet".
async fn save_download_to_files(
    state: &Arc<BrokerState>,
    download_id: &str,
    root_id: Option<&str>,
    relative_path: &str,
    misc_tx: &mpsc::UnboundedSender<String>,
) {
    let Some(auth) = &state.auth else {
        let _ = misc_tx.send(json!({"type": "error", "message": "save unavailable"}).to_string());
        return;
    };
    let (staging_path, sanitized_default) = {
        let downloads = state.downloads.lock().await;
        let Some(record) = downloads.get(download_id) else {
            let _ =
                misc_tx.send(json!({"type": "error", "message": "unknown download"}).to_string());
            return;
        };
        if record.state != crate::browser_downloads::DownloadStateKind::Completed {
            let _ = misc_tx
                .send(json!({"type": "error", "message": "download is not completed"}).to_string());
            return;
        }
        (
            record.staging_path.clone(),
            record.sanitized_filename.clone(),
        )
    };

    let relative_path = if relative_path.trim().is_empty() {
        sanitized_default
    } else {
        crate::browser_downloads::sanitize_download_filename(relative_path)
    };

    let destination_root = if let Some(root_id) = root_id {
        match auth
            .resolve_assigned_root_for_user(&state.owner_user_id, root_id)
            .await
        {
            Ok(root) if root.read_write => root.path,
            Ok(_) => {
                let _ = misc_tx.send(
                    json!({"type": "error", "message": "destination is read-only"}).to_string(),
                );
                return;
            }
            Err(_) => {
                let _ = misc_tx
                    .send(json!({"type": "error", "message": "unknown destination"}).to_string());
                return;
            }
        }
    } else if let Some(identity) = auth
        .linux_identity_for_user(&state.owner_user_id)
        .await
        .ok()
        .and_then(|mapping| clouddesk_linux::lookup_uid(mapping.uid).ok().flatten())
    {
        identity.home.to_string_lossy().into_owned()
    } else {
        let _ = misc_tx
            .send(json!({"type": "error", "message": "no destination available"}).to_string());
        return;
    };

    let candidate = std::path::Path::new(&destination_root).join(&relative_path);
    // The staging file must actually exist on disk before attempting
    // to canonicalize the destination's parent -- catches a download
    // whose bytes never actually landed (e.g. this instance's own
    // container was never healthy) with a clean error instead of an
    // `io::Error` leaking through.
    if tokio::fs::metadata(&staging_path).await.is_err() {
        let _ =
            misc_tx.send(json!({"type": "error", "message": "download file missing"}).to_string());
        return;
    }
    let Ok(canonical_root) = tokio::fs::canonicalize(&destination_root).await else {
        let _ = misc_tx
            .send(json!({"type": "error", "message": "destination unavailable"}).to_string());
        return;
    };
    // Re-canonicalize the *parent* of the candidate (the file itself
    // doesn't exist yet) and require it to still be inside the
    // authorized root -- rejects a `relative_path` that tries to
    // traverse back out via `..` even after sanitization.
    let parent = candidate.parent().unwrap_or(&candidate);
    let canonical_parent = tokio::fs::canonicalize(parent)
        .await
        .unwrap_or_else(|_| parent.to_path_buf());
    if !canonical_parent.starts_with(&canonical_root) {
        let _ = misc_tx.send(
            json!({"type": "error", "message": "destination outside authorized root"}).to_string(),
        );
        return;
    }

    match tokio::fs::copy(&staging_path, &candidate).await {
        Ok(_) => {
            let _ = misc_tx.send(
                json!({"type": "download_saved", "download_id": download_id, "path": relative_path})
                    .to_string(),
            );
        }
        Err(_) => {
            let _ = misc_tx
                .send(json!({"type": "error", "message": "failed to save download"}).to_string());
        }
    }
}

/// Task 8: only ever forwards safe fields (URL, loading state, title).
/// Never the internal container IP, debug port, or raw CDP target
/// metadata.
#[allow(clippy::too_many_lines)]
async fn handle_cdp_event(
    state: &Arc<BrokerState>,
    session_id: Option<&str>,
    method: &str,
    params: &Value,
    frame_tx: &watch::Sender<Option<String>>,
    misc_tx: &mpsc::UnboundedSender<String>,
) {
    match method {
        "Page.screencastFrame" => {
            let Some(session_id) = session_id else { return };
            let active_tab_id = state.active_tab.lock().await.clone();
            let active_matches = match &active_tab_id {
                Some(active) => state
                    .tabs
                    .lock()
                    .await
                    .get(active)
                    .is_some_and(|t| t.session_id == session_id),
                None => false,
            };
            let cdp_session_id = params
                .get("sessionId")
                .and_then(Value::as_i64)
                .unwrap_or_default();
            if active_matches {
                let data = params
                    .get("data")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let width = params
                    .pointer("/metadata/deviceWidth")
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
                let height = params
                    .pointer("/metadata/deviceHeight")
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
                let frame =
                    json!({"type": "frame", "data_base64": data, "width": width, "height": height})
                        .to_string();
                let _ = frame_tx.send(Some(frame));
            }
            // Chromium will not send another screencast frame for this
            // session until this ack arrives -- this is the CDP-native
            // half of Task 10's backpressure (bounded to one
            // outstanding frame per session), regardless of whether it
            // was the active tab (an inactive tab's screencast is
            // stopped, so this path is mostly defensive).
            let _ = state
                .cdp
                .call_session(
                    session_id,
                    "Page.screencastFrameAck",
                    json!({"sessionId": cdp_session_id}),
                )
                .await;
        }
        "Page.frameNavigated" => {
            let Some(session_id) = session_id else { return };
            if params.pointer("/frame/parentId").is_some() {
                return;
            }
            let Some(url) = params.pointer("/frame/url").and_then(Value::as_str) else {
                return;
            };
            let mut tabs = state.tabs.lock().await;
            if let Some((tab_id, tab)) = tabs.iter_mut().find(|(_, t)| t.session_id == session_id) {
                url.clone_into(&mut tab.url);
                tab.loading = true;
                let tab_id = tab_id.clone();
                drop(tabs);
                let _ = misc_tx.send(
                    json!({"type": "page_state", "tab_id": tab_id, "url": url, "loading": true})
                        .to_string(),
                );
                send_tab_list(state, misc_tx).await;
            }
        }
        "Page.loadEventFired" => {
            let Some(session_id) = session_id else { return };
            let mut tabs = state.tabs.lock().await;
            if let Some((tab_id, tab)) = tabs.iter_mut().find(|(_, t)| t.session_id == session_id) {
                tab.loading = false;
                let tab_id = tab_id.clone();
                drop(tabs);
                let _ = misc_tx.send(
                    json!({"type": "page_state", "tab_id": tab_id, "loading": false}).to_string(),
                );
                send_tab_list(state, misc_tx).await;
            }
        }
        "Inspector.targetCrashed" => {
            let _ = misc_tx.send(json!({"type": "page_state", "crashed": true}).to_string());
        }
        "Target.targetCreated" => {
            let target_type = params.pointer("/targetInfo/type").and_then(Value::as_str);
            let Some(target_id) = params
                .pointer("/targetInfo/targetId")
                .and_then(Value::as_str)
            else {
                return;
            };
            if target_type != Some("page") {
                return;
            }
            let already_known = !state
                .known_target_ids
                .lock()
                .await
                .insert(target_id.to_owned());
            if already_known {
                return;
            }
            // Task 4: a real popup/`window.open()` target Brave created
            // on its own -- translate it into a managed tab, never an
            // unmanaged renderer left outside this session's own
            // bookkeeping.
            attach_and_register_tab(state, target_id, true).await;
            send_tab_list(state, misc_tx).await;
        }
        "Target.targetDestroyed" => {
            let Some(target_id) = params.get("targetId").and_then(Value::as_str) else {
                return;
            };
            let closed_tab_id = {
                let mut tabs = state.tabs.lock().await;
                let id = tabs
                    .iter()
                    .find(|(_, t)| t.target_id == target_id)
                    .map(|(id, _)| id.clone());
                if let Some(id) = &id {
                    tabs.remove(id);
                }
                id
            };
            if let Some(tab_id) = closed_tab_id {
                let was_active = state.active_tab.lock().await.as_deref() == Some(tab_id.as_str());
                if was_active {
                    *state.active_tab.lock().await = None;
                    let remaining = state.tabs.lock().await.keys().next().cloned();
                    if let Some(next_tab) = remaining {
                        activate_tab_internal(state, &next_tab).await;
                    }
                }
                let _ = misc_tx.send(json!({"type": "tab_closed", "tab_id": tab_id}).to_string());
                send_tab_list(state, misc_tx).await;
            }
        }
        "Page.fileChooserOpened" => {
            let Some(session_id) = session_id else { return };
            let Some(backend_node_id) = params.get("backendNodeId").and_then(Value::as_u64)
            else {
                // No backend node id means this broker can't target
                // `DOM.setFileInputFiles` at anything -- nothing to
                // offer the client.
                return;
            };
            let chooser_id = format!(
                "chooser-{}",
                GLOBAL_TAB_SEQ.fetch_add(1, Ordering::SeqCst)
            );
            state.pending_choosers.lock().await.insert(
                chooser_id.clone(),
                PendingChooser {
                    session_id: session_id.to_owned(),
                    backend_node_id,
                    created_at: std::time::Instant::now(),
                },
            );
            let _ = misc_tx.send(
                json!({"type": "file_chooser_opened", "chooser_id": chooser_id}).to_string(),
            );
        }
        "Page.downloadWillBegin" => {
            let Some(guid) = params.get("guid").and_then(Value::as_str) else {
                return;
            };
            let url = params
                .get("url")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            let suggested = params
                .get("suggestedFilename")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            let sanitized = crate::browser_downloads::sanitize_download_filename(&suggested);
            let record = crate::browser_downloads::DownloadRecord {
                guid: guid.to_owned(),
                suggested_filename: suggested,
                sanitized_filename: sanitized,
                url,
                total_bytes: None,
                received_bytes: 0,
                state: crate::browser_downloads::DownloadStateKind::InProgress,
                failure_reason: None,
                staging_path: state.state_dir.join("downloads").join(guid),
            };
            let public = record.public_json();
            state.downloads.lock().await.insert(guid.to_owned(), record);
            let _ =
                misc_tx.send(json!({"type": "download_started", "download": public}).to_string());
        }
        "Browser.downloadProgress" => {
            let Some(guid) = params.get("guid").and_then(Value::as_str) else {
                return;
            };
            let cdp_state = params.get("state").and_then(Value::as_str).unwrap_or("");
            let received = params
                .get("receivedBytes")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            let total = params.get("totalBytes").and_then(Value::as_u64);

            // Task 4: enforce both the per-download and per-session
            // quotas *during* transfer, not only after completion --
            // cancel immediately once either bound is crossed so a
            // hostile/oversized download can never grow the staging
            // area unbounded.
            let over_per_download_quota = received > crate::browser_downloads::max_download_bytes();
            let over_session_quota = {
                let downloads = state.downloads.lock().await;
                let running_total: u64 = downloads
                    .values()
                    .map(|d| {
                        if d.guid == guid {
                            received
                        } else {
                            d.received_bytes
                        }
                    })
                    .sum();
                running_total > crate::browser_downloads::max_session_download_bytes()
            };
            if (over_per_download_quota || over_session_quota) && cdp_state == "inProgress" {
                let _ = state
                    .cdp
                    .call("Browser.cancelDownload", json!({"guid": guid}))
                    .await;
            }

            let mut downloads = state.downloads.lock().await;
            let Some(record) = downloads.get_mut(guid) else {
                return;
            };
            record.received_bytes = received;
            if total.is_some() {
                record.total_bytes = total;
            }
            record.state = match cdp_state {
                "completed" => crate::browser_downloads::DownloadStateKind::Completed,
                "canceled" => {
                    record.failure_reason =
                        Some(if over_per_download_quota || over_session_quota {
                            "quota exceeded".to_owned()
                        } else {
                            "cancelled".to_owned()
                        });
                    crate::browser_downloads::DownloadStateKind::Cancelled
                }
                _ => crate::browser_downloads::DownloadStateKind::InProgress,
            };
            let public = record.public_json();
            drop(downloads);
            let event_type = match cdp_state {
                "completed" => "download_completed",
                "canceled" => "download_failed",
                _ => "download_progress",
            };
            let _ = misc_tx.send(json!({"type": event_type, "download": public}).to_string());
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
/// `CloudDesk` Browser `WebSocket` connection to one real browser-level
/// CDP connection on the caller's own, already-ownership-checked
/// runtime instance (`id.owner_user_id` is derived from the
/// authenticated session by the caller -- see `instance_id_from_path`
/// in `lib.rs` -- never accepted from the request itself). The session
/// is implicitly bound to the instance's generation at connect time: if
/// the underlying container is replaced (restart/crash-recovery),
/// either the CDP socket itself dies (detected by `cdp_reader_loop`
/// exiting) or the periodic generation check below notices first --
/// either way the client receives an explicit `closed` message rather
/// than silently hanging.
#[allow(clippy::too_many_lines)]
pub async fn run_browser_session(
    runtime: Arc<RuntimeManager>,
    owner_user_id: String,
    id: InstanceId,
    socket: WebSocket,
    auth: Option<clouddesk_auth::AuthService>,
) {
    let (client_tx, mut client_rx) = socket.split();

    let Some(port) = runtime.instance_port(&owner_user_id, &id).await else {
        fail(client_tx, "browser runtime is not running").await;
        return;
    };
    let state_dir = runtime.instance_state_dir(&id).unwrap_or_default();
    let generation = runtime
        .store()
        .get(&id)
        .await
        .ok()
        .flatten()
        .map(|row| row.generation);

    let base = format!("http://127.0.0.1:{port}");
    let http = reqwest::Client::new();

    let Ok(version_response) = http.get(format!("{base}/json/version")).send().await else {
        fail(client_tx, "failed to reach the browser").await;
        return;
    };
    let Ok(version) = version_response.json::<Value>().await else {
        fail(client_tx, "invalid response from the browser").await;
        return;
    };
    let Some(browser_ws_url) = version
        .get("webSocketDebuggerUrl")
        .and_then(Value::as_str)
        .map(str::to_owned)
    else {
        fail(client_tx, "browser missing debugger endpoint").await;
        return;
    };

    let Ok((cdp_stream, _)) = tokio_tungstenite::connect_async(&browser_ws_url).await else {
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
    let (events_tx, mut events_rx) = mpsc::unbounded_channel::<(Option<String>, String, Value)>();
    tokio::spawn(cdp_reader_loop(cdp_read, pending, events_tx));

    // The container's own entrypoint launches Brave with an initial
    // `about:blank` tab already open (`docker/brave/Dockerfile`).
    // `Target.setDiscoverTargets` reports every *pre-existing* target
    // as a `Target.targetCreated` event too, not only future ones --
    // without this snapshot, that startup tab would be indistinguishable
    // from a genuine popup and get auto-attached/activated ahead of the
    // tab this session explicitly creates below.
    let mut pre_existing_target_ids = HashSet::new();
    if let Ok(existing) = cdp.call("Target.getTargets", json!({})).await {
        if let Some(infos) = existing.get("targetInfos").and_then(Value::as_array) {
            for info in infos {
                if let Some(target_id) = info.get("targetId").and_then(Value::as_str) {
                    pre_existing_target_ids.insert(target_id.to_owned());
                }
            }
        }
    }

    let _ = cdp
        .call("Target.setDiscoverTargets", json!({"discover": true}))
        .await;

    // Task 1-3 (Pass 3B): every download is renamed to its own opaque
    // GUID on disk by Chromium itself (`allowAndName`) -- a hostile
    // site's suggested filename never influences the real staging
    // path. `/state/downloads` is this same instance's own already-
    // isolated `/state` mount, never shared across instances/users.
    let _ = cdp
        .call(
            "Browser.setDownloadBehavior",
            json!({"behavior": "allowAndName", "downloadPath": "/state/downloads", "eventsEnabled": true}),
        )
        .await;

    let state = Arc::new(BrokerState {
        cdp: cdp.clone(),
        tabs: Mutex::new(HashMap::new()),
        known_target_ids: Mutex::new(pre_existing_target_ids),
        active_tab: Mutex::new(None),
        width: Mutex::new(DEFAULT_VIEWPORT_WIDTH),
        height: Mutex::new(DEFAULT_VIEWPORT_HEIGHT),
        downloads: Mutex::new(HashMap::new()),
        pending_choosers: Mutex::new(HashMap::new()),
        state_dir,
        auth,
        owner_user_id: owner_user_id.clone(),
    });

    // The first tab, created explicitly rather than relying on
    // whatever default target Brave happened to start with -- this
    // session's own tab bookkeeping must be authoritative from the
    // start.
    let Ok(first_target) = cdp
        .call("Target.createTarget", json!({"url": "about:blank"}))
        .await
    else {
        fail(client_tx, "failed to open the initial browser tab").await;
        return;
    };
    let Some(first_target_id) = first_target
        .get("targetId")
        .and_then(Value::as_str)
        .map(str::to_owned)
    else {
        fail(client_tx, "browser did not report a tab id").await;
        return;
    };
    state
        .known_target_ids
        .lock()
        .await
        .insert(first_target_id.clone());
    if attach_and_register_tab(&state, &first_target_id, true)
        .await
        .is_none()
    {
        fail(client_tx, "failed to attach to the initial browser tab").await;
        return;
    }

    let (frame_tx, frame_rx) = watch::channel::<Option<String>>(None);
    let (misc_tx, misc_rx) = mpsc::unbounded_channel::<String>();
    tokio::spawn(outbound_writer(client_tx, frame_rx, misc_rx));

    let _ = misc_tx.send(json!({"type": "connected"}).to_string());
    send_tab_list(&state, &misc_tx).await;

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
                if let Some((session_id, method, params)) = event {
                    handle_cdp_event(&state, session_id.as_deref(), &method, &params, &frame_tx, &misc_tx).await;
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
                                handle_client_message(&state, client_message, &misc_tx).await;
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

    // Task 26: close every tab this session ever attached, not just the
    // active one -- a session ending (client disconnect, generation
    // change, runtime stop) must never leave orphaned Brave targets.
    let target_ids: Vec<String> = state
        .tabs
        .lock()
        .await
        .values()
        .map(|t| t.target_id.clone())
        .collect();
    for target_id in target_ids {
        let _ = cdp
            .call("Target.closeTarget", json!({"targetId": target_id}))
            .await;
    }
}
