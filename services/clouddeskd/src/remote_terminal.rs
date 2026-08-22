//! PASS SSH-C: a real remote SSH PTY terminal, exposed through an
//! authenticated `CloudDesk` WebSocket. Reuses the exact same
//! `resolve_ssh_session` connection builder SFTP/SCP/advanced-auth
//! already use (host-key verification, credential resolution,
//! `ProxyJump`, ownership -- all inherited, never a second SSH stack)
//! plus the real PTY (`clouddesk_remote::pty`) proven live in
//! `crates/remote/tests/pty.rs`.
//!
//! Task 2/3: the terminal session is *opaque only in the sense that
//! its ID is unguessable* -- it carries no independent authority.
//! Every connection re-verifies `RemoteServer` ownership and the
//! caller's own live `CloudDesk` session at connect time, and again
//! periodically for as long as the WebSocket stays open (Task 18/19),
//! so revocation and logout take effect promptly rather than only at
//! the next reconnect. There is no "attach to an existing terminal by
//! ID" capability (matching the pre-existing local-terminal precedent
//! in this same codebase) -- the WebSocket connection itself *is* the
//! terminal session; a terminal ID exists only for audit correlation.

use crate::worker::resolve_ssh_session;
use crate::{audit_remote_action, authorize_request, principal, require_auth_service, AppState};
use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        ConnectInfo, Path, State,
    },
    http::HeaderMap,
    response::{IntoResponse, Response},
};
use clouddesk_remote::pty::TerminalEvent;
use clouddesk_remote::RemoteServerStore;
use clouddesk_vault::Vault;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::net::SocketAddr;
use std::time::Duration;

/// Task 6: typed client -> server messages. `Data`'s payload travels
/// as a raw WebSocket binary frame (never wrapped in this JSON
/// envelope) -- see `bridge` below -- so PTY bytes are never assumed
/// to be valid UTF-8 (Task 8).
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ClientMessage {
    Resize { cols: u16, rows: u16 },
    Close,
}

/// Task 6: typed server -> client control messages (JSON text frames);
/// PTY output itself travels as separate raw binary frames.
#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ServerMessage<'a> {
    Exit { code: Option<u32> },
    Error { message: &'a str },
    Revoked,
}

/// Task 33: reject absurd terminal dimensions before they ever reach
/// the SSH library -- a real terminal is never 0x0 or larger than any
/// sane physical/virtual display.
const MIN_DIMENSION: u16 = 1;
const MAX_DIMENSION: u16 = 1000;

fn valid_dimensions(cols: u16, rows: u16) -> bool {
    (MIN_DIMENSION..=MAX_DIMENSION).contains(&cols)
        && (MIN_DIMENSION..=MAX_DIMENSION).contains(&rows)
}

/// Task 41: a conservative, generous ceiling on live terminal
/// WebSocket connections at once (process-wide, not per-user) -- bounds
/// resource use without measurement-driven fine-tuning being available
/// yet; easy to raise later if legitimate use requires it.
const MAX_CONCURRENT_TERMINALS: usize = 64;
static ACTIVE_TERMINALS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

pub async fn open_remote_terminal_websocket(
    websocket: WebSocketUpgrade,
    State(state): State<AppState>,
    Path(server_id): Path<String>,
    ConnectInfo(connect): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Result<Response, crate::ApiError> {
    let auth = require_auth_service(&state)?;
    let principal = principal(&state, &headers).await?;
    authorize_request(
        auth,
        &principal,
        "remote.terminal.open",
        false,
        connect,
        &headers,
    )
    .await?;

    // Task 2: re-verified here (ownership) and periodically for the
    // life of the connection (Task 18/19), never trusted merely
    // because it passed once.
    let store = RemoteServerStore::new(auth.pool().clone());
    store.get(&principal.user_id, &server_id).await?;

    if ACTIVE_TERMINALS.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
        >= MAX_CONCURRENT_TERMINALS
    {
        ACTIVE_TERMINALS.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
        return Err(crate::ApiError::too_many_requests(
            "too many concurrent remote terminals",
        ));
    }

    audit_remote_action(
        auth,
        &principal,
        "terminal.requested",
        &server_id,
        "success",
        json!({}),
        connect,
        &headers,
    )
    .await?;

    let auth = auth.clone();
    let cookie = headers
        .get(axum::http::header::COOKIE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    Ok(websocket
        .on_upgrade(move |socket| async move {
            bridge(socket, auth, principal, server_id, cookie).await;
            ACTIVE_TERMINALS.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
        })
        .into_response())
}

#[allow(clippy::too_many_lines)]
async fn bridge(
    mut socket: WebSocket,
    auth: clouddesk_auth::AuthService,
    principal: clouddesk_auth::SessionPrincipal,
    server_id: String,
    cookie: Option<String>,
) {
    let store = RemoteServerStore::new(auth.pool().clone());
    let vault = Vault::new(auth.pool().clone(), auth.secret_cipher());

    let session = match resolve_ssh_session(&store, &vault, &principal.user_id, &server_id).await {
        Ok(session) => session,
        Err(error) => {
            let _ = socket
                .send(Message::Text(
                    serde_json::to_string(&ServerMessage::Error {
                        message: "could not connect to the remote server",
                    })
                    .unwrap_or_default()
                    .into(),
                ))
                .await;
            let _ = audit_remote_action(
                &auth,
                &principal,
                "terminal.failed",
                &server_id,
                "failure",
                json!({ "reason": error.to_string() }),
                "0.0.0.0:0".parse().unwrap(),
                &HeaderMap::new(),
            )
            .await;
            return;
        }
    };

    // Task 4: a real PTY, real interactive shell -- never a plain exec.
    let mut terminal = match session.open_terminal("xterm-256color", 80, 24).await {
        Ok(terminal) => terminal,
        Err(error) => {
            let _ = socket
                .send(Message::Text(
                    serde_json::to_string(&ServerMessage::Error {
                        message: "could not open a remote terminal",
                    })
                    .unwrap_or_default()
                    .into(),
                ))
                .await;
            let _ = audit_remote_action(
                &auth,
                &principal,
                "terminal.failed",
                &server_id,
                "failure",
                json!({ "reason": error.to_string() }),
                "0.0.0.0:0".parse().unwrap(),
                &HeaderMap::new(),
            )
            .await;
            return;
        }
    };

    let _ = audit_remote_action(
        &auth,
        &principal,
        "terminal.opened",
        &server_id,
        "success",
        json!({}),
        "0.0.0.0:0".parse().unwrap(),
        &HeaderMap::new(),
    )
    .await;

    // Task 18/19: periodic revalidation -- RemoteServer ownership and
    // the caller's own CloudDesk session must both still be valid, or
    // this terminal is torn down promptly rather than only at the next
    // reconnect attempt.
    let mut revalidate = tokio::time::interval(Duration::from_secs(5));
    revalidate.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    let mut close_reason = "closed";
    let exit_code;
    loop {
        tokio::select! {
            _ = revalidate.tick() => {
                let server_ok = store.get(&principal.user_id, &server_id).await.is_ok();
                let session_ok = if let Some(cookie) = &cookie {
                    auth.authenticate(cookie.split('=').nth(1).unwrap_or(cookie)).await.is_ok()
                } else {
                    false
                };
                if !server_ok || !session_ok {
                    let _ = socket.send(Message::Text(
                        serde_json::to_string(&ServerMessage::Revoked).unwrap_or_default().into(),
                    )).await;
                    close_reason = "revoked";
                    exit_code = None;
                    break;
                }
            }
            client = socket.recv() => {
                let Some(Ok(message)) = client else { exit_code = None; break; };
                match message {
                    Message::Binary(data) => {
                        if terminal.write_input(&data).await.is_err() {
                            exit_code = None;
                            break;
                        }
                    }
                    Message::Text(text) => {
                        match serde_json::from_str::<ClientMessage>(text.as_str()) {
                            Ok(ClientMessage::Resize { cols, rows }) => {
                                // Task 33: bounded, validated dimensions --
                                // never passed unchecked into the SSH library.
                                if valid_dimensions(cols, rows) {
                                    let _ = terminal.resize(cols, rows).await;
                                }
                            }
                            Ok(ClientMessage::Close) => { close_reason = "closed"; exit_code = None; break; }
                            Err(_) => {} // Task 32: malformed message, ignored safely.
                        }
                    }
                    Message::Close(_) => { exit_code = None; break; }
                    Message::Ping(data) => { let _ = socket.send(Message::Pong(data)).await; }
                    Message::Pong(_) => {}
                }
            }
            event = terminal.next_event() => {
                match event {
                    Some(TerminalEvent::Output(data)) => {
                        if socket.send(Message::Binary(data.into())).await.is_err() {
                            exit_code = None;
                            break;
                        }
                    }
                    Some(TerminalEvent::Exit { code }) => {
                        let _ = socket.send(Message::Text(
                            serde_json::to_string(&ServerMessage::Exit { code }).unwrap_or_default().into(),
                        )).await;
                        close_reason = "exited";
                        exit_code = code;
                        break;
                    }
                    Some(TerminalEvent::Closed) | None => {
                        close_reason = "exited";
                        exit_code = None;
                        break;
                    }
                }
            }
        }
    }

    let _ = terminal.close().await;
    drop(socket);
    let _ = audit_remote_action(
        &auth,
        &principal,
        "terminal.closed",
        &server_id,
        "success",
        json!({ "reason": close_reason, "exit_code": exit_code }),
        "0.0.0.0:0".parse().unwrap(),
        &HeaderMap::new(),
    )
    .await;
}
