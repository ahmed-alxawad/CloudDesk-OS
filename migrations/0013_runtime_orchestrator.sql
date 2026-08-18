-- Global per-kind enable/disable (Task 8). Media is intentionally not a
-- row here -- it has its own runtime.media.enabled setting from Phase 3
-- and is not routed through this manager.
CREATE TABLE runtime_config (
    kind TEXT PRIMARY KEY,
    enabled INTEGER NOT NULL DEFAULT 0 CHECK (enabled IN (0, 1)),
    updated_at INTEGER NOT NULL
);
INSERT INTO runtime_config (kind, enabled, updated_at) VALUES
    ('code', 0, unixepoch()),
    ('office', 0, unixepoch()),
    ('browser', 0, unixepoch());

-- Per-user runtime instance bookkeeping (Task 26). A row here is a
-- *hint* for management/recovery, reconciled against live process
-- reality on every clouddeskd startup (Task 27) -- never trusted as
-- authoritative on its own. See crates/orchestrator/src/manager.rs.
CREATE TABLE runtime_instances (
    kind TEXT NOT NULL,
    owner_user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    instance_id TEXT NOT NULL,
    generation INTEGER NOT NULL,
    state TEXT NOT NULL,
    persistence TEXT NOT NULL CHECK (persistence IN ('persistent', 'ephemeral')),
    port INTEGER,
    pid INTEGER,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    last_activity_at INTEGER NOT NULL,
    restart_count INTEGER NOT NULL DEFAULT 0,
    exit_code INTEGER,
    exit_signal TEXT,
    failure_message TEXT,
    PRIMARY KEY (kind, owner_user_id, instance_id)
);
CREATE INDEX idx_runtime_instances_owner ON runtime_instances(owner_user_id);
