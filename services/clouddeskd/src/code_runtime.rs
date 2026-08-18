//! Phase 7: the real `code-server` OCI runtime definition, and the
//! small trusted marker file mechanism that lets `create_instance`
//! (which has access to the authenticated principal's mapped Linux
//! identity via `clouddesk_auth`) hand that identity to the OCI
//! adapter's `run_as`/`extra_mounts`/`extra_env` closures (which only
//! see `InstanceContext`, deliberately decoupled from `clouddesk_auth`
//! -- see `crates/orchestrator`'s own docs on why).
//!
//! Every closure here reads only server-side state written earlier in
//! the same request by trusted code -- never a client-supplied value.

use clouddesk_orchestrator::adapter::InstanceContext;
use clouddesk_orchestrator::oci::OciSpec;
use clouddesk_orchestrator::RuntimeKind;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Filename of the trusted identity marker written into an instance's
/// own state directory before it starts (Task 10/11/15: run as the
/// owning user's real mapped Linux identity, never root, never
/// `cloudeskd`'s own identity).
pub const CODE_IDENTITY_MARKER: &str = ".code_identity.json";

#[derive(Serialize, Deserialize)]
pub struct CodeIdentityMarker {
    pub uid: u32,
    pub gid: u32,
    pub home: String,
}

fn read_identity_marker(ctx: &InstanceContext) -> Option<CodeIdentityMarker> {
    let raw = std::fs::read(ctx.state_dir.join(CODE_IDENTITY_MARKER)).ok()?;
    serde_json::from_slice(&raw).ok()
}

/// The trusted `code-server` runtime descriptor. `image` is a
/// compiled-in, version-pinned reference (Task 33: no request-time
/// download, no client-chosen image) -- see
/// `PHASE7_CODE_EVIDENCE.md` for the exact tag verified this pass.
#[must_use]
pub fn code_oci_spec(image: String) -> OciSpec {
    OciSpec {
        kind: RuntimeKind::Code,
        image,
        container_port: 8080,
        // Unused by `OciAdapter::health` today (bare TCP connect only)
        // -- kept for documentation/future use, see
        // `PHASE7_CODE_EVIDENCE.md`'s honest note on this.
        health_check_path: "/",
        command: Some(Arc::new(|ctx: &InstanceContext| {
            let workspace = read_identity_marker(ctx)
                .map_or_else(|| "/home/coder".to_owned(), |marker| marker.home);
            vec![
                "--bind-addr".to_owned(),
                "0.0.0.0:8080".to_owned(),
                // No second password flow (Task 4): the internal
                // endpoint is never reachable except through
                // CloudDesk's own authenticated proxy (loopback-only
                // published port -- see `OciAdapter`/Task 3), so
                // code-server's own auth is redundant defense that
                // would otherwise require CloudDesk to invent and
                // store a second credential per user.
                "--auth".to_owned(),
                "none".to_owned(),
                "--disable-telemetry".to_owned(),
                "--disable-update-check".to_owned(),
                // Reverse-proxied under a per-instance path, not at
                // the origin root (Task 24) -- code-server's own
                // documented mechanism for exactly this.
                "--abs-proxy-base-path".to_owned(),
                format!(
                    "/api/v1/runtime-instances/code/{}/proxy",
                    ctx.id.instance_id
                ),
                workspace,
            ]
        })),
        extra_mounts: Some(Arc::new(|ctx: &InstanceContext| {
            let Some(marker) = read_identity_marker(ctx) else {
                return Vec::new();
            };
            // The user's own real, already-authorized home directory
            // (Task 9: reuses the existing CloudDesk Linux-identity/
            // VFS authorization model rather than inventing a new
            // workspace-root concept) mounted at the identical path
            // inside the container, so `HOME`/absolute paths/tooling
            // behave exactly as they would in the integrated terminal.
            // This is also where code-server's own
            // `~/.local/share/code-server` and `~/.config/code-server`
            // land -- real host-filesystem persistence, not a
            // separate ephemeral volume (Task 8).
            vec![(marker.home.clone(), marker.home)]
        })),
        run_as: Some(Arc::new(|ctx: &InstanceContext| {
            read_identity_marker(ctx).map(|marker| (marker.uid, marker.gid))
        })),
        extra_env: Some(Arc::new(|ctx: &InstanceContext| {
            let Some(marker) = read_identity_marker(ctx) else {
                return Vec::new();
            };
            vec![("HOME".to_owned(), marker.home)]
        })),
    }
}
