//! Phase 7: the real `code-server` OCI runtime definition, and the
//! small trusted marker file mechanism that lets the runtime HTTP
//! handlers (which have access to the authenticated principal's mapped
//! Linux identity and authorized workspaces via `clouddesk_auth`) hand
//! that resolved state to the OCI adapter's `run_as`/`extra_mounts`/
//! `extra_env`/`command` closures (which only see `InstanceContext`,
//! deliberately decoupled from `clouddesk_auth` -- see
//! `crates/orchestrator`'s own docs on why).
//!
//! Every closure here reads only server-side state written earlier in
//! the same request by trusted code -- never a client-supplied value.
//!
//! ## Workspace model (Phase 7 Task 2)
//!
//! A Code container mounts exactly two directories:
//!
//! - `/profile` -- the user's own real home directory, always mounted
//!   read-write. This is where code-server keeps its persistent state
//!   (`~/.local/share/code-server`, `~/.config/code-server`, settings,
//!   installed extensions). It never changes when the workspace does.
//! - `/workspace` -- the *currently selected* authorized directory
//!   (either the user's home by default, or one of their
//!   `assigned_roots` rows), mounted read-only or read-write according
//!   to that row's `access_mode`. code-server is started with
//!   `/workspace` as its open folder.
//!
//! Workspace identity is always the existing `assigned_roots.id` --
//! never a raw host path supplied by the browser. See
//! `resolve_workspace` in `lib.rs`'s `runtime` module for the
//! authorization step that turns a client-supplied `workspace_id` into
//! the canonical path recorded here.
//!
//! Switching workspace is implemented as stop-the-instance,
//! re-authorize, start-a-new-generation-with-a-new-mount -- OCI gives
//! no live remount primitive, so this module does not attempt one.

use clouddesk_auth::{AuthError, AuthService, SessionPrincipal};
use clouddesk_orchestrator::adapter::InstanceContext;
use clouddesk_orchestrator::oci::OciSpec;
use clouddesk_orchestrator::RuntimeKind;
use serde::{Deserialize, Serialize};
use sqlx::{Row, SqlitePool};
use std::sync::Arc;

/// Filename of the trusted identity/workspace marker written into an
/// instance's own state directory before it starts (Task 10/11/15: run
/// as the owning user's real mapped Linux identity, never root, never
/// `cloudeskd`'s own identity; Task 2: mount only the one
/// server-authorized workspace this instance was started with).
pub const CODE_IDENTITY_MARKER: &str = ".code_identity.json";

/// Fixed in-container path the currently selected workspace is always
/// mounted at, regardless of which host directory it resolves to --
/// this keeps workspace-relative paths (used by the Files -> Code
/// deep-link) stable across switches.
pub const WORKSPACE_CONTAINER_PATH: &str = "/workspace";

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct CodeIdentityMarker {
    pub uid: u32,
    pub gid: u32,
    pub home: String,
    /// The `assigned_roots.id` currently selected as this instance's
    /// workspace, or `None` when the default (home) workspace is in
    /// use. Re-resolved (Task 11 reauthorization) on every start,
    /// restart, and switch -- never trusted as still-valid just because
    /// it was valid when this marker was written.
    pub workspace_id: Option<String>,
    /// Canonical server-side path mounted at `/workspace`. Equal to
    /// `home` for the default workspace.
    pub workspace_path: String,
    pub workspace_read_write: bool,
    /// One-shot: a workspace-relative file to additionally open on this
    /// start (Files -> Code deep-link foundation, Task 10). Always a
    /// relative path already validated to stay inside `workspace_path`
    /// by the caller -- never an absolute path taken from the request.
    pub open_relative_file: Option<String>,
}

fn read_identity_marker(ctx: &InstanceContext) -> Option<CodeIdentityMarker> {
    let raw = std::fs::read(ctx.state_dir.join(CODE_IDENTITY_MARKER)).ok()?;
    serde_json::from_slice(&raw).ok()
}

/// Encodes read-only-ness into the Docker volume spec's container-path
/// segment (`host:container:ro`), reusing the existing 2-tuple
/// `OciMountBuilder` signature rather than widening the orchestrator's
/// generic mount type for one adapter's needs.
fn mount_target(read_write: bool) -> String {
    if read_write {
        WORKSPACE_CONTAINER_PATH.to_owned()
    } else {
        format!("{WORKSPACE_CONTAINER_PATH}:ro")
    }
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
            let mut args = vec![
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
                // Phase 7 closure Task 4: code-server ships a built-in
                // path-based local-port proxy (`/proxy/{port}/...` and
                // `/absproxy/{port}/...`, gated only by
                // `ensureProxyEnabled`/`proxyEnabled` in its own
                // `out/node/http.js`) that forwards to
                // `http://0.0.0.0:{port}/...` *within the container's
                // own network namespace* -- confirmed live: a harmless
                // in-container echo listener on port 9999 was reachable
                // through it before this flag, and returned 403
                // Forbidden after. `getProxyTarget()` in
                // `out/node/routes/pathProxy.js` only ever accepts an
                // integer port (`parseInt(req.params.port, 10)`, NaN ->
                // 400) -- there is no hostname-injection path to an
                // arbitrary external host through this specific
                // mechanism. Still disabled outright: CloudDesk has no
                // product feature that depends on it, and leaving an
                // unused, unaudited network-reachability primitive
                // enabled inside every Code container is needless
                // trusted surface with zero benefit. `--proxy-domain`
                // (the separate subdomain-based variant) was already
                // never set, so this closes both code paths.
                "--disable-proxy".to_owned(),
                // Reverse-proxied under a per-instance path, not at
                // the origin root (Task 24) -- code-server's own
                // documented mechanism for exactly this.
                "--abs-proxy-base-path".to_owned(),
                format!(
                    "/api/v1/runtime-instances/code/{}/proxy",
                    ctx.id.instance_id
                ),
                WORKSPACE_CONTAINER_PATH.to_owned(),
            ];
            // code-server (inheriting VS Code's CLI) opens any
            // additional positional path argument as a file within the
            // already-opened folder -- this is the Files -> Code
            // deep-link foundation (Task 10): a specific file, not just
            // the IDE home page.
            if let Some(marker) = read_identity_marker(ctx) {
                if let Some(relative) = marker.open_relative_file {
                    args.push(format!("{WORKSPACE_CONTAINER_PATH}/{relative}"));
                }
            }
            args
        })),
        extra_mounts: Some(Arc::new(|ctx: &InstanceContext| {
            let Some(marker) = read_identity_marker(ctx) else {
                return Vec::new();
            };
            vec![
                // Profile: the user's own real, already-authorized home
                // directory (Task 9: reuses the existing CloudDesk
                // Linux-identity/VFS authorization model rather than
                // inventing a new workspace-root concept), always
                // read-write, always mounted at the identical path --
                // this is where code-server's own persistent state
                // lands, and it never changes when the workspace does
                // (Task 2 requirement: switching workspace must not
                // destroy settings/extensions/history).
                (marker.home.clone(), marker.home),
                // Workspace: the one currently selected, re-authorized
                // assigned root (or home by default), mounted at the
                // fixed `/workspace` path with the access mode it was
                // actually granted -- never silently upgraded to
                // writable (Task 2 requirement 6).
                (
                    marker.workspace_path,
                    mount_target(marker.workspace_read_write),
                ),
            ]
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
        extra_capabilities: &[],
        add_host_gateway: false,
        graceful_stop: None,
        network_name: None,
        network_subnet: None,
    }
}

/// A workspace resolved and authorized for the current request, ready
/// to be written into a `CodeIdentityMarker`.
pub struct ResolvedWorkspace {
    pub workspace_id: Option<String>,
    pub path: String,
    pub read_write: bool,
}

/// Resolves the workspace a Code start should mount, per Phase 7 Task 2:
///
/// - An explicit `requested` workspace ID is authorized and used as-is;
///   a bad/revoked/foreign ID here is a hard error (never silently
///   substituted -- the caller asked for a specific workspace).
/// - With no explicit request, the user's last-used workspace (if any)
///   is re-authorized and reused. If it no longer resolves (deleted or
///   revoked since last use), this falls back to the default (home)
///   workspace rather than failing an implicit "just reopen Code"
///   request.
/// - With no explicit request and no (resolvable) last-used workspace,
///   the default is the user's home directory.
pub async fn resolve_workspace(
    auth: &AuthService,
    principal: &SessionPrincipal,
    home: &str,
    requested: Option<&str>,
) -> Result<ResolvedWorkspace, AuthError> {
    if let Some(id) = requested {
        let resolved = auth.resolve_own_assigned_root(principal, id).await?;
        return Ok(ResolvedWorkspace {
            workspace_id: Some(resolved.id),
            path: resolved.path,
            read_write: resolved.read_write,
        });
    }
    if let Ok(Some(last_id)) = last_workspace_id(auth.pool(), &principal.user_id).await {
        if let Ok(resolved) = auth.resolve_own_assigned_root(principal, &last_id).await {
            return Ok(ResolvedWorkspace {
                workspace_id: Some(resolved.id),
                path: resolved.path,
                read_write: resolved.read_write,
            });
        }
    }
    Ok(ResolvedWorkspace {
        workspace_id: None,
        path: home.to_owned(),
        read_write: true,
    })
}

/// Reads the user's last-used Code workspace ID, if any. A missing row
/// (never selected a non-default workspace) is `Ok(None)`, not an
/// error.
pub async fn last_workspace_id(
    pool: &SqlitePool,
    user_id: &str,
) -> Result<Option<String>, sqlx::Error> {
    let row = sqlx::query("SELECT last_workspace_id FROM code_user_state WHERE owner_user_id = ?")
        .bind(user_id)
        .fetch_optional(pool)
        .await?;
    Ok(row.and_then(|r| r.get::<Option<String>, _>("last_workspace_id")))
}

/// Persists the user's last-used Code workspace ID. Callers must only
/// invoke this *after* the corresponding Code instance has actually
/// become healthy (Task 2 item 5) -- a failed or still-starting
/// selection must never become the persistent default.
pub async fn set_last_workspace_id(
    pool: &SqlitePool,
    user_id: &str,
    workspace_id: Option<&str>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO code_user_state (owner_user_id, last_workspace_id, updated_at)
         VALUES (?, ?, ?)
         ON CONFLICT(owner_user_id) DO UPDATE SET
            last_workspace_id = excluded.last_workspace_id,
            updated_at = excluded.updated_at",
    )
    .bind(user_id)
    .bind(workspace_id)
    .bind(unix_now())
    .execute(pool)
    .await?;
    Ok(())
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}
