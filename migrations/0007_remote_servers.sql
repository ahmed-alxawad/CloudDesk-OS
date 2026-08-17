CREATE TABLE remote_servers (
    id TEXT PRIMARY KEY NOT NULL,
    owner_user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    hostname TEXT NOT NULL,
    port INTEGER NOT NULL CHECK (port BETWEEN 1 AND 65535),
    username TEXT NOT NULL,
    auth_method TEXT NOT NULL CHECK (auth_method IN (
        'password', 'private_key', 'ssh_agent', 'keyboard_interactive', 'certificate'
    )),
    credential_secret_id TEXT REFERENCES vault_secrets(id) ON DELETE RESTRICT,
    host_key_type TEXT NOT NULL,
    host_key_base64 TEXT NOT NULL,
    proxy_jump_server_id TEXT REFERENCES remote_servers(id) ON DELETE SET NULL,
    tags_json TEXT NOT NULL DEFAULT '[]',
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    UNIQUE (owner_user_id, name)
);

CREATE INDEX remote_servers_owner_idx ON remote_servers(owner_user_id, name);
CREATE INDEX remote_servers_target_idx ON remote_servers(owner_user_id, hostname, port);
