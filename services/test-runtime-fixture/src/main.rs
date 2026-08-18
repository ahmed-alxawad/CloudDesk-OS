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

    let app = Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .route("/echo", get(echo))
        .route("/ws", get(ws_handler));

    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
        .await
        .expect("bind");
    axum::serve(listener, app).await.expect("serve");
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
        if let Message::Text(text) = msg {
            if socket.send(Message::Text(text)).await.is_err() {
                break;
            }
        }
    }
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
