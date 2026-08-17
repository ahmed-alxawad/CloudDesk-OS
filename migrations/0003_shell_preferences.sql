CREATE TABLE user_preferences (
    user_id TEXT PRIMARY KEY NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    ui_mode TEXT NOT NULL DEFAULT 'desktop' CHECK (ui_mode IN ('desktop', 'dashboard')),
    layout_json TEXT NOT NULL DEFAULT '{}',
    favorites_json TEXT NOT NULL DEFAULT '[]',
    recent_json TEXT NOT NULL DEFAULT '[]',
    updated_at INTEGER NOT NULL
);
