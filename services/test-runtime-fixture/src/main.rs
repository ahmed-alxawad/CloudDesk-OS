//! TEST FIXTURE ONLY.
//!
//! A disposable HTTP+WebSocket server used solely to prove the Phase 6
//! runtime orchestrator's lifecycle/health/proxy plumbing
//! (`crates/orchestrator`'s live test suite). This is not part of any
//! `CloudDesk` product surface and must never be treated as evidence
//! that Code, Office, or Brave are implemented -- see
//! `CLAUDE_ENGINEERING_CHECKPOINT.md`.
//!
//! Controlled entirely by environment variables (set by the orchestrator
//! test suite, never by request input):
//!
//! - `PORT` (required): port to listen on, 127.0.0.1 only.
//! - `CRASH_AFTER_MS`: exit(7) after this many milliseconds, to test
//!   crash detection.
//! - `IGNORE_SIGTERM`: install a SIGTERM handler that does nothing, to
//!   test the SIGKILL fallback.
//! - `SPAWN_CHILD`: spawn a long-lived child `sleep` process, to test
//!   process-tree termination.
//! - `CPU_HOG`: spin a busy loop on a background thread.
//! - `MEM_HOG_MB`: allocate and hold this many megabytes.

use axum::{
    extract::ws::{Message, WebSocket, WebSocketUpgrade},
    response::IntoResponse,
    routing::get,
    Router,
};
use std::time::Duration;

#[tokio::main]
async fn main() {
    let port: u16 = std::env::var("PORT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);

    if let Ok(ms) = std::env::var("CRASH_AFTER_MS") {
        if let Ok(ms) = ms.parse::<u64>() {
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(ms)).await;
                std::process::exit(7);
            });
        }
    }

    if std::env::var("IGNORE_SIGTERM").is_ok() {
        #[cfg(unix)]
        ignore_sigterm();
    }

    if std::env::var("SPAWN_CHILD").is_ok() {
        let _ = std::process::Command::new("sleep").arg("300").spawn();
    }

    if std::env::var("CPU_HOG").is_ok() {
        std::thread::spawn(|| loop {
            let mut x: u64 = 0;
            for i in 0..10_000_000_u64 {
                x = x.wrapping_add(i);
            }
            std::hint::black_box(x);
        });
    }

    if let Ok(mb) = std::env::var("MEM_HOG_MB") {
        if let Ok(mb) = mb.parse::<usize>() {
            let block = vec![1_u8; mb * 1024 * 1024];
            std::mem::forget(block);
        }
    }

    // Task 11/12 log-abuse coverage: writes a raw byte payload (hex-
    // encoded so the shell/env-var boundary can carry arbitrary bytes,
    // including invalid UTF-8 and control/ANSI sequences) to stdout,
    // optionally repeated -- lets a *test* construct exactly the
    // hostile log content it wants to feed through the orchestrator's
    // bounded log capture. Still trusted, compiled-in test-only
    // behavior: the payload comes from an environment variable the
    // test harness sets, never from a client HTTP request.
    if let Ok(hex) = std::env::var("LOG_TEST_PAYLOAD_HEX") {
        if let Some(bytes) = decode_hex(&hex) {
            use std::io::Write;
            let repeat: usize = std::env::var("LOG_TEST_REPEAT")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(1);
            let stdout = std::io::stdout();
            let mut handle = stdout.lock();
            for _ in 0..repeat {
                let _ = handle.write_all(&bytes);
                let _ = handle.write_all(b"\n");
            }
            let _ = handle.flush();
        }
    }

    let app = Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .route("/echo", get(echo))
        .route("/echo-headers", get(echo_headers))
        .route("/ws", get(ws_handler));

    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
        .await
        .expect("bind");
    axum::serve(listener, app).await.expect("serve");
}

/// Phase 7 closure Task 5 -- reflects every header this fixture
/// actually received (as JSON) so a test can prove, against the real
/// end-to-end `clouddeskd` proxy chain, that `CloudDesk`'s own
/// `STRIPPED_REQUEST_HEADERS` list (`crates/orchestrator/src/proxy.rs`,
/// shared code with no per-kind branch -- the same path Code's proxy
/// route uses) actually keeps the caller's session cookie/auth header
/// from ever reaching the instance, rather than trusting the source
/// reading/config alone.
async fn echo_headers(headers: axum::http::HeaderMap) -> impl IntoResponse {
    let map: std::collections::BTreeMap<String, String> = headers
        .iter()
        .map(|(name, value)| {
            (
                name.as_str().to_owned(),
                String::from_utf8_lossy(value.as_bytes()).into_owned(),
            )
        })
        .collect();
    axum::Json(map)
}

async fn echo(
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> impl IntoResponse {
    params.get("msg").cloned().unwrap_or_default()
}

async fn ws_handler(ws: WebSocketUpgrade) -> impl IntoResponse {
    ws.on_upgrade(handle_socket)
}

async fn handle_socket(mut socket: WebSocket) {
    while let Some(Ok(msg)) = socket.recv().await {
        let reply = match msg {
            Message::Text(text) => {
                // Phase 7D: on-demand trigger so a test can observe the
                // *upstream-to-client* direction of the proxy's control-
                // frame relay -- a plain echo can't originate a Ping/
                // Pong itself, and the axum/tungstenite libraries' own
                // automatic Pong-on-Ping behavior only ever answers a
                // frame the fixture *received*, never lets it send an
                // unsolicited one. `Message::Ping`/`Message::Pong` sent
                // here are the fixture genuinely initiating a control
                // frame, for the proxy to relay onward to the real
                // client.
                if let Some(payload) = text.strip_prefix("SEND_PING:") {
                    Some(Message::Ping(payload.as_bytes().to_vec().into()))
                } else if let Some(payload) = text.strip_prefix("SEND_PONG:") {
                    Some(Message::Pong(payload.as_bytes().to_vec().into()))
                } else {
                    Some(Message::Text(text))
                }
            }
            Message::Binary(data) => Some(Message::Binary(data)),
            // Phase 7D: tag exactly what was received (frame kind +
            // payload) as a distinguishable Text reply -- this can only
            // ever be produced by this fixture's own application code
            // actually processing the frame, unlike the underlying
            // WebSocket libraries' automatic per-hop Pong-on-Ping reply
            // (which a client-facing test can observe regardless of
            // whether the proxy relays anything to this fixture at
            // all). Proves the *upstream* genuinely saw the frame.
            Message::Ping(data) => Some(Message::Text(
                format!("FIXTURE_SAW_PING:{}", String::from_utf8_lossy(&data)).into(),
            )),
            Message::Pong(data) => Some(Message::Text(
                format!("FIXTURE_SAW_PONG:{}", String::from_utf8_lossy(&data)).into(),
            )),
            Message::Close(_) => break,
        };
        if let Some(reply) = reply {
            if socket.send(reply).await.is_err() {
                break;
            }
        }
    }
}

/// Minimal, dependency-free hex decoder -- avoids adding a `hex` crate
/// dependency to this disposable fixture just for one test knob.
fn decode_hex(value: &str) -> Option<Vec<u8>> {
    if !value.len().is_multiple_of(2) {
        return None;
    }
    (0..value.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&value[i..i + 2], 16).ok())
        .collect()
}

#[cfg(unix)]
fn ignore_sigterm() {
    // tokio's own SIGTERM stream registration replaces the default
    // disposition; simply never acting on receipt is what "ignores
    // SIGTERM" means for this fixture -- no `unsafe` signal-handler
    // installation needed.
    tokio::spawn(async {
        if let Ok(mut term) =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            loop {
                term.recv().await;
                // Deliberately do nothing -- this is what "ignores
                // SIGTERM" means for this fixture.
            }
        }
    });
}
