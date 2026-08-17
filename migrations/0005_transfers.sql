CREATE TABLE transfer_jobs (
    id TEXT PRIMARY KEY NOT NULL,
    owner_user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    source_json TEXT NOT NULL,
    destination_json TEXT NOT NULL,
    strategy TEXT NOT NULL CHECK (strategy IN ('direct', 'server-relay')),
    state TEXT NOT NULL CHECK (state IN ('queued', 'running', 'paused', 'completed', 'failed', 'cancelled')),
    bytes_total INTEGER,
    bytes_transferred INTEGER NOT NULL DEFAULT 0,
    checksum TEXT,
    attempts INTEGER NOT NULL DEFAULT 0,
    next_attempt_at INTEGER NOT NULL,
    last_error TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE INDEX transfer_jobs_queue_idx ON transfer_jobs(state, next_attempt_at, created_at);
CREATE INDEX transfer_jobs_owner_idx ON transfer_jobs(owner_user_id, created_at DESC);
