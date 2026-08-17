CREATE TABLE upload_sessions (
    id TEXT PRIMARY KEY,
    owner_user_id TEXT NOT NULL REFERENCES users(id),
    virtual_path TEXT NOT NULL,
    temp_path TEXT NOT NULL,
    total_size INTEGER NOT NULL,
    bytes_received INTEGER NOT NULL DEFAULT 0,
    expected_sha256 TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    completed_at INTEGER
);

CREATE INDEX upload_sessions_owner_idx ON upload_sessions(owner_user_id);
CREATE INDEX upload_sessions_updated_idx ON upload_sessions(updated_at);
