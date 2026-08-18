CREATE TABLE music_library_roots (
    id TEXT PRIMARY KEY,
    owner_user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    virtual_path TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    UNIQUE (owner_user_id, virtual_path)
);

-- Track identity is (owner_user_id, virtual_path) -- the same convention
-- used throughout this project (media_playback_state, upload_sessions).
-- A rename/move is therefore indistinguishable from delete-then-add: the
-- old row disappears on the next scan (its file no longer exists at that
-- path) and a new row is created for the new path. This is a deliberate,
-- documented policy choice, not an oversight -- true content-hash-based
-- identity would require hashing every file's full contents on every
-- scan, which was judged not worth the cost for this phase. Playlist
-- entries/favorites/recent-history rows referencing a track ON DELETE
-- CASCADE with it: renaming a track drops it from playlists/favorites
-- rather than leaving a dangling or silently-wrong reference.
CREATE TABLE music_tracks (
    id TEXT PRIMARY KEY,
    owner_user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    root_id TEXT NOT NULL REFERENCES music_library_roots(id) ON DELETE CASCADE,
    virtual_path TEXT NOT NULL,
    title TEXT,
    artist TEXT,
    album TEXT,
    album_artist TEXT,
    track_number INTEGER,
    disc_number INTEGER,
    duration_seconds REAL,
    codec TEXT,
    bit_rate INTEGER,
    year TEXT,
    genre TEXT,
    -- "<size>:<mtime_unix>" -- cheap incremental-scan fingerprint. A file
    -- whose fingerprint hasn't changed is skipped without re-probing it.
    fingerprint TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    UNIQUE (owner_user_id, virtual_path)
);
CREATE INDEX idx_music_tracks_owner ON music_tracks(owner_user_id);
CREATE INDEX idx_music_tracks_artist ON music_tracks(owner_user_id, artist);
CREATE INDEX idx_music_tracks_album ON music_tracks(owner_user_id, album);
CREATE INDEX idx_music_tracks_title ON music_tracks(owner_user_id, title);
CREATE INDEX idx_music_tracks_root ON music_tracks(root_id);

CREATE TABLE music_playlists (
    id TEXT PRIMARY KEY,
    owner_user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
CREATE INDEX idx_music_playlists_owner ON music_playlists(owner_user_id);

CREATE TABLE music_playlist_entries (
    id TEXT PRIMARY KEY,
    playlist_id TEXT NOT NULL REFERENCES music_playlists(id) ON DELETE CASCADE,
    track_id TEXT NOT NULL REFERENCES music_tracks(id) ON DELETE CASCADE,
    position INTEGER NOT NULL
);
CREATE INDEX idx_music_playlist_entries_playlist ON music_playlist_entries(playlist_id, position);

CREATE TABLE music_favorites (
    owner_user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    track_id TEXT NOT NULL REFERENCES music_tracks(id) ON DELETE CASCADE,
    created_at INTEGER NOT NULL,
    PRIMARY KEY (owner_user_id, track_id)
);

-- One row per (owner, track, play). "Actual playback" is enforced by the
-- caller (the frontend only records a play after a minimum elapsed-time
-- threshold -- see MusicApp.svelte), not by this table itself, which
-- just stores whatever the caller reports.
CREATE TABLE music_recently_played (
    id TEXT PRIMARY KEY,
    owner_user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    track_id TEXT NOT NULL REFERENCES music_tracks(id) ON DELETE CASCADE,
    played_at INTEGER NOT NULL
);
CREATE INDEX idx_music_recent_owner ON music_recently_played(owner_user_id, played_at);

-- One queue per user (not per session/window) -- a single logged-in
-- user has one Music playback queue, matching the single-instance-per-
-- app-id desktop shell. items_json is a JSON array of track IDs.
CREATE TABLE music_queue (
    owner_user_id TEXT PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    items_json TEXT NOT NULL,
    updated_at INTEGER NOT NULL
);
