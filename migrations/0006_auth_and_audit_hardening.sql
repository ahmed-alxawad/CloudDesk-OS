CREATE TABLE login_throttle_buckets (
    dimension TEXT NOT NULL CHECK (dimension IN ('account', 'ip')),
    bucket_key TEXT NOT NULL,
    failure_count INTEGER NOT NULL DEFAULT 0,
    locked_until INTEGER NOT NULL DEFAULT 0,
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (dimension, bucket_key)
);

CREATE INDEX login_throttle_buckets_expiry_idx
ON login_throttle_buckets(locked_until, updated_at);

CREATE TABLE audit_chain_head (
    singleton INTEGER PRIMARY KEY NOT NULL CHECK (singleton = 1),
    last_hash TEXT NOT NULL
);

INSERT INTO audit_chain_head (singleton, last_hash)
VALUES (
    1,
    COALESCE((SELECT event_hash FROM audit_events ORDER BY id DESC LIMIT 1), '')
);
