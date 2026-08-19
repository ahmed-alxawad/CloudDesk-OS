-- Phase 8 remote VFS closure: extends office_wopi_files so a WOPI file
-- identity can point at a document on a user-owned SFTP remote server
-- instead of only a local canonical path (Task 1/2). `remote_server_id`
-- is NULL for local files (unchanged behavior).
--
-- Two different remote servers can legitimately have a document at the
-- same relative path (e.g. "/docs/report.docx" on server A and on
-- server B) -- a bare UNIQUE(canonical_path), as the original table
-- had, would collide across them. `identity_key` disambiguates:
-- for local files it equals canonical_path (identical to today's
-- behavior); for remote files it is `remote:{server_id}:{path}`, unique
-- per user+server+path.
--
-- Built to CREATE the replacement under a *fresh* name first, then
-- rename it into the final `office_wopi_files` name at the end, rather
-- than renaming the original table out of the way first: SQLite's
-- ALTER TABLE RENAME rewrites *other* tables' foreign key definitions
-- to follow a renamed table by default, so renaming the original away
-- would silently repoint `office_wopi_tokens`/`office_locks`'s
-- `REFERENCES office_wopi_files(id)` at the old table's new name --
-- and leave it dangling once that old table is dropped. Renaming a
-- table that *nothing else currently references* (the fresh one) into
-- the real name at the end has nothing to rewrite.
CREATE TABLE office_wopi_files_new (
    id TEXT PRIMARY KEY NOT NULL,
    remote_server_id TEXT NULL REFERENCES remote_servers(id) ON DELETE CASCADE,
    canonical_path TEXT NOT NULL,
    identity_key TEXT NOT NULL UNIQUE,
    generation INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL
);

INSERT INTO office_wopi_files_new (id, remote_server_id, canonical_path, identity_key, generation, created_at)
    SELECT id, NULL, canonical_path, canonical_path, generation, created_at
    FROM office_wopi_files;

DROP TABLE office_wopi_files;

ALTER TABLE office_wopi_files_new RENAME TO office_wopi_files;

CREATE INDEX idx_office_wopi_files_remote_server ON office_wopi_files(remote_server_id);
