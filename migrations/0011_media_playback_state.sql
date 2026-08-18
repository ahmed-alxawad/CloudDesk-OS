-- Per-user playback resume position, keyed by (owner, virtual path) --
-- not filename alone, so two users' or two folders' identically-named
-- files never collide. Renaming a file resets its resume position; that
-- is an accepted, documented limitation (content-hash identity would
-- require hashing the whole file on every playback start).
CREATE TABLE media_playback_state (
    owner_user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    virtual_path TEXT NOT NULL,
    position_seconds REAL NOT NULL,
    duration_seconds REAL,
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (owner_user_id, virtual_path)
);
