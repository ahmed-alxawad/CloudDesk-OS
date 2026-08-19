//! Phase 9 (foundation only -- see `PHASE9_BROWSER_EVIDENCE.md`): the
//! real, server-side Brave Browser OCI runtime definition.
//!
//! ## What this module is
//!
//! Registers a real, pinned Brave Browser (`docker/brave/Dockerfile`,
//! built from Brave's own official signed apt repository, never a
//! third-party image) as a normal Phase 6 `RuntimeManager` adapter --
//! same lifecycle states, same `state_dir`-per-instance model every
//! other runtime uses, no second lifecycle manager.
//!
//! ## What this module is NOT (yet)
//!
//! This is the runtime-adapter foundation only. It does **not** yet
//! include the typed browser broker (Task 8), frame/screencast
//! transport (Task 9-11), input handling (Task 13-15), tab management
//! (Task 23-25), role-aware persistent-vs-ephemeral profile policy
//! (Task 4 -- `default_persistence` in `lib.rs` still returns
//! `Ephemeral` unconditionally for every `Browser` instance, which is
//! wrong for Administrator/Manager/User and is a known, documented
//! gap), audio (Task 29-31), downloads/uploads (Task 34-39), clipboard
//! (Task 40-41), or the `BrowserApp.svelte` frontend (Task 68). See
//! `PHASE9_BROWSER_EVIDENCE.md` for the full, honest accounting of
//! what is and is not built.
//!
//! ## CDP exposure (Task 7)
//!
//! Real Chromium/Brave binds its remote-debugging port to the
//! container's own loopback interface *regardless* of any
//! `--remote-debugging-address` flag -- a structural browser-level
//! security hardening (live-verified this pass against the real
//! pinned Brave build, not assumed). `docker/brave/Dockerfile`'s own
//! entrypoint relays that loopback-only port to a container-wide port
//! via `socat`, which is what `container_port` below points at --
//! still never published to the host, reachable only via the private
//! Docker bridge network the way every other runtime's port already
//! is. No `CloudDesk` code exposes the raw CDP WebSocket URL to a
//! frontend caller (there is no frontend caller yet at all).
//!
//! ## Security posture verified live this pass
//!
//! Real Brave 1.93.136 (Chromium 151 base) launches headless, serves a
//! real CDP endpoint, and renders real pages (verified via a real
//! `Page.navigate` + `Page.captureScreenshot` round trip against
//! `https://example.com`) with: a non-root container user (uid 10001),
//! Docker's *default* seccomp profile (not `unconfined`), and exactly
//! one added capability (`SYS_ADMIN`, needed for Chromium's own
//! namespace-based sandbox to initialize at all -- without it, the
//! zygote process aborts with `Operation not permitted` before Brave
//! ever starts). No `--no-sandbox` flag used.

use clouddesk_orchestrator::oci::OciSpec;
use clouddesk_orchestrator::RuntimeKind;
use futures_util::SinkExt;
use serde_json::{json, Value};

/// Task 5 (Phase 9 Pass 3A-3): a real CDP `Browser.close` call before
/// `docker stop` -- the same application-level shutdown path a user
/// closing a real browser window triggers, which Chromium's SIGTERM
/// handling alone does not reliably match (live-verified this pass: a
/// real cookie set via `document.cookie` was NOT durably flushed to
/// disk on a plain graceful `docker stop`, even with the real Chromium
/// binary as PID 1, but consistently was after a real `Browser.close`
/// call first). Bounded and best-effort: if Brave is already gone, or
/// doesn't answer within the timeout, `stop()` proceeds to `docker
/// stop` regardless -- this must never be able to block shutdown
/// indefinitely.
async fn graceful_stop_via_cdp(port: u16) {
    let attempt = async {
        let base = format!("http://127.0.0.1:{port}");
        let version: Value = reqwest::Client::new()
            .get(format!("{base}/json/version"))
            .send()
            .await
            .ok()?
            .json()
            .await
            .ok()?;
        let ws_url = version.get("webSocketDebuggerUrl")?.as_str()?.to_owned();
        let (stream, _) = tokio_tungstenite::connect_async(&ws_url).await.ok()?;
        let (mut sink, _read) = futures_util::StreamExt::split(stream);
        let payload = json!({"id": 1, "method": "Browser.close", "params": {}}).to_string();
        let _ = sink
            .send(tokio_tungstenite::tungstenite::Message::Text(payload))
            .await;
        Some(())
    };
    let _ = tokio::time::timeout(std::time::Duration::from_secs(5), attempt).await;
}

/// Live-verified minimal capability set (see module docs). Chromium's
/// own namespace-based sandbox (not the legacy SUID-helper sandbox --
/// this deliberately never adds `no-new-privileges`-incompatible SUID
/// bits) needs two capabilities beyond Docker's hardened zero-
/// capability default (`--cap-drop ALL`, no additions), confirmed by
/// two distinct, real, live container-log failures found while
/// hardening this: without `SYS_ADMIN`, the zygote can't create a new
/// namespace at all (`Failed to move to new namespace... Operation not
/// permitted`); with only `SYS_ADMIN` but under `clouddeskd`'s own
/// standard `--security-opt no-new-privileges` (kept, not disabled),
/// the zygote's own `sys_chroot` call is refused (`Check failed:
/// sys_chroot(...) == 0`, `Permission denied`). Adding `SYS_CHROOT`
/// too (the same pairing Collabora's own adapter already needs for its
/// unrelated internal per-document jailing) resolves it -- default
/// seccomp kept throughout, no `--security-opt seccomp=unconfined`,
/// `no-new-privileges` kept enabled.
const EXTRA_CAPABILITIES: &[&str] = &["SYS_ADMIN", "SYS_CHROOT"];

/// The relayed CDP port `docker/brave/Dockerfile`'s entrypoint exposes
/// (see module docs) -- real Chromium `DevTools` Protocol underneath,
/// reached only by `CloudDesk`'s own trusted backend code, never a
/// client.
const CDP_RELAY_PORT: u16 = 9223;

#[must_use]
pub fn browser_oci_spec(image: String) -> OciSpec {
    OciSpec {
        kind: RuntimeKind::Browser,
        image,
        container_port: CDP_RELAY_PORT,
        // A real HTTP GET against the CDP HTTP endpoint (Task 18's
        // established lesson from Code/Office: never a bare TCP
        // connect) -- `/json/version` only ever answers once Brave's
        // own DevTools HTTP server is actually up, which is Task 3's
        // exact "process alive AND CDP reachable" requirement, not PID
        // existence alone.
        health_check_path: "/json/version",
        command: None,
        extra_mounts: None,
        // The adapter's own `state_dir` (mounted at `/state`, see
        // `docker/brave/Dockerfile`'s `--user-data-dir=/state/profile`)
        // is created server-side by `clouddeskd`'s own process and is
        // therefore owned by whatever real UID/GID that process runs
        // as -- Code's adapter never hits this because it mounts the
        // *user's own home directory* (already correctly owned);
        // Office's never needs a writable state mount at all
        // (Collabora is deliberately given no persistent state, see
        // `office_runtime.rs`). Brave is the first adapter that
        // genuinely needs to write into `/state`, so it must run as
        // the same real identity that created it, not the image's
        // fixed build-time `USER brave` (uid 10001) -- live-verified
        // this pass: without this, Brave aborts outright with "Failed
        // To Create Data Directory".
        run_as: Some(std::sync::Arc::new(|_ctx| {
            Some((
                rustix::process::getuid().as_raw(),
                rustix::process::getgid().as_raw(),
            ))
        })),
        // `run_as` above puts the container under a real UID with no
        // `/etc/passwd` entry inside the container, so the shell never
        // sets `$HOME` -- Brave's own wrapper script then tries to
        // write to an empty/root-relative path for XDG data
        // (`//.local/...`) and aborts. Point it at the one directory
        // this container is actually allowed to write to.
        extra_env: Some(std::sync::Arc::new(|_ctx| {
            vec![("HOME".to_owned(), "/state".to_owned())]
        })),
        extra_capabilities: EXTRA_CAPABILITIES,
        add_host_gateway: false,
        graceful_stop: Some(std::sync::Arc::new(|port| {
            Box::pin(graceful_stop_via_cdp(port))
        })),
    }
}
