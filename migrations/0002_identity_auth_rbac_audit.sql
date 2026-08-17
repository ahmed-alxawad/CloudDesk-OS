CREATE TABLE users (
    id TEXT PRIMARY KEY NOT NULL,
    username TEXT NOT NULL COLLATE NOCASE UNIQUE,
    display_name TEXT NOT NULL,
    password_hash TEXT NOT NULL,
    totp_secret TEXT,
    totp_enabled INTEGER NOT NULL DEFAULT 0 CHECK (totp_enabled IN (0, 1)),
    linux_uid INTEGER,
    linux_gid INTEGER,
    disabled INTEGER NOT NULL DEFAULT 0 CHECK (disabled IN (0, 1)),
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE TABLE roles (
    id TEXT PRIMARY KEY NOT NULL,
    name TEXT NOT NULL UNIQUE,
    description TEXT NOT NULL,
    created_at INTEGER NOT NULL
);

CREATE TABLE permissions (
    name TEXT PRIMARY KEY NOT NULL,
    description TEXT NOT NULL DEFAULT ''
);

CREATE TABLE role_permissions (
    role_id TEXT NOT NULL REFERENCES roles(id) ON DELETE CASCADE,
    permission_name TEXT NOT NULL REFERENCES permissions(name) ON DELETE CASCADE,
    PRIMARY KEY (role_id, permission_name)
);

CREATE TABLE user_roles (
    user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    role_id TEXT NOT NULL REFERENCES roles(id) ON DELETE CASCADE,
    PRIMARY KEY (user_id, role_id)
);

CREATE TABLE user_permissions (
    user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    permission_name TEXT NOT NULL REFERENCES permissions(name) ON DELETE CASCADE,
    effect TEXT NOT NULL CHECK (effect IN ('allow', 'deny')),
    PRIMARY KEY (user_id, permission_name)
);

CREATE TABLE sessions (
    id_hash TEXT PRIMARY KEY NOT NULL,
    user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    created_at INTEGER NOT NULL,
    last_activity INTEGER NOT NULL,
    idle_expires_at INTEGER NOT NULL,
    absolute_expires_at INTEGER NOT NULL,
    revoked_at INTEGER,
    step_up_expires_at INTEGER,
    remember_device INTEGER NOT NULL DEFAULT 0 CHECK (remember_device IN (0, 1)),
    source_ip TEXT NOT NULL,
    user_agent TEXT NOT NULL,
    device_label TEXT NOT NULL DEFAULT ''
);

CREATE INDEX sessions_user_id_idx ON sessions(user_id);
CREATE INDEX sessions_expiration_idx ON sessions(idle_expires_at, absolute_expires_at);

CREATE TABLE recovery_codes (
    id TEXT PRIMARY KEY NOT NULL,
    user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    code_hash TEXT NOT NULL,
    used_at INTEGER,
    created_at INTEGER NOT NULL
);

CREATE UNIQUE INDEX recovery_codes_user_hash_idx ON recovery_codes(user_id, code_hash);

CREATE TABLE login_throttle (
    account_key TEXT NOT NULL,
    source_ip TEXT NOT NULL,
    failure_count INTEGER NOT NULL DEFAULT 0,
    locked_until INTEGER NOT NULL DEFAULT 0,
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (account_key, source_ip)
);

CREATE TABLE login_history (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id TEXT REFERENCES users(id) ON DELETE SET NULL,
    attempted_username TEXT NOT NULL,
    source_ip TEXT NOT NULL,
    user_agent TEXT NOT NULL,
    succeeded INTEGER NOT NULL CHECK (succeeded IN (0, 1)),
    reason TEXT NOT NULL,
    created_at INTEGER NOT NULL
);

CREATE INDEX login_history_user_id_idx ON login_history(user_id, created_at DESC);

CREATE TABLE assigned_roots (
    id TEXT PRIMARY KEY NOT NULL,
    user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    path TEXT NOT NULL,
    access_mode TEXT NOT NULL CHECK (access_mode IN ('read', 'read-write')),
    created_at INTEGER NOT NULL,
    UNIQUE (user_id, path)
);

CREATE TABLE audit_events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    timestamp INTEGER NOT NULL,
    user_id TEXT,
    role_snapshot TEXT NOT NULL,
    session_id_hash TEXT,
    source_ip TEXT NOT NULL,
    user_agent TEXT NOT NULL,
    action TEXT NOT NULL,
    resource_type TEXT NOT NULL,
    resource_id TEXT,
    path TEXT,
    remote_target TEXT,
    result TEXT NOT NULL,
    metadata_json TEXT NOT NULL,
    previous_hash TEXT NOT NULL,
    event_hash TEXT NOT NULL UNIQUE
);

CREATE INDEX audit_events_timestamp_idx ON audit_events(timestamp DESC);
CREATE INDEX audit_events_user_id_idx ON audit_events(user_id, timestamp DESC);

CREATE TRIGGER audit_events_no_update
BEFORE UPDATE ON audit_events
BEGIN
    SELECT RAISE(ABORT, 'audit events are append-only');
END;

CREATE TRIGGER audit_events_no_delete
BEFORE DELETE ON audit_events
BEGIN
    SELECT RAISE(ABORT, 'audit events are append-only');
END;

CREATE TABLE system_settings (
    key TEXT PRIMARY KEY NOT NULL,
    value_json TEXT NOT NULL,
    updated_at INTEGER NOT NULL,
    updated_by TEXT REFERENCES users(id) ON DELETE SET NULL
);

INSERT INTO system_settings (key, value_json, updated_at)
VALUES
    ('ui.default_mode', '"desktop"', unixepoch()),
    ('runtime.browser.enabled', 'false', unixepoch()),
    ('runtime.code.enabled', 'false', unixepoch()),
    ('runtime.office.enabled', 'false', unixepoch());
