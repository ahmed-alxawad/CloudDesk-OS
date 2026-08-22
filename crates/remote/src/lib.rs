pub mod pty;
pub mod s3;
pub mod scp;
pub mod sftp;
pub mod ssh;
pub mod webdav;

use std::time::{SystemTime, UNIX_EPOCH};

use base64::{
    engine::general_purpose::{STANDARD, STANDARD_NO_PAD},
    Engine,
};
use rand::{rngs::OsRng, RngCore};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::{Row, SqlitePool};
use subtle::ConstantTimeEq;
use thiserror::Error;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SshAuthMethod {
    Password,
    PrivateKey,
    SshAgent,
    KeyboardInteractive,
    Certificate,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NewRemoteServer {
    pub name: String,
    pub hostname: String,
    pub port: u16,
    pub username: String,
    pub auth_method: SshAuthMethod,
    pub credential_secret_id: Option<String>,
    /// `SshAuthMethod::SshAgent` only: a real `ssh-agent` UNIX socket
    /// path. Never a secret itself (no key material lives here) --
    /// re-validated at every connection (see `worker.rs`) to belong to
    /// this server's own owning user's real Linux UID, never trusted
    /// merely because it was accepted at registration time.
    #[serde(default)]
    pub agent_socket_path: Option<String>,
    pub host_key_type: String,
    pub host_key_base64: String,
    pub proxy_jump_server_id: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
}

/// Task 37 (PASS SSH-A-2): the mutable subset of a `RemoteServer` --
/// switching auth methods (e.g. password -> SSH agent, or private key
/// -> certificate) never needs to re-scan the host key or rename the
/// server, so this intentionally carries only the auth-related fields
/// rather than reusing the full `NewRemoteServer` shape.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateRemoteServerAuth {
    pub auth_method: SshAuthMethod,
    pub credential_secret_id: Option<String>,
    #[serde(default)]
    pub agent_socket_path: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct RemoteServer {
    pub id: String,
    pub owner_user_id: String,
    pub name: String,
    pub hostname: String,
    pub port: u16,
    pub username: String,
    pub auth_method: SshAuthMethod,
    pub credential_secret_id: Option<String>,
    pub agent_socket_path: Option<String>,
    pub host_key_type: String,
    pub host_key_fingerprint: String,
    pub proxy_jump_server_id: Option<String>,
    pub tags: Vec<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PinnedHostKey {
    pub key_type: String,
    pub key_base64: String,
}

#[derive(Clone)]
pub struct RemoteServerStore {
    pool: SqlitePool,
}

impl RemoteServerStore {
    #[must_use]
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// Task 1 (Phase 2 closure): lets a caller resolving SSH agent
    /// auth re-check the agent socket's owning UID against the
    /// `RemoteServer` owner's own real Linux UID, without needing a
    /// separate `AuthService` handle threaded through the whole SSH
    /// connection-resolution call chain.
    #[must_use]
    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    pub async fn create(
        &self,
        owner_user_id: &str,
        input: &NewRemoteServer,
    ) -> Result<String, RemoteError> {
        validate_input(input)?;
        self.validate_references(owner_user_id, input).await?;
        let id = random_id();
        let timestamp = now();
        sqlx::query(
            "INSERT INTO remote_servers (
                id, owner_user_id, name, hostname, port, username, auth_method,
                credential_secret_id, agent_socket_path, host_key_type, host_key_base64,
                proxy_jump_server_id, tags_json, created_at, updated_at
             ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(owner_user_id)
        .bind(input.name.trim())
        .bind(input.hostname.trim())
        .bind(i64::from(input.port))
        .bind(input.username.trim())
        .bind(auth_method_name(input.auth_method))
        .bind(&input.credential_secret_id)
        .bind(&input.agent_socket_path)
        .bind(&input.host_key_type)
        .bind(&input.host_key_base64)
        .bind(&input.proxy_jump_server_id)
        .bind(serde_json::to_string(&normalized_tags(&input.tags))?)
        .bind(timestamp)
        .bind(timestamp)
        .execute(&self.pool)
        .await?;
        Ok(id)
    }

    /// Task 37 (PASS SSH-A-2): lets an owner switch a `RemoteServer`'s
    /// auth method (e.g. add SSH-agent or certificate support to a
    /// server originally registered with a password) without deleting
    /// and recreating it -- validated exactly as strictly as `create`
    /// (same `validate_auth_fields`, same Vault-secret ownership check),
    /// and scoped to `owner_user_id` exactly like every other method
    /// here, so User B can never edit User A's server (`NotFound`, not
    /// `Forbidden`, matching this store's existing cross-user
    /// indistinguishability discipline).
    pub async fn update_auth(
        &self,
        owner_user_id: &str,
        server_id: &str,
        input: &UpdateRemoteServerAuth,
    ) -> Result<(), RemoteError> {
        validate_auth_fields(
            input.auth_method,
            input.credential_secret_id.as_ref(),
            input.agent_socket_path.as_ref(),
        )?;
        if let Some(secret_id) = &input.credential_secret_id {
            let owned: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM vault_secrets WHERE id = ? AND owner_user_id = ?)",
            )
            .bind(secret_id)
            .bind(owner_user_id)
            .fetch_one(&self.pool)
            .await?;
            if !owned {
                return Err(RemoteError::InvalidCredentialReference);
            }
        }
        let updated = sqlx::query(
            "UPDATE remote_servers
             SET auth_method = ?, credential_secret_id = ?, agent_socket_path = ?, updated_at = ?
             WHERE id = ? AND owner_user_id = ?",
        )
        .bind(auth_method_name(input.auth_method))
        .bind(&input.credential_secret_id)
        .bind(&input.agent_socket_path)
        .bind(now())
        .bind(server_id)
        .bind(owner_user_id)
        .execute(&self.pool)
        .await?
        .rows_affected();
        if updated == 1 {
            Ok(())
        } else {
            Err(RemoteError::NotFound)
        }
    }

    pub async fn list(&self, owner_user_id: &str) -> Result<Vec<RemoteServer>, RemoteError> {
        let rows = sqlx::query(
            "SELECT * FROM remote_servers WHERE owner_user_id = ? ORDER BY name COLLATE NOCASE",
        )
        .bind(owner_user_id)
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(row_to_server).collect()
    }

    pub async fn get(
        &self,
        owner_user_id: &str,
        server_id: &str,
    ) -> Result<RemoteServer, RemoteError> {
        let row = sqlx::query("SELECT * FROM remote_servers WHERE id = ? AND owner_user_id = ?")
            .bind(server_id)
            .bind(owner_user_id)
            .fetch_optional(&self.pool)
            .await?
            .ok_or(RemoteError::NotFound)?;
        row_to_server(&row)
    }

    pub async fn delete(&self, owner_user_id: &str, server_id: &str) -> Result<(), RemoteError> {
        let deleted = sqlx::query("DELETE FROM remote_servers WHERE id = ? AND owner_user_id = ?")
            .bind(server_id)
            .bind(owner_user_id)
            .execute(&self.pool)
            .await?
            .rows_affected();
        if deleted == 1 {
            Ok(())
        } else {
            Err(RemoteError::NotFound)
        }
    }

    pub async fn pinned_host_key(
        &self,
        owner_user_id: &str,
        server_id: &str,
    ) -> Result<PinnedHostKey, RemoteError> {
        let row = sqlx::query(
            "SELECT host_key_type, host_key_base64 FROM remote_servers
             WHERE id = ? AND owner_user_id = ?",
        )
        .bind(server_id)
        .bind(owner_user_id)
        .fetch_optional(&self.pool)
        .await?
        .ok_or(RemoteError::NotFound)?;
        Ok(PinnedHostKey {
            key_type: row.get("host_key_type"),
            key_base64: row.get("host_key_base64"),
        })
    }

    async fn validate_references(
        &self,
        owner_user_id: &str,
        input: &NewRemoteServer,
    ) -> Result<(), RemoteError> {
        if let Some(secret_id) = &input.credential_secret_id {
            let owned: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM vault_secrets WHERE id = ? AND owner_user_id = ?)",
            )
            .bind(secret_id)
            .bind(owner_user_id)
            .fetch_one(&self.pool)
            .await?;
            if !owned {
                return Err(RemoteError::InvalidCredentialReference);
            }
        }
        if let Some(proxy_id) = &input.proxy_jump_server_id {
            let owned: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM remote_servers WHERE id = ? AND owner_user_id = ?)",
            )
            .bind(proxy_id)
            .bind(owner_user_id)
            .fetch_one(&self.pool)
            .await?;
            if !owned {
                return Err(RemoteError::InvalidProxyJump);
            }
        }
        Ok(())
    }
}

#[must_use]
pub fn known_hosts_line(server: &RemoteServer, host_key_base64: &str) -> String {
    let host = if server.port == 22 {
        server.hostname.clone()
    } else {
        format!("[{}]:{}", server.hostname, server.port)
    };
    format!("{host} {} {host_key_base64}\n", server.host_key_type)
}

pub fn verify_host_key(expected_base64: &str, presented_base64: &str) -> Result<(), RemoteError> {
    let expected = STANDARD
        .decode(expected_base64)
        .map_err(|_| RemoteError::InvalidHostKey)?;
    let presented = STANDARD
        .decode(presented_base64)
        .map_err(|_| RemoteError::InvalidHostKey)?;
    if expected.len() == presented.len()
        && bool::from(expected.as_slice().ct_eq(presented.as_slice()))
    {
        Ok(())
    } else {
        Err(RemoteError::HostKeyChanged)
    }
}

pub fn validate_hostname(hostname: &str) -> Result<(), RemoteError> {
    let hostname = hostname.trim();
    if hostname.is_empty()
        || hostname.starts_with('-')
        || hostname.len() > 253
        || hostname
            .bytes()
            .any(|byte| byte.is_ascii_whitespace() || matches!(byte, b'/' | b'\\' | b'\0'))
    {
        Err(RemoteError::InvalidHostname)
    } else {
        Ok(())
    }
}

pub fn host_key_fingerprint(key_base64: &str) -> Result<String, RemoteError> {
    let key = STANDARD
        .decode(key_base64)
        .map_err(|_| RemoteError::InvalidHostKey)?;
    if key.len() < 32 {
        return Err(RemoteError::InvalidHostKey);
    }
    Ok(format!(
        "SHA256:{}",
        STANDARD_NO_PAD.encode(Sha256::digest(key))
    ))
}

fn validate_input(input: &NewRemoteServer) -> Result<(), RemoteError> {
    if input.name.trim().is_empty() || input.name.len() > 128 {
        return Err(RemoteError::InvalidName);
    }
    validate_hostname(&input.hostname)?;
    if input.username.trim().is_empty()
        || input.username.len() > 128
        || input.username.bytes().any(|byte| byte.is_ascii_control())
    {
        return Err(RemoteError::InvalidUsername);
    }
    if !matches!(
        input.host_key_type.as_str(),
        "ssh-ed25519"
            | "ssh-rsa"
            | "ecdsa-sha2-nistp256"
            | "ecdsa-sha2-nistp384"
            | "ecdsa-sha2-nistp521"
    ) {
        return Err(RemoteError::InvalidHostKey);
    }
    let key = STANDARD
        .decode(&input.host_key_base64)
        .map_err(|_| RemoteError::InvalidHostKey)?;
    if key.len() < 32 {
        return Err(RemoteError::InvalidHostKey);
    }
    if input.tags.len() > 32
        || input
            .tags
            .iter()
            .any(|tag| tag.trim().is_empty() || tag.len() > 64)
    {
        return Err(RemoteError::InvalidTags);
    }
    validate_auth_fields(
        input.auth_method,
        input.credential_secret_id.as_ref(),
        input.agent_socket_path.as_ref(),
    )
}

/// Shared by `validate_input` (create) and `RemoteServerStore::update_auth`
/// (Task 37, PASS SSH-A-2): every auth method carries exactly one of
/// "a Vault secret" or "an agent socket", never both, never neither --
/// factored out so switching auth methods on an existing `RemoteServer`
/// is validated exactly as strictly as creating one.
fn validate_auth_fields(
    auth_method: SshAuthMethod,
    credential_secret_id: Option<&String>,
    agent_socket_path: Option<&String>,
) -> Result<(), RemoteError> {
    let secret_required = !matches!(auth_method, SshAuthMethod::SshAgent);
    if secret_required != credential_secret_id.is_some() {
        return Err(RemoteError::InvalidCredentialReference);
    }
    let agent_required = matches!(auth_method, SshAuthMethod::SshAgent);
    if agent_required != agent_socket_path.is_some() {
        return Err(RemoteError::InvalidAgentSocketPath);
    }
    if let Some(path) = agent_socket_path {
        validate_agent_socket_path(path)?;
    }
    Ok(())
}

/// Task 2 (Phase 2 closure): format validation only -- an absolute
/// path, no traversal segments, no embedded NUL/control bytes,
/// bounded length. The real security boundary is enforced at
/// *connection* time, not here: `worker.rs::resolve_auth` re-checks
/// that the socket file is actually owned by this server's owning
/// `CloudDesk` user's real Linux UID before ever connecting to it, so
/// a misconfigured or since-repurposed path can never be used to
/// reach another user's agent even if it passed this check when the
/// `RemoteServer` was first registered.
fn validate_agent_socket_path(path: &str) -> Result<(), RemoteError> {
    let trimmed = path.trim();
    if trimmed.is_empty()
        || trimmed.len() > 255
        || !trimmed.starts_with('/')
        || trimmed.split('/').any(|segment| segment == "..")
        || trimmed.bytes().any(|byte| byte.is_ascii_control())
    {
        return Err(RemoteError::InvalidAgentSocketPath);
    }
    Ok(())
}

fn normalized_tags(tags: &[String]) -> Vec<String> {
    let mut tags = tags
        .iter()
        .map(|tag| tag.trim().to_owned())
        .collect::<Vec<_>>();
    tags.sort();
    tags.dedup();
    tags
}

fn row_to_server(row: &sqlx::sqlite::SqliteRow) -> Result<RemoteServer, RemoteError> {
    let port = u16::try_from(row.get::<i64, _>("port")).map_err(|_| RemoteError::Corrupt)?;
    let key: String = row.get("host_key_base64");
    Ok(RemoteServer {
        id: row.get("id"),
        owner_user_id: row.get("owner_user_id"),
        name: row.get("name"),
        hostname: row.get("hostname"),
        port,
        username: row.get("username"),
        auth_method: parse_auth_method(&row.get::<String, _>("auth_method"))?,
        credential_secret_id: row.get("credential_secret_id"),
        agent_socket_path: row.get("agent_socket_path"),
        host_key_type: row.get("host_key_type"),
        host_key_fingerprint: host_key_fingerprint(&key).map_err(|_| RemoteError::Corrupt)?,
        proxy_jump_server_id: row.get("proxy_jump_server_id"),
        tags: serde_json::from_str(&row.get::<String, _>("tags_json"))?,
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    })
}

fn auth_method_name(method: SshAuthMethod) -> &'static str {
    match method {
        SshAuthMethod::Password => "password",
        SshAuthMethod::PrivateKey => "private_key",
        SshAuthMethod::SshAgent => "ssh_agent",
        SshAuthMethod::KeyboardInteractive => "keyboard_interactive",
        SshAuthMethod::Certificate => "certificate",
    }
}

fn parse_auth_method(value: &str) -> Result<SshAuthMethod, RemoteError> {
    match value {
        "password" => Ok(SshAuthMethod::Password),
        "private_key" => Ok(SshAuthMethod::PrivateKey),
        "ssh_agent" => Ok(SshAuthMethod::SshAgent),
        "keyboard_interactive" => Ok(SshAuthMethod::KeyboardInteractive),
        "certificate" => Ok(SshAuthMethod::Certificate),
        _ => Err(RemoteError::Corrupt),
    }
}

fn random_id() -> String {
    let mut bytes = [0_u8; 16];
    OsRng.fill_bytes(&mut bytes);
    hex::encode(bytes)
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| {
            i64::try_from(duration.as_secs()).unwrap_or(i64::MAX)
        })
}

#[derive(Debug, Error)]
pub enum RemoteError {
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("remote server was not found")]
    NotFound,
    #[error("remote server name is invalid")]
    InvalidName,
    #[error("remote hostname is invalid")]
    InvalidHostname,
    #[error("remote username is invalid")]
    InvalidUsername,
    #[error("SSH host key is invalid")]
    InvalidHostKey,
    #[error("SSH host key changed")]
    HostKeyChanged,
    #[error("credential reference is invalid")]
    InvalidCredentialReference,
    #[error("SSH agent socket path is invalid")]
    InvalidAgentSocketPath,
    #[error("ProxyJump reference is invalid")]
    InvalidProxyJump,
    #[error("tags are invalid")]
    InvalidTags,
    #[error("stored remote server is corrupt")]
    Corrupt,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(key: &str) -> NewRemoteServer {
        NewRemoteServer {
            name: "Production".into(),
            hostname: "server.example".into(),
            port: 22,
            username: "deploy".into(),
            auth_method: SshAuthMethod::SshAgent,
            credential_secret_id: None,
            agent_socket_path: Some("/tmp/test-agent.sock".into()),
            host_key_type: "ssh-ed25519".into(),
            host_key_base64: key.into(),
            proxy_jump_server_id: None,
            tags: vec!["production".into()],
        }
    }

    #[tokio::test]
    async fn saved_servers_are_owner_scoped_and_host_key_changes_fail_closed() {
        let pool = clouddesk_db::connect("sqlite::memory:", 1).await.unwrap();
        clouddesk_db::migrate(&pool).await.unwrap();
        for (id, username) in [("u1", "one"), ("u2", "two")] {
            sqlx::query("INSERT INTO users (id, username, display_name, password_hash, created_at, updated_at) VALUES (?, ?, 'User', 'hash', 1, 1)").bind(id).bind(username).execute(&pool).await.unwrap();
        }
        let store = RemoteServerStore::new(pool);
        let key = STANDARD.encode([7_u8; 32]);
        let id = store.create("u1", &input(&key)).await.unwrap();
        assert_eq!(
            store.get("u1", &id).await.unwrap().host_key_fingerprint,
            format!(
                "SHA256:{}",
                STANDARD_NO_PAD.encode(Sha256::digest([7_u8; 32]))
            )
        );
        assert!(matches!(
            store.get("u2", &id).await,
            Err(RemoteError::NotFound)
        ));
        verify_host_key(&key, &key).unwrap();
        assert!(matches!(
            verify_host_key(&key, &STANDARD.encode([8_u8; 32])),
            Err(RemoteError::HostKeyChanged)
        ));
    }
}
