-- Phase 8 (LibreOffice/Collabora Runtime): CloudDesk's own WOPI host
-- state. Collabora never receives a raw host path -- `id` here is the
-- only file identifier ever handed to the browser/Collabora, resolved
-- server-side to `canonical_path` on every operation. `generation` is
-- bumped on every successful PutFile and folded into the WOPI
-- Version/ETag string together with the live file size/mtime, giving
-- Collabora a value that changes both on CloudDesk-driven saves and on
-- externally-modified files (detected by comparing live stat() against
-- what was last recorded here).
CREATE TABLE office_wopi_files (
    id TEXT PRIMARY KEY NOT NULL,
    canonical_path TEXT NOT NULL UNIQUE,
    generation INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL
);

-- Short-lived, single-purpose WOPI access tokens (Task 7). Only the
-- SHA-256 hash is stored, mirroring how `clouddesk_auth` stores session
-- tokens -- the raw token exists only in the response handed to the
-- browser/Collabora and is never persisted or logged in the clear.
-- `read_write` is snapshotted from CloudDesk's authorization at
-- issuance time but is re-verified fresh (never trusted stale) on every
-- WOPI call that matters (Task 8/41). `runtime_instance_id` binds the
-- token to the Office runtime generation it was issued for, so a
-- stopped/restarted runtime's old tokens stop mattering.
CREATE TABLE office_wopi_tokens (
    token_hash TEXT PRIMARY KEY NOT NULL,
    user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    file_id TEXT NOT NULL REFERENCES office_wopi_files(id) ON DELETE CASCADE,
    read_write INTEGER NOT NULL CHECK (read_write IN (0, 1)),
    runtime_instance_id TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL
);
CREATE INDEX idx_office_wopi_tokens_file ON office_wopi_tokens(file_id);
CREATE INDEX idx_office_wopi_tokens_user ON office_wopi_tokens(user_id);

-- Server-authoritative locks (Task 14), one live lock per file,
-- surviving a `clouddeskd` restart (Task 68). `snapshot_size`/
-- `snapshot_mtime` capture the file's state at lock-acquisition time,
-- so PutFile can detect an out-of-band external modification that
-- happened *during* the locked session (Task 13/17) even though the
-- same lock owner is saving.
CREATE TABLE office_locks (
    file_id TEXT PRIMARY KEY NOT NULL REFERENCES office_wopi_files(id) ON DELETE CASCADE,
    lock_value TEXT NOT NULL,
    owner_user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    snapshot_size INTEGER NOT NULL,
    snapshot_mtime INTEGER NOT NULL,
    created_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL
);
