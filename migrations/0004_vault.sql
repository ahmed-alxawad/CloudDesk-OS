CREATE TABLE vault_secrets (
    id TEXT PRIMARY KEY NOT NULL,
    owner_user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    kind TEXT NOT NULL,
    label TEXT NOT NULL,
    encrypted_value TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    last_revealed_at INTEGER
);

CREATE INDEX vault_secrets_owner_idx ON vault_secrets(owner_user_id, kind, label);
