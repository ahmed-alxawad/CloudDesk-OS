CREATE TABLE media_jobs (
    id TEXT PRIMARY KEY,
    owner_user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    source_virtual_path TEXT NOT NULL,
    operation TEXT NOT NULL CHECK (operation IN ('remux', 'transcode')),
    state TEXT NOT NULL CHECK (
        state IN ('queued', 'probing', 'running', 'completed', 'failed', 'cancelled', 'expired')
    ),
    error_class TEXT,
    output_path TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    completed_at INTEGER
);

CREATE INDEX idx_media_jobs_owner ON media_jobs(owner_user_id);
CREATE INDEX idx_media_jobs_updated_at ON media_jobs(updated_at);

INSERT INTO system_settings (key, value_json, updated_at)
VALUES ('runtime.media.enabled', 'false', unixepoch());
