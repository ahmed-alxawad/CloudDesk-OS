//! Phase 8: `CloudDesk`'s own WOPI host domain logic (no HTTP types here
//! -- the axum handlers live in `lib.rs`'s `wopi_api` module, mirroring
//! how `code_runtime.rs` holds Code's pure runtime-descriptor logic
//! while `lib.rs`'s `runtime` module holds its HTTP handlers).
//!
//! ## Security model
//!
//! - File identity handed to the browser/Collabora is always an opaque
//!   `office_wopi_files.id`, never a raw host path (Task 6).
//! - WOPI access tokens are short-lived, single-purpose, and verified
//!   fresh against `CloudDesk`'s *current* authorization on every call
//!   (Task 7/8/41) -- a revoked assigned root fails the very next WOPI
//!   operation, not just future token issuance.
//! - `read_write` is never trusted from the stored token snapshot alone
//!   for anything security-relevant; `verify_token` re-derives it from
//!   live authorization state every time.
//! - Locks are server-authoritative, persisted in `SQLite` (survive a
//!   `clouddeskd` restart, Task 68), and capture a snapshot of the
//!   file's (size, mtime) at acquisition time so an out-of-band
//!   external modification during the locked session is detectable at
//!   save time (Task 13/17), not just between two saves.

use clouddesk_auth::{AuthError, AuthService, SessionPrincipal};
use sha2::{Digest, Sha256};
use sqlx::{Row, SqlitePool};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// WOPI access tokens are refreshed by reopening the document; a
/// 30-minute ceiling bounds how long a stale browser tab can keep
/// editing after `CloudDesk`-side authorization changes, while remaining
/// comfortably longer than a normal editing session needs to re-poll.
pub const TOKEN_TTL_SECONDS: i64 = 30 * 60;
/// Collabora refreshes an active lock roughly every 30 seconds in
/// normal operation; this ceiling is deliberately much longer so a
/// live editing session's own legitimate refresh cadence never races
/// expiry (Task 16), while an abandoned session's lock still becomes
/// recoverable in bounded time rather than permanently.
pub const LOCK_TTL_SECONDS: i64 = 10 * 60;

#[derive(Debug)]
pub enum WopiError {
    NotAuthorized,
    NotFound,
    InvalidToken,
    TokenExpired,
    FileMismatch,
    Database(sqlx::Error),
    Io(std::io::Error),
}

impl From<sqlx::Error> for WopiError {
    fn from(e: sqlx::Error) -> Self {
        Self::Database(e)
    }
}

impl From<std::io::Error> for WopiError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

fn hash_token(raw: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(raw.as_bytes());
    hex::encode(hasher.finalize())
}

/// A file authorized and resolved to its opaque `CloudDesk`-controlled
/// WOPI identity.
pub struct ResolvedFile {
    pub file_id: String,
    pub canonical_path: PathBuf,
    pub read_write: bool,
}

/// Re-authorizes an absolute, already-canonicalizable path against the
/// caller's home directory and their current `assigned_roots`, using
/// the same longest-matching-prefix rule Code's deep-link resolver
/// uses (Phase 7): the most specific containing root wins, so a
/// `read`-only assigned root nested inside home is never silently
/// widened to home's read-write default. Returns `None` if the path
/// isn't authorized at all.
async fn authorize_path(
    auth: &AuthService,
    principal: &SessionPrincipal,
    home: &str,
    canonical_path: &Path,
) -> Result<Option<bool>, WopiError> {
    let mut best: Option<(usize, bool)> = None;
    if canonical_path.starts_with(home) {
        best = Some((home.len(), true));
    }
    for root in auth
        .list_own_assigned_roots(principal)
        .await
        .map_err(auth_error_to_wopi)?
    {
        let Ok(resolved) = auth.resolve_own_assigned_root(principal, &root.id).await else {
            continue;
        };
        if canonical_path.starts_with(&resolved.path) {
            let len = resolved.path.len();
            if best.is_none_or(|(l, _)| len > l) {
                best = Some((len, resolved.read_write));
            }
        }
    }
    Ok(best.map(|(_, rw)| rw))
}

fn auth_error_to_wopi(e: AuthError) -> WopiError {
    match e {
        AuthError::Database(inner) => WopiError::Database(inner),
        _ => WopiError::NotAuthorized,
    }
}

/// Resolves an already-Files-authorized absolute path (the same kind
/// of value Files' own local-file routes already accept, never taken
/// from a raw client-supplied WOPI file ID) to its opaque WOPI file
/// identity, registering a new `office_wopi_files` row if this is the
/// first time this canonical path has been opened in Office. The
/// opaque ID is stable per canonical path (not per user), so two
/// independently authorized users opening the same underlying file
/// share the same lock/version state.
pub async fn resolve_and_register_file(
    pool: &SqlitePool,
    auth: &AuthService,
    principal: &SessionPrincipal,
    home: &str,
    absolute_path: &Path,
) -> Result<ResolvedFile, WopiError> {
    let canonical = tokio::fs::canonicalize(absolute_path)
        .await
        .map_err(WopiError::Io)?;
    let Some(read_write) = authorize_path(auth, principal, home, &canonical).await? else {
        return Err(WopiError::NotAuthorized);
    };
    let canonical_str = canonical.to_string_lossy().into_owned();
    let existing: Option<String> =
        sqlx::query_scalar("SELECT id FROM office_wopi_files WHERE canonical_path = ?")
            .bind(&canonical_str)
            .fetch_optional(pool)
            .await?;
    let file_id = if let Some(id) = existing {
        id
    } else {
        let id = clouddesk_auth::random_identifier(16);
        sqlx::query(
            "INSERT INTO office_wopi_files (id, canonical_path, generation, created_at)
             VALUES (?, ?, 0, ?)
             ON CONFLICT(canonical_path) DO NOTHING",
        )
        .bind(&id)
        .bind(&canonical_str)
        .bind(unix_now())
        .execute(pool)
        .await?;
        // Someone else may have won the race; re-read authoritatively.
        sqlx::query_scalar("SELECT id FROM office_wopi_files WHERE canonical_path = ?")
            .bind(&canonical_str)
            .fetch_one(pool)
            .await?
    };
    Ok(ResolvedFile {
        file_id,
        canonical_path: canonical,
        read_write,
    })
}

/// Looks up an already-registered WOPI file by its opaque ID (used by
/// every WOPI callback, which only ever has the ID, never a path).
pub async fn lookup_file(pool: &SqlitePool, file_id: &str) -> Result<PathBuf, WopiError> {
    let path: Option<String> =
        sqlx::query_scalar("SELECT canonical_path FROM office_wopi_files WHERE id = ?")
            .bind(file_id)
            .fetch_optional(pool)
            .await?;
    path.map(PathBuf::from).ok_or(WopiError::NotFound)
}

/// Issues a new opaque WOPI access token, returning the raw value
/// (given to the browser/Collabora exactly once) -- only its SHA-256
/// hash is ever persisted.
pub async fn issue_token(
    pool: &SqlitePool,
    user_id: &str,
    file_id: &str,
    read_write: bool,
    runtime_instance_id: &str,
) -> Result<String, WopiError> {
    let raw = clouddesk_auth::random_identifier(32);
    let hash = hash_token(&raw);
    let now = unix_now();
    sqlx::query(
        "INSERT INTO office_wopi_tokens
            (token_hash, user_id, file_id, read_write, runtime_instance_id, created_at, expires_at)
         VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&hash)
    .bind(user_id)
    .bind(file_id)
    .bind(read_write)
    .bind(runtime_instance_id)
    .bind(now)
    .bind(now + TOKEN_TTL_SECONDS)
    .execute(pool)
    .await?;
    Ok(raw)
}

/// A verified token, re-authorized against `CloudDesk`'s *current* state
/// -- never trusting the snapshot recorded at issuance time.
pub struct VerifiedToken {
    pub user_id: String,
    pub file_id: String,
    pub canonical_path: PathBuf,
    pub read_write: bool,
}

/// Verifies a raw WOPI access token: must exist, be unexpired, and
/// (if `expected_file_id` is given) be bound to exactly that file --
/// Task 8's "valid token + wrong file" attack. Re-derives `read_write`
/// from the user's *current* authorization (Task 41: a revoked root
/// fails the very next call, not just future token issuance) rather
/// than trusting the value recorded when the token was issued.
pub async fn verify_token(
    pool: &SqlitePool,
    auth: &AuthService,
    token: &str,
    expected_file_id: Option<&str>,
) -> Result<VerifiedToken, WopiError> {
    let hash = hash_token(token);
    let row = sqlx::query(
        "SELECT user_id, file_id, expires_at FROM office_wopi_tokens WHERE token_hash = ?",
    )
    .bind(&hash)
    .fetch_optional(pool)
    .await?
    .ok_or(WopiError::InvalidToken)?;
    let user_id: String = row.get("user_id");
    let file_id: String = row.get("file_id");
    let expires_at: i64 = row.get("expires_at");
    if unix_now() >= expires_at {
        return Err(WopiError::TokenExpired);
    }
    if let Some(expected) = expected_file_id {
        if expected != file_id {
            return Err(WopiError::FileMismatch);
        }
    }
    let canonical_path = lookup_file(pool, &file_id).await?;

    // Re-derive current identity/home and current authorization --
    // never trust the token's own stored read_write flag for anything
    // security-relevant.
    let identity = clouddesk_linux::lookup_uid(
        sqlx::query_scalar::<_, i64>("SELECT linux_uid FROM users WHERE id = ?")
            .bind(&user_id)
            .fetch_optional(pool)
            .await?
            .and_then(|v| u32::try_from(v).ok())
            .ok_or(WopiError::NotAuthorized)?,
    )
    .ok()
    .flatten()
    .ok_or(WopiError::NotAuthorized)?;
    let principal = synthetic_principal(&user_id);
    let read_write = authorize_path(
        auth,
        &principal,
        &identity.home.to_string_lossy(),
        &canonical_path,
    )
    .await?
    .ok_or(WopiError::NotAuthorized)?;

    Ok(VerifiedToken {
        user_id,
        file_id,
        canonical_path,
        read_write,
    })
}

/// A synthetic `SessionPrincipal` used only to satisfy
/// `AuthService::{list_own_assigned_roots,resolve_own_assigned_root}`'s
/// signature for WOPI callbacks, which authenticate via the scoped
/// WOPI token (Task 29), never a `CloudDesk` browser session. Those two
/// functions only ever read `.user_id` from the principal they're
/// given -- never roles/capabilities -- so a principal with no
/// capabilities is safe here and cannot be used to bypass any
/// capability-gated `CloudDesk` API.
fn synthetic_principal(user_id: &str) -> SessionPrincipal {
    SessionPrincipal {
        user_id: user_id.to_owned(),
        username: String::new(),
        session_id_hash: String::new(),
        roles: Vec::new(),
        capabilities: HashSet::new(),
        step_up_expires_at: None,
    }
}

/// Live (size, mtime-seconds) snapshot of a file on disk.
pub async fn stat_snapshot(path: &Path) -> Result<(u64, i64), WopiError> {
    let metadata = tokio::fs::metadata(path).await?;
    let mtime = metadata
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(0));
    Ok((metadata.len(), mtime))
}

/// The WOPI `Version` string `CloudDesk` reports: a persisted per-file
/// generation counter (bumped on every successful save) combined with
/// the file's live size/mtime, so the value changes both on
/// `CloudDesk`-driven saves and on any externally-caused modification.
pub async fn current_version(
    pool: &SqlitePool,
    file_id: &str,
    path: &Path,
) -> Result<String, WopiError> {
    let generation: i64 =
        sqlx::query_scalar("SELECT generation FROM office_wopi_files WHERE id = ?")
            .bind(file_id)
            .fetch_one(pool)
            .await?;
    let (size, mtime) = stat_snapshot(path).await.unwrap_or((0, 0));
    Ok(format!("{generation}-{size}-{mtime}"))
}

pub struct LockInfo {
    pub lock_value: String,
    pub owner_user_id: String,
    pub expires_at: i64,
}

/// Reads the current lock, transparently treating an expired lock as
/// absent (Task 16: recoverable after expiry, never a permanent dead
/// lock) rather than requiring a separate sweep.
pub async fn get_lock(pool: &SqlitePool, file_id: &str) -> Result<Option<LockInfo>, WopiError> {
    let row = sqlx::query(
        "SELECT lock_value, owner_user_id, expires_at FROM office_locks WHERE file_id = ?",
    )
    .bind(file_id)
    .fetch_optional(pool)
    .await?;
    let Some(row) = row else { return Ok(None) };
    let expires_at: i64 = row.get("expires_at");
    if unix_now() >= expires_at {
        return Ok(None);
    }
    Ok(Some(LockInfo {
        lock_value: row.get("lock_value"),
        owner_user_id: row.get("owner_user_id"),
        expires_at,
    }))
}

/// Acquires a fresh lock. Caller must already have confirmed no live,
/// unexpired lock exists (or that this exact owner+value already holds
/// it, for an idempotent re-LOCK) -- this function itself just writes
/// the row, upserting over any expired lock.
pub async fn acquire_lock(
    pool: &SqlitePool,
    file_id: &str,
    lock_value: &str,
    owner_user_id: &str,
    path: &Path,
) -> Result<(), WopiError> {
    let (size, mtime) = stat_snapshot(path).await.unwrap_or((0, 0));
    let now = unix_now();
    sqlx::query(
        "INSERT INTO office_locks
            (file_id, lock_value, owner_user_id, snapshot_size, snapshot_mtime, created_at, expires_at)
         VALUES (?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT(file_id) DO UPDATE SET
            lock_value = excluded.lock_value,
            owner_user_id = excluded.owner_user_id,
            snapshot_size = excluded.snapshot_size,
            snapshot_mtime = excluded.snapshot_mtime,
            created_at = excluded.created_at,
            expires_at = excluded.expires_at",
    )
    .bind(file_id)
    .bind(lock_value)
    .bind(owner_user_id)
    .bind(i64::try_from(size).unwrap_or(i64::MAX))
    .bind(mtime)
    .bind(now)
    .bind(now + LOCK_TTL_SECONDS)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn refresh_lock(pool: &SqlitePool, file_id: &str) -> Result<(), WopiError> {
    sqlx::query("UPDATE office_locks SET expires_at = ? WHERE file_id = ?")
        .bind(unix_now() + LOCK_TTL_SECONDS)
        .bind(file_id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn release_lock(pool: &SqlitePool, file_id: &str) -> Result<(), WopiError> {
    sqlx::query("DELETE FROM office_locks WHERE file_id = ?")
        .bind(file_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Returns the lock's own recorded (size, mtime) snapshot -- used to
/// detect an external modification that happened *during* the locked
/// session (Task 13/17).
pub async fn lock_snapshot(
    pool: &SqlitePool,
    file_id: &str,
) -> Result<Option<(u64, i64)>, WopiError> {
    let row =
        sqlx::query("SELECT snapshot_size, snapshot_mtime FROM office_locks WHERE file_id = ?")
            .bind(file_id)
            .fetch_optional(pool)
            .await?;
    Ok(row.map(|r| {
        let size: i64 = r.get("snapshot_size");
        let mtime: i64 = r.get("snapshot_mtime");
        (u64::try_from(size).unwrap_or(0), mtime)
    }))
}

/// Deletes every lock row whose expiry has already passed, returning
/// how many were removed (Task 1/16).
///
/// This is a *storage* cleanup, not the authorization boundary: every
/// read path already goes through `get_lock`, which treats an expired
/// row as absent, so an abandoned session can never block a legitimate
/// new LOCK even if this sweep has not run yet. Correctness therefore
/// does not depend on sweep timing -- the sweep only stops the table
/// from accumulating dead rows forever.
pub async fn sweep_expired_locks(pool: &SqlitePool) -> Result<u64, WopiError> {
    let result = sqlx::query("DELETE FROM office_locks WHERE expires_at <= ?")
        .bind(unix_now())
        .execute(pool)
        .await?;
    Ok(result.rows_affected())
}

/// Bumps the per-file generation counter -- called after a successful
/// atomic save.
pub async fn bump_generation(pool: &SqlitePool, file_id: &str) -> Result<(), WopiError> {
    sqlx::query("UPDATE office_wopi_files SET generation = generation + 1 WHERE id = ?")
        .bind(file_id)
        .execute(pool)
        .await?;
    Ok(())
}
