CREATE TABLE system_metadata (
    key TEXT PRIMARY KEY NOT NULL,
    value TEXT NOT NULL,
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);

INSERT INTO system_metadata (key, value)
VALUES ('schema_baseline', 'phase-0');

