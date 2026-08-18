use std::{
    collections::HashSet,
    time::{SystemTime, UNIX_EPOCH},
};

use argon2::{
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use clouddesk_audit::{append as append_audit, append_in_transaction, NewAuditEvent};
use clouddesk_permissions::{is_known_capability, CAPABILITIES};
use clouddesk_secrets::SecretCipher;
use data_encoding::BASE32_NOPAD;
use hmac::{Hmac, Mac};
use rand::{rngs::OsRng, RngCore};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha1::Sha1;
use sha2::{Digest, Sha256};
use sqlx::{Row, Sqlite, SqlitePool, Transaction};
use subtle::ConstantTimeEq;
use thiserror::Error;

const TOTP_PERIOD_SECONDS: i64 = 30;
const TOTP_DIGITS: u32 = 6;
const RECOVERY_CODE_COUNT: usize = 10;

#[derive(Clone, Debug)]
pub struct AuthPolicy {
    pub idle_timeout_seconds: i64,
    pub absolute_timeout_seconds: i64,
    pub remember_timeout_seconds: i64,
    pub step_up_seconds: i64,
    pub maximum_failures: i64,
}

impl Default for AuthPolicy {
    fn default() -> Self {
        Self {
            idle_timeout_seconds: 30 * 60,
            absolute_timeout_seconds: 12 * 60 * 60,
            remember_timeout_seconds: 30 * 24 * 60 * 60,
            step_up_seconds: 5 * 60,
            maximum_failures: 5,
        }
    }
}

#[derive(Clone)]
pub struct AuthService {
    pool: SqlitePool,
    cipher: SecretCipher,
    policy: AuthPolicy,
    dummy_password_hash: String,
}

impl AuthService {
    pub fn new(
        pool: SqlitePool,
        cipher: SecretCipher,
        policy: AuthPolicy,
    ) -> Result<Self, AuthError> {
        Ok(Self {
            pool,
            cipher,
            policy,
            dummy_password_hash: hash_password("invalid account timing sentinel")?,
        })
    }

    #[must_use]
    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    #[must_use]
    pub fn secret_cipher(&self) -> SecretCipher {
        self.cipher.clone()
    }

    pub async fn seed_authorization_model(&self) -> Result<(), AuthError> {
        let mut transaction = self.pool.begin().await?;
        seed_authorization_model(&mut transaction).await?;
        transaction.commit().await?;
        Ok(())
    }

    pub async fn has_users(&self) -> Result<bool, AuthError> {
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users")
            .fetch_one(&self.pool)
            .await?;
        Ok(count > 0)
    }

    pub async fn bootstrap_administrator(
        &self,
        username: &str,
        display_name: &str,
        password: &str,
        source_ip: &str,
        user_agent: &str,
    ) -> Result<String, AuthError> {
        self.bootstrap_administrator_configured(
            username,
            display_name,
            password,
            BootstrapConfiguration::default(),
            source_ip,
            user_agent,
        )
        .await
    }

    #[allow(clippy::too_many_arguments, clippy::similar_names)]
    pub async fn bootstrap_administrator_configured(
        &self,
        username: &str,
        display_name: &str,
        password: &str,
        configuration: BootstrapConfiguration<'_>,
        source_ip: &str,
        user_agent: &str,
    ) -> Result<String, AuthError> {
        validate_username(username)?;
        validate_password(password)?;
        if !matches!(configuration.ui_mode, "desktop" | "dashboard") {
            return Err(AuthError::InvalidUiMode);
        }
        if configuration
            .linux_identity
            .is_some_and(|(uid, gid)| uid == 0 || gid == 0)
        {
            return Err(AuthError::InvalidLinuxIdentity);
        }

        let user_id = random_identifier(16);
        let timestamp = now();
        let password_hash = hash_password(password)?;
        let mut transaction = self.pool.begin().await?;
        seed_authorization_model(&mut transaction).await?;
        let users: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users")
            .fetch_one(&mut *transaction)
            .await?;
        if users > 0 {
            return Err(AuthError::BootstrapComplete);
        }
        let (linux_uid, linux_gid) = configuration
            .linux_identity
            .map_or((None, None), |(uid, gid)| {
                (Some(i64::from(uid)), Some(i64::from(gid)))
            });
        sqlx::query(
            "INSERT INTO users (
                id, username, display_name, password_hash, linux_uid, linux_gid,
                created_at, updated_at
             ) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&user_id)
        .bind(normalize_username(username))
        .bind(display_name.trim())
        .bind(password_hash)
        .bind(linux_uid)
        .bind(linux_gid)
        .bind(timestamp)
        .bind(timestamp)
        .execute(&mut *transaction)
        .await?;
        sqlx::query("INSERT INTO user_roles (user_id, role_id) VALUES (?, 'administrator')")
            .bind(&user_id)
            .execute(&mut *transaction)
            .await?;
        for (key, value) in [
            ("ui.default_mode", json!(configuration.ui_mode)),
            (
                "runtime.browser.enabled",
                json!(configuration.enable_browser),
            ),
            ("runtime.code.enabled", json!(configuration.enable_code)),
            ("runtime.office.enabled", json!(configuration.enable_office)),
        ] {
            sqlx::query(
                "UPDATE system_settings SET value_json = ?, updated_at = ?, updated_by = ?
                 WHERE key = ?",
            )
            .bind(value.to_string())
            .bind(timestamp)
            .bind(&user_id)
            .bind(key)
            .execute(&mut *transaction)
            .await?;
        }
        sqlx::query("INSERT INTO user_preferences (user_id, ui_mode, updated_at) VALUES (?, ?, ?)")
            .bind(&user_id)
            .bind(configuration.ui_mode)
            .bind(timestamp)
            .execute(&mut *transaction)
            .await?;
        append_in_transaction(
            &mut transaction,
            &audit_event(
                timestamp,
                Some(user_id.clone()),
                "bootstrap.administrator.create",
                "user",
                Some(user_id.clone()),
                "success",
                source_ip,
                user_agent,
            ),
        )
        .await?;
        transaction.commit().await?;
        Ok(user_id)
    }
}

async fn seed_authorization_model(
    transaction: &mut Transaction<'_, Sqlite>,
) -> Result<(), AuthError> {
    for capability in CAPABILITIES {
        sqlx::query("INSERT OR IGNORE INTO permissions (name) VALUES (?)")
            .bind(capability.as_str())
            .execute(&mut **transaction)
            .await?;
    }

    let roles = [
        (
            "administrator",
            "Administrator",
            "Full CloudDesk administration",
        ),
        (
            "manager",
            "Manager",
            "Workspace and user workflow management",
        ),
        ("user", "User", "Standard personal workspace access"),
        ("guest", "Guest", "Restricted read-only access"),
    ];
    for (id, name, description) in roles {
        sqlx::query(
            "INSERT OR IGNORE INTO roles (id, name, description, created_at) VALUES (?, ?, ?, ?)",
        )
        .bind(id)
        .bind(name)
        .bind(description)
        .bind(now())
        .execute(&mut **transaction)
        .await?;
    }

    for capability in CAPABILITIES {
        sqlx::query(
                "INSERT OR IGNORE INTO role_permissions (role_id, permission_name) VALUES ('administrator', ?)",
            )
            .bind(capability.as_str())
            .execute(&mut **transaction)
            .await?;
    }

    for capability in [
        "files.local.read",
        "files.local.write",
        "remote.servers.read",
        "remote.servers.manage",
        "remote.terminal.open",
        "transfers.create",
        "transfers.cancel",
        "terminal.local.open",
        "apps.browser.use",
        "apps.code.use",
        "apps.office.use",
        "apps.media.use",
    ] {
        grant_role(transaction, "manager", capability).await?;
        grant_role(transaction, "user", capability).await?;
    }
    grant_role(transaction, "guest", "files.local.read").await?;
    Ok(())
}

impl AuthService {
    #[allow(clippy::too_many_lines)]
    pub async fn login(&self, request: LoginRequest<'_>) -> Result<LoginSuccess, AuthError> {
        let timestamp = now();
        let normalized = normalize_username(request.username);
        let account_key = hash_value(normalized.as_bytes());
        self.enforce_rate_limit(&account_key, request.source_ip, timestamp)
            .await?;

        let row = sqlx::query(
            "SELECT id, username, password_hash, totp_secret, totp_enabled, disabled
             FROM users WHERE username = ?",
        )
        .bind(&normalized)
        .fetch_optional(&self.pool)
        .await?;

        let (user_id, username, password_hash, totp_secret, totp_enabled, disabled) =
            row.as_ref().map_or_else(
                || {
                    (
                        None,
                        normalized.clone(),
                        self.dummy_password_hash.clone(),
                        None,
                        false,
                        true,
                    )
                },
                |row| {
                    (
                        Some(row.get::<String, _>("id")),
                        row.get::<String, _>("username"),
                        row.get::<String, _>("password_hash"),
                        row.get::<Option<String>, _>("totp_secret"),
                        row.get::<bool, _>("totp_enabled"),
                        row.get::<bool, _>("disabled"),
                    )
                },
            );

        let password_valid = verify_password(request.password, password_hash.as_ref());
        let mut second_factor_valid = !totp_enabled;
        if password_valid && totp_enabled {
            if let (Some(user_id), Some(encrypted), Some(code)) = (
                user_id.as_ref(),
                totp_secret.as_ref(),
                request.second_factor,
            ) {
                second_factor_valid = self
                    .verify_second_factor(user_id, encrypted, code, timestamp)
                    .await?;
            }
        }

        if !password_valid || !second_factor_valid || disabled || user_id.is_none() {
            self.record_login_failure(
                &account_key,
                user_id.as_deref(),
                request,
                if password_valid && totp_enabled {
                    "invalid_second_factor"
                } else {
                    "invalid_credentials"
                },
                timestamp,
            )
            .await?;
            return Err(AuthError::InvalidCredentials);
        }

        let user_id = user_id.expect("validated user id");
        let ip_key = hash_value(request.source_ip.as_bytes());
        sqlx::query(
            "DELETE FROM login_throttle_buckets
             WHERE (dimension = 'account' AND bucket_key = ?)
                OR (dimension = 'ip' AND bucket_key = ?)",
        )
        .bind(&account_key)
        .bind(ip_key)
        .execute(&self.pool)
        .await?;
        self.record_login_history(Some(&user_id), request, true, "success", timestamp)
            .await?;

        let token = random_identifier(32);
        let session_id_hash = hash_value(token.as_bytes());
        let absolute_timeout = if request.remember_device {
            self.policy.remember_timeout_seconds
        } else {
            self.policy.absolute_timeout_seconds
        };
        sqlx::query(
            "INSERT INTO sessions (
                id_hash, user_id, created_at, last_activity, idle_expires_at,
                absolute_expires_at, remember_device, source_ip, user_agent, device_label
             ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&session_id_hash)
        .bind(&user_id)
        .bind(timestamp)
        .bind(timestamp)
        .bind(timestamp + self.policy.idle_timeout_seconds)
        .bind(timestamp + absolute_timeout)
        .bind(request.remember_device)
        .bind(request.source_ip)
        .bind(request.user_agent)
        .bind(request.device_label.unwrap_or(""))
        .execute(&self.pool)
        .await?;

        append_audit(
            &self.pool,
            &audit_event(
                timestamp,
                Some(user_id.clone()),
                "auth.login",
                "session",
                Some(session_id_hash.clone()),
                "success",
                request.source_ip,
                request.user_agent,
            ),
        )
        .await?;

        Ok(LoginSuccess {
            token,
            session_id_hash,
            user_id,
            username,
            absolute_expires_at: timestamp + absolute_timeout,
        })
    }

    pub async fn authenticate(&self, token: &str) -> Result<SessionPrincipal, AuthError> {
        let timestamp = now();
        let session_id_hash = hash_value(token.as_bytes());
        let row = sqlx::query(
            "SELECT s.user_id, u.username, s.idle_expires_at, s.absolute_expires_at,
                    s.step_up_expires_at
             FROM sessions s JOIN users u ON u.id = s.user_id
             WHERE s.id_hash = ? AND s.revoked_at IS NULL AND u.disabled = 0",
        )
        .bind(&session_id_hash)
        .fetch_optional(&self.pool)
        .await?
        .ok_or(AuthError::InvalidSession)?;

        let idle_expires_at: i64 = row.get("idle_expires_at");
        let absolute_expires_at: i64 = row.get("absolute_expires_at");
        if idle_expires_at <= timestamp || absolute_expires_at <= timestamp {
            return Err(AuthError::InvalidSession);
        }

        let user_id: String = row.get("user_id");
        let roles = self.roles_for_user(&user_id).await?;
        let capabilities = self.capabilities_for_user(&user_id).await?;
        let refreshed_idle =
            (timestamp + self.policy.idle_timeout_seconds).min(absolute_expires_at);
        sqlx::query("UPDATE sessions SET last_activity = ?, idle_expires_at = ? WHERE id_hash = ?")
            .bind(timestamp)
            .bind(refreshed_idle)
            .bind(&session_id_hash)
            .execute(&self.pool)
            .await?;

        Ok(SessionPrincipal {
            user_id,
            username: row.get("username"),
            session_id_hash,
            roles,
            capabilities,
            step_up_expires_at: row.get("step_up_expires_at"),
        })
    }

    pub async fn revoke(
        &self,
        token: &str,
        source_ip: &str,
        user_agent: &str,
    ) -> Result<(), AuthError> {
        let timestamp = now();
        let session_id_hash = hash_value(token.as_bytes());
        let principal = self.authenticate(token).await?;
        sqlx::query("UPDATE sessions SET revoked_at = ? WHERE id_hash = ?")
            .bind(timestamp)
            .bind(&session_id_hash)
            .execute(&self.pool)
            .await?;
        append_audit(
            &self.pool,
            &audit_event(
                timestamp,
                Some(principal.user_id),
                "auth.logout",
                "session",
                Some(session_id_hash),
                "success",
                source_ip,
                user_agent,
            ),
        )
        .await?;
        Ok(())
    }

    pub async fn begin_totp(&self, principal: &SessionPrincipal) -> Result<String, AuthError> {
        let mut secret = [0_u8; 20];
        OsRng.fill_bytes(&mut secret);
        let encrypted = self
            .cipher
            .encrypt(&secret, totp_context(&principal.user_id).as_bytes())?;
        sqlx::query(
            "UPDATE users SET totp_secret = ?, totp_enabled = 0, updated_at = ? WHERE id = ?",
        )
        .bind(encrypted)
        .bind(now())
        .bind(&principal.user_id)
        .execute(&self.pool)
        .await?;
        self.append_actor_audit(
            principal,
            "auth.totp.setup.begin",
            "user",
            Some(principal.user_id.clone()),
            "success",
            "internal",
            "auth-service",
        )
        .await?;
        Ok(BASE32_NOPAD.encode(&secret))
    }

    pub async fn confirm_totp(
        &self,
        principal: &SessionPrincipal,
        code: &str,
    ) -> Result<Vec<String>, AuthError> {
        self.confirm_totp_at(principal, code, now()).await
    }

    async fn confirm_totp_at(
        &self,
        principal: &SessionPrincipal,
        code: &str,
        timestamp: i64,
    ) -> Result<Vec<String>, AuthError> {
        let encrypted: String = sqlx::query_scalar("SELECT totp_secret FROM users WHERE id = ?")
            .bind(&principal.user_id)
            .fetch_optional(&self.pool)
            .await?
            .ok_or(AuthError::TotpNotConfigured)?;
        let secret = self
            .cipher
            .decrypt(&encrypted, totp_context(&principal.user_id).as_bytes())?;
        if !verify_totp(&secret, code, timestamp) {
            return Err(AuthError::InvalidSecondFactor);
        }

        let codes: Vec<String> = (0..RECOVERY_CODE_COUNT)
            .map(|_| grouped_recovery_code())
            .collect();
        let mut transaction = self.pool.begin().await?;
        sqlx::query("UPDATE users SET totp_enabled = 1, updated_at = ? WHERE id = ?")
            .bind(timestamp)
            .bind(&principal.user_id)
            .execute(&mut *transaction)
            .await?;
        sqlx::query("DELETE FROM recovery_codes WHERE user_id = ?")
            .bind(&principal.user_id)
            .execute(&mut *transaction)
            .await?;
        for code in &codes {
            sqlx::query(
                "INSERT INTO recovery_codes (id, user_id, code_hash, created_at) VALUES (?, ?, ?, ?)",
            )
            .bind(random_identifier(16))
            .bind(&principal.user_id)
            .bind(hash_value(normalize_recovery_code(code).as_bytes()))
            .bind(timestamp)
            .execute(&mut *transaction)
            .await?;
        }
        transaction.commit().await?;
        self.append_actor_audit(
            principal,
            "auth.totp.enable",
            "user",
            Some(principal.user_id.clone()),
            "success",
            "internal",
            "auth-service",
        )
        .await?;
        Ok(codes)
    }

    pub async fn step_up(
        &self,
        token: &str,
        password: &str,
        second_factor: Option<&str>,
        source_ip: &str,
        user_agent: &str,
    ) -> Result<i64, AuthError> {
        let principal = self.authenticate(token).await?;
        let row =
            sqlx::query("SELECT password_hash, totp_secret, totp_enabled FROM users WHERE id = ?")
                .bind(&principal.user_id)
                .fetch_one(&self.pool)
                .await?;
        let password_hash: String = row.get("password_hash");
        if !verify_password(password, &password_hash) {
            return Err(AuthError::InvalidCredentials);
        }
        if row.get::<bool, _>("totp_enabled") {
            let encrypted: String = row.get("totp_secret");
            let Some(code) = second_factor else {
                return Err(AuthError::InvalidSecondFactor);
            };
            if !self
                .verify_second_factor(&principal.user_id, &encrypted, code, now())
                .await?
            {
                return Err(AuthError::InvalidSecondFactor);
            }
        }

        let expires_at = now() + self.policy.step_up_seconds;
        sqlx::query("UPDATE sessions SET step_up_expires_at = ? WHERE id_hash = ?")
            .bind(expires_at)
            .bind(&principal.session_id_hash)
            .execute(&self.pool)
            .await?;
        append_audit(
            &self.pool,
            &audit_event(
                now(),
                Some(principal.user_id),
                "auth.step_up",
                "session",
                Some(principal.session_id_hash),
                "success",
                source_ip,
                user_agent,
            ),
        )
        .await?;
        Ok(expires_at)
    }

    pub async fn create_user(
        &self,
        actor: &SessionPrincipal,
        request: CreateUserRequest<'_>,
        source_ip: &str,
        user_agent: &str,
    ) -> Result<String, AuthError> {
        require_actor(actor, "users.manage", true)?;
        validate_username(request.username)?;
        validate_password(request.password)?;
        if request.role_ids.is_empty() {
            return Err(AuthError::UnknownRole);
        }

        let user_id = random_identifier(16);
        let timestamp = now();
        let password_hash = hash_password(request.password)?;
        let mut transaction = self.pool.begin().await?;
        sqlx::query(
            "INSERT INTO users (
                id, username, display_name, password_hash, created_at, updated_at
             ) VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(&user_id)
        .bind(normalize_username(request.username))
        .bind(request.display_name.trim())
        .bind(password_hash)
        .bind(timestamp)
        .bind(timestamp)
        .execute(&mut *transaction)
        .await?;
        for role_id in request.role_ids {
            let inserted = sqlx::query("INSERT INTO user_roles (user_id, role_id) VALUES (?, ?)")
                .bind(&user_id)
                .bind(role_id)
                .execute(&mut *transaction)
                .await;
            if inserted.is_err() {
                return Err(AuthError::UnknownRole);
            }
        }
        transaction.commit().await?;
        self.append_actor_audit(
            actor,
            "users.create",
            "user",
            Some(user_id.clone()),
            "success",
            source_ip,
            user_agent,
        )
        .await?;
        Ok(user_id)
    }

    pub async fn assign_role(
        &self,
        actor: &SessionPrincipal,
        user_id: &str,
        role_id: &str,
        source_ip: &str,
        user_agent: &str,
    ) -> Result<(), AuthError> {
        require_actor(actor, "roles.manage", true)?;
        let result =
            sqlx::query("INSERT OR IGNORE INTO user_roles (user_id, role_id) VALUES (?, ?)")
                .bind(user_id)
                .bind(role_id)
                .execute(&self.pool)
                .await;
        if result.is_err() {
            return Err(AuthError::UnknownRole);
        }
        self.append_actor_audit(
            actor,
            "roles.assign",
            "user",
            Some(user_id.to_owned()),
            "success",
            source_ip,
            user_agent,
        )
        .await?;
        Ok(())
    }

    pub async fn set_user_permission(
        &self,
        actor: &SessionPrincipal,
        user_id: &str,
        capability: &str,
        allow: bool,
        source_ip: &str,
        user_agent: &str,
    ) -> Result<(), AuthError> {
        require_actor(actor, "roles.manage", true)?;
        if !is_known_capability(capability) {
            return Err(AuthError::UnknownCapability);
        }
        let effect = if allow { "allow" } else { "deny" };
        sqlx::query(
            "INSERT INTO user_permissions (user_id, permission_name, effect) VALUES (?, ?, ?)
             ON CONFLICT(user_id, permission_name) DO UPDATE SET effect = excluded.effect",
        )
        .bind(user_id)
        .bind(capability)
        .bind(effect)
        .execute(&self.pool)
        .await?;
        self.append_actor_audit(
            actor,
            "permissions.user.set",
            "user",
            Some(user_id.to_owned()),
            "success",
            source_ip,
            user_agent,
        )
        .await?;
        Ok(())
    }

    pub async fn set_linux_identity(
        &self,
        actor: &SessionPrincipal,
        user_id: &str,
        uid: u32,
        gid: u32,
        source_ip: &str,
        user_agent: &str,
    ) -> Result<(), AuthError> {
        require_actor(actor, "users.manage", true)?;
        if uid == 0 || gid == 0 {
            return Err(AuthError::InvalidLinuxIdentity);
        }
        let updated = sqlx::query(
            "UPDATE users SET linux_uid = ?, linux_gid = ?, updated_at = ? WHERE id = ?",
        )
        .bind(i64::from(uid))
        .bind(i64::from(gid))
        .bind(now())
        .bind(user_id)
        .execute(&self.pool)
        .await?
        .rows_affected();
        if updated != 1 {
            return Err(AuthError::UnknownUser);
        }
        self.append_actor_audit(
            actor,
            "users.linux_identity.set",
            "user",
            Some(user_id.to_owned()),
            "success",
            source_ip,
            user_agent,
        )
        .await?;
        Ok(())
    }

    pub async fn linux_identity(
        &self,
        actor: &SessionPrincipal,
    ) -> Result<LinuxIdentityMapping, AuthError> {
        let row = sqlx::query("SELECT linux_uid, linux_gid FROM users WHERE id = ?")
            .bind(&actor.user_id)
            .fetch_one(&self.pool)
            .await?;
        let uid = row
            .get::<Option<i64>, _>("linux_uid")
            .ok_or(AuthError::LinuxIdentityNotMapped)?;
        let gid = row
            .get::<Option<i64>, _>("linux_gid")
            .ok_or(AuthError::LinuxIdentityNotMapped)?;
        Ok(LinuxIdentityMapping {
            uid: u32::try_from(uid).map_err(|_| AuthError::InvalidLinuxIdentity)?,
            gid: u32::try_from(gid).map_err(|_| AuthError::InvalidLinuxIdentity)?,
        })
    }

    pub async fn add_assigned_root(
        &self,
        actor: &SessionPrincipal,
        user_id: &str,
        path: &str,
        access_mode: AssignedRootAccess,
        source_ip: &str,
        user_agent: &str,
    ) -> Result<String, AuthError> {
        require_actor(actor, "users.manage", true)?;
        let root_id = random_identifier(16);
        sqlx::query(
            "INSERT INTO assigned_roots (id, user_id, path, access_mode, created_at)
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(&root_id)
        .bind(user_id)
        .bind(path)
        .bind(access_mode.as_str())
        .bind(now())
        .execute(&self.pool)
        .await?;
        self.append_actor_audit(
            actor,
            "users.assigned_root.add",
            "assigned_root",
            Some(root_id.clone()),
            "success",
            source_ip,
            user_agent,
        )
        .await?;
        Ok(root_id)
    }

    /// Admin-only: revoke a previously assigned root. Any Code/Files
    /// authorization that re-resolves this `root_id` afterward (which every
    /// new start, restart, workspace switch, and deep-link is required to
    /// do) will fail closed once the row is gone.
    pub async fn remove_assigned_root(
        &self,
        actor: &SessionPrincipal,
        root_id: &str,
        source_ip: &str,
        user_agent: &str,
    ) -> Result<(), AuthError> {
        require_actor(actor, "users.manage", true)?;
        let deleted = sqlx::query("DELETE FROM assigned_roots WHERE id = ?")
            .bind(root_id)
            .execute(&self.pool)
            .await?
            .rows_affected();
        if deleted == 0 {
            return Err(AuthError::UnknownAssignedRoot);
        }
        self.append_actor_audit(
            actor,
            "users.assigned_root.remove",
            "assigned_root",
            Some(root_id.to_owned()),
            "success",
            source_ip,
            user_agent,
        )
        .await?;
        Ok(())
    }

    /// Self-service: list only the calling user's own assigned roots.
    /// Returns safe display metadata (a basename-derived label, not the raw
    /// host path) plus the workspace ID (the `assigned_roots.id`) that
    /// callers use everywhere else -- the browser never sees or chooses a
    /// raw filesystem path.
    pub async fn list_own_assigned_roots(
        &self,
        actor: &SessionPrincipal,
    ) -> Result<Vec<AssignedRootSummary>, AuthError> {
        let rows = sqlx::query(
            "SELECT id, path, access_mode FROM assigned_roots WHERE user_id = ? ORDER BY created_at",
        )
        .bind(&actor.user_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|row| {
                let path: String = row.get("path");
                let access_mode: String = row.get("access_mode");
                AssignedRootSummary {
                    id: row.get("id"),
                    label: std::path::Path::new(&path)
                        .file_name()
                        .map_or_else(|| path.clone(), |name| name.to_string_lossy().into_owned()),
                    read_write: access_mode == AssignedRootAccess::ReadWrite.as_str(),
                }
            })
            .collect())
    }

    /// Resolve a workspace ID (an `assigned_roots.id`) to its canonical
    /// server-side path and access mode, but ONLY if the row still exists
    /// and still belongs to the calling user. This is the sole path by
    /// which a client-supplied identifier is ever turned into a filesystem
    /// root -- callers must never accept a raw path from the browser. A
    /// revoked, deleted, or cross-user `root_id` fails exactly the same way
    /// (`UnknownAssignedRoot`) to avoid confirming another user's root ID
    /// exists.
    pub async fn resolve_own_assigned_root(
        &self,
        actor: &SessionPrincipal,
        root_id: &str,
    ) -> Result<ResolvedAssignedRoot, AuthError> {
        let row = sqlx::query(
            "SELECT path, access_mode FROM assigned_roots WHERE id = ? AND user_id = ?",
        )
        .bind(root_id)
        .bind(&actor.user_id)
        .fetch_optional(&self.pool)
        .await?
        .ok_or(AuthError::UnknownAssignedRoot)?;
        let path: String = row.get("path");
        let access_mode: String = row.get("access_mode");
        let read_write = access_mode == AssignedRootAccess::ReadWrite.as_str();
        Ok(ResolvedAssignedRoot {
            id: root_id.to_owned(),
            path,
            read_write,
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn audit_action(
        &self,
        actor: &SessionPrincipal,
        action: &str,
        resource_type: &str,
        resource_id: Option<String>,
        result: &str,
        metadata: Value,
        source_ip: &str,
        user_agent: &str,
    ) -> Result<(), AuthError> {
        let mut event = audit_event(
            now(),
            Some(actor.user_id.clone()),
            action,
            resource_type,
            resource_id,
            result,
            source_ip,
            user_agent,
        );
        event.role_snapshot.clone_from(&actor.roles);
        event.session_id_hash = Some(actor.session_id_hash.clone());
        event.metadata = metadata;
        append_audit(&self.pool, &event).await?;
        Ok(())
    }

    pub async fn sessions(
        &self,
        actor: &SessionPrincipal,
    ) -> Result<Vec<SessionRecord>, AuthError> {
        let rows = sqlx::query(
            "SELECT id_hash, created_at, last_activity, idle_expires_at, absolute_expires_at,
                    revoked_at, step_up_expires_at, remember_device, source_ip, user_agent,
                    device_label
             FROM sessions WHERE user_id = ? ORDER BY created_at DESC",
        )
        .bind(&actor.user_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|row| SessionRecord {
                id: row.get("id_hash"),
                created_at: row.get("created_at"),
                last_activity: row.get("last_activity"),
                idle_expires_at: row.get("idle_expires_at"),
                absolute_expires_at: row.get("absolute_expires_at"),
                revoked_at: row.get("revoked_at"),
                step_up_expires_at: row.get("step_up_expires_at"),
                remember_device: row.get("remember_device"),
                source_ip: row.get("source_ip"),
                user_agent: row.get("user_agent"),
                device_label: row.get("device_label"),
                current: row.get::<String, _>("id_hash") == actor.session_id_hash,
            })
            .collect())
    }

    pub async fn revoke_session(
        &self,
        actor: &SessionPrincipal,
        session_id_hash: &str,
        source_ip: &str,
        user_agent: &str,
    ) -> Result<(), AuthError> {
        let owner: Option<String> =
            sqlx::query_scalar("SELECT user_id FROM sessions WHERE id_hash = ?")
                .bind(session_id_hash)
                .fetch_optional(&self.pool)
                .await?;
        let owner = owner.ok_or(AuthError::InvalidSession)?;
        if owner != actor.user_id && !actor.can("users.manage") {
            return Err(AuthError::PermissionDenied);
        }
        sqlx::query("UPDATE sessions SET revoked_at = ? WHERE id_hash = ? AND revoked_at IS NULL")
            .bind(now())
            .bind(session_id_hash)
            .execute(&self.pool)
            .await?;
        self.append_actor_audit(
            actor,
            "sessions.revoke",
            "session",
            Some(session_id_hash.to_owned()),
            "success",
            source_ip,
            user_agent,
        )
        .await?;
        Ok(())
    }

    pub async fn reset_totp(
        &self,
        actor: &SessionPrincipal,
        user_id: &str,
        source_ip: &str,
        user_agent: &str,
    ) -> Result<(), AuthError> {
        require_actor(actor, "users.manage", true)?;
        let mut transaction = self.pool.begin().await?;
        sqlx::query(
            "UPDATE users SET totp_secret = NULL, totp_enabled = 0, updated_at = ? WHERE id = ?",
        )
        .bind(now())
        .bind(user_id)
        .execute(&mut *transaction)
        .await?;
        sqlx::query("DELETE FROM recovery_codes WHERE user_id = ?")
            .bind(user_id)
            .execute(&mut *transaction)
            .await?;
        sqlx::query("UPDATE sessions SET revoked_at = ? WHERE user_id = ?")
            .bind(now())
            .bind(user_id)
            .execute(&mut *transaction)
            .await?;
        transaction.commit().await?;
        self.append_actor_audit(
            actor,
            "auth.totp.reset",
            "user",
            Some(user_id.to_owned()),
            "success",
            source_ip,
            user_agent,
        )
        .await?;
        Ok(())
    }

    pub async fn audit_authorization(
        &self,
        actor: Option<&SessionPrincipal>,
        capability: &str,
        allowed: bool,
        source_ip: &str,
        user_agent: &str,
    ) -> Result<(), AuthError> {
        let mut event = audit_event(
            now(),
            actor.map(|principal| principal.user_id.clone()),
            "authorization.check",
            "capability",
            Some(capability.to_owned()),
            if allowed { "success" } else { "denied" },
            source_ip,
            user_agent,
        );
        if let Some(principal) = actor {
            event.role_snapshot.clone_from(&principal.roles);
            event.session_id_hash = Some(principal.session_id_hash.clone());
        }
        append_audit(&self.pool, &event).await?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    async fn append_actor_audit(
        &self,
        actor: &SessionPrincipal,
        action: &str,
        resource_type: &str,
        resource_id: Option<String>,
        result: &str,
        source_ip: &str,
        user_agent: &str,
    ) -> Result<(), AuthError> {
        let mut event = audit_event(
            now(),
            Some(actor.user_id.clone()),
            action,
            resource_type,
            resource_id,
            result,
            source_ip,
            user_agent,
        );
        event.role_snapshot.clone_from(&actor.roles);
        event.session_id_hash = Some(actor.session_id_hash.clone());
        append_audit(&self.pool, &event).await?;
        Ok(())
    }

    async fn verify_second_factor(
        &self,
        user_id: &str,
        encrypted: &str,
        code: &str,
        timestamp: i64,
    ) -> Result<bool, AuthError> {
        let secret = self
            .cipher
            .decrypt(encrypted, totp_context(user_id).as_bytes())?;
        if verify_totp(&secret, code, timestamp) {
            return Ok(true);
        }

        let code_hash = hash_value(normalize_recovery_code(code).as_bytes());
        let updated = sqlx::query(
            "UPDATE recovery_codes SET used_at = ?
             WHERE user_id = ? AND code_hash = ? AND used_at IS NULL",
        )
        .bind(timestamp)
        .bind(user_id)
        .bind(code_hash)
        .execute(&self.pool)
        .await?
        .rows_affected();
        Ok(updated == 1)
    }

    async fn enforce_rate_limit(
        &self,
        account_key: &str,
        source_ip: &str,
        timestamp: i64,
    ) -> Result<(), AuthError> {
        let ip_key = hash_value(source_ip.as_bytes());
        let locked = sqlx::query_scalar::<_, Option<i64>>(
            "SELECT MAX(locked_until) FROM login_throttle_buckets
             WHERE (dimension = 'account' AND bucket_key = ?)
                OR (dimension = 'ip' AND bucket_key = ?)",
        )
        .bind(account_key)
        .bind(ip_key)
        .fetch_one(&self.pool)
        .await?;
        if locked.is_some_and(|locked_until| locked_until > timestamp) {
            return Err(AuthError::RateLimited);
        }
        Ok(())
    }

    async fn record_login_failure(
        &self,
        account_key: &str,
        user_id: Option<&str>,
        request: LoginRequest<'_>,
        reason: &str,
        timestamp: i64,
    ) -> Result<(), AuthError> {
        let ip_key = hash_value(request.source_ip.as_bytes());
        let mut transaction = self.pool.begin().await?;
        for (dimension, bucket_key) in [("account", account_key), ("ip", ip_key.as_str())] {
            let failure_count: i64 = sqlx::query_scalar(
                "INSERT INTO login_throttle_buckets (
                    dimension, bucket_key, failure_count, locked_until, updated_at
                 ) VALUES (?, ?, 1, 0, ?)
                 ON CONFLICT(dimension, bucket_key) DO UPDATE SET
                    failure_count = login_throttle_buckets.failure_count + 1,
                    updated_at = excluded.updated_at
                 RETURNING failure_count",
            )
            .bind(dimension)
            .bind(bucket_key)
            .bind(timestamp)
            .fetch_one(&mut *transaction)
            .await?;
            let lock_seconds = backoff_seconds(failure_count, self.policy.maximum_failures);
            sqlx::query(
                "UPDATE login_throttle_buckets SET locked_until = ?
                 WHERE dimension = ? AND bucket_key = ?",
            )
            .bind(timestamp + lock_seconds)
            .bind(dimension)
            .bind(bucket_key)
            .execute(&mut *transaction)
            .await?;
        }
        transaction.commit().await?;
        self.record_login_history(user_id, request, false, reason, timestamp)
            .await?;
        append_audit(
            &self.pool,
            &audit_event(
                timestamp,
                user_id.map(str::to_owned),
                "auth.login",
                "session",
                None,
                "failure",
                request.source_ip,
                request.user_agent,
            ),
        )
        .await?;
        Ok(())
    }

    async fn record_login_history(
        &self,
        user_id: Option<&str>,
        request: LoginRequest<'_>,
        succeeded: bool,
        reason: &str,
        timestamp: i64,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO login_history (
                user_id, attempted_username, source_ip, user_agent, succeeded, reason, created_at
             ) VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(user_id)
        .bind(normalize_username(request.username))
        .bind(request.source_ip)
        .bind(request.user_agent)
        .bind(succeeded)
        .bind(reason)
        .bind(timestamp)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn roles_for_user(&self, user_id: &str) -> Result<Vec<String>, sqlx::Error> {
        let rows = sqlx::query(
            "SELECT r.name FROM roles r JOIN user_roles ur ON ur.role_id = r.id
             WHERE ur.user_id = ? ORDER BY r.name",
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(|row| row.get("name")).collect())
    }

    async fn capabilities_for_user(&self, user_id: &str) -> Result<HashSet<String>, sqlx::Error> {
        let rows = sqlx::query(
            "SELECT permission_name, effect FROM (
                SELECT rp.permission_name, 'allow' AS effect
                FROM role_permissions rp
                JOIN user_roles ur ON ur.role_id = rp.role_id
                WHERE ur.user_id = ?
                UNION ALL
                SELECT permission_name, effect FROM user_permissions WHERE user_id = ?
             ) ORDER BY permission_name",
        )
        .bind(user_id)
        .bind(user_id)
        .fetch_all(&self.pool)
        .await?;
        let mut capabilities = HashSet::new();
        for row in rows {
            let name: String = row.get("permission_name");
            if row.get::<String, _>("effect") == "deny" {
                capabilities.remove(&name);
            } else {
                capabilities.insert(name);
            }
        }
        Ok(capabilities)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct LoginRequest<'a> {
    pub username: &'a str,
    pub password: &'a str,
    pub second_factor: Option<&'a str>,
    pub remember_device: bool,
    pub source_ip: &'a str,
    pub user_agent: &'a str,
    pub device_label: Option<&'a str>,
}

#[derive(Clone, Copy, Debug)]
pub struct CreateUserRequest<'a> {
    pub username: &'a str,
    pub display_name: &'a str,
    pub password: &'a str,
    pub role_ids: &'a [&'a str],
}

#[derive(Clone, Copy, Debug)]
pub struct BootstrapConfiguration<'a> {
    pub ui_mode: &'a str,
    pub enable_browser: bool,
    pub enable_code: bool,
    pub enable_office: bool,
    pub linux_identity: Option<(u32, u32)>,
}

impl Default for BootstrapConfiguration<'_> {
    fn default() -> Self {
        Self {
            ui_mode: "desktop",
            enable_browser: false,
            enable_code: false,
            enable_office: false,
            linux_identity: None,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct LoginSuccess {
    pub token: String,
    pub session_id_hash: String,
    pub user_id: String,
    pub username: String,
    pub absolute_expires_at: i64,
}

#[derive(Clone, Debug, Serialize)]
pub struct SessionPrincipal {
    pub user_id: String,
    pub username: String,
    pub session_id_hash: String,
    pub roles: Vec<String>,
    pub capabilities: HashSet<String>,
    pub step_up_expires_at: Option<i64>,
}

#[derive(Clone, Debug, Serialize)]
pub struct SessionRecord {
    pub id: String,
    pub created_at: i64,
    pub last_activity: i64,
    pub idle_expires_at: i64,
    pub absolute_expires_at: i64,
    pub revoked_at: Option<i64>,
    pub step_up_expires_at: Option<i64>,
    pub remember_device: bool,
    pub source_ip: String,
    pub user_agent: String,
    pub device_label: String,
    pub current: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct LinuxIdentityMapping {
    pub uid: u32,
    pub gid: u32,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum AssignedRootAccess {
    Read,
    ReadWrite,
}

impl AssignedRootAccess {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::ReadWrite => "read-write",
        }
    }
}

/// Safe, self-service display metadata for one of the caller's own
/// assigned roots. Deliberately omits the raw host path.
#[derive(Clone, Debug, Serialize)]
pub struct AssignedRootSummary {
    pub id: String,
    pub label: String,
    pub read_write: bool,
}

/// The result of authoritatively resolving a workspace ID against the
/// caller's own assigned roots. `path` is the canonical server-side
/// filesystem root; it is meant for trusted server-side use (e.g. an OCI
/// mount), not for re-exposing to the browser.
#[derive(Clone, Debug)]
pub struct ResolvedAssignedRoot {
    pub id: String,
    pub path: String,
    pub read_write: bool,
}

impl SessionPrincipal {
    #[must_use]
    pub fn can(&self, capability: &str) -> bool {
        self.capabilities.contains(capability)
    }

    #[must_use]
    pub fn has_fresh_step_up(&self) -> bool {
        self.step_up_expires_at
            .is_some_and(|expires_at| expires_at > now())
    }
}

async fn grant_role(
    transaction: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    role: &str,
    capability: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT OR IGNORE INTO role_permissions (role_id, permission_name) VALUES (?, ?)")
        .bind(role)
        .bind(capability)
        .execute(&mut **transaction)
        .await?;
    Ok(())
}

fn require_actor(
    actor: &SessionPrincipal,
    capability: &str,
    require_step_up: bool,
) -> Result<(), AuthError> {
    if !actor.can(capability) {
        return Err(AuthError::PermissionDenied);
    }
    if require_step_up && !actor.has_fresh_step_up() {
        return Err(AuthError::StepUpRequired);
    }
    Ok(())
}

fn validate_username(username: &str) -> Result<(), AuthError> {
    let normalized = normalize_username(username);
    if !(3..=64).contains(&normalized.len())
        || !normalized
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        return Err(AuthError::InvalidUsername);
    }
    Ok(())
}

fn validate_password(password: &str) -> Result<(), AuthError> {
    if password.len() < 12 || password.len() > 1024 {
        return Err(AuthError::InvalidPassword);
    }
    Ok(())
}

fn normalize_username(username: &str) -> String {
    username.trim().to_lowercase()
}

fn hash_password(password: &str) -> Result<String, AuthError> {
    let salt = SaltString::generate(&mut OsRng);
    Ok(Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map_err(|error| AuthError::PasswordHash(error.to_string()))?
        .to_string())
}

fn verify_password(password: &str, encoded: &str) -> bool {
    let Ok(hash) = PasswordHash::new(encoded) else {
        return false;
    };
    Argon2::default()
        .verify_password(password.as_bytes(), &hash)
        .is_ok()
}

#[must_use]
pub fn random_identifier(bytes: usize) -> String {
    let mut value = vec![0_u8; bytes];
    OsRng.fill_bytes(&mut value);
    URL_SAFE_NO_PAD.encode(value)
}

fn grouped_recovery_code() -> String {
    let raw = random_identifier(15).to_uppercase();
    raw.as_bytes()
        .chunks(5)
        .map(|chunk| String::from_utf8_lossy(chunk))
        .collect::<Vec<_>>()
        .join("-")
}

fn normalize_recovery_code(code: &str) -> String {
    code.chars()
        .filter(char::is_ascii_alphanumeric)
        .flat_map(char::to_uppercase)
        .collect()
}

fn hash_value(value: &[u8]) -> String {
    hex::encode(Sha256::digest(value))
}

fn totp_context(user_id: &str) -> String {
    format!("user:{user_id}:totp")
}

fn verify_totp(secret: &[u8], code: &str, timestamp: i64) -> bool {
    let normalized = code.trim();
    if normalized.len() != TOTP_DIGITS as usize
        || !normalized.bytes().all(|byte| byte.is_ascii_digit())
    {
        return false;
    }
    (-1..=1).any(|offset| {
        let candidate = totp_at(secret, timestamp + offset * TOTP_PERIOD_SECONDS);
        candidate.as_bytes().ct_eq(normalized.as_bytes()).into()
    })
}

fn totp_at(secret: &[u8], timestamp: i64) -> String {
    let counter = u64::try_from((timestamp / TOTP_PERIOD_SECONDS).max(0)).unwrap_or_default();
    let mut mac = Hmac::<Sha1>::new_from_slice(secret).expect("HMAC accepts any key length");
    mac.update(&counter.to_be_bytes());
    let digest = mac.finalize().into_bytes();
    let offset = (digest[digest.len() - 1] & 0x0f) as usize;
    let value = (u32::from(digest[offset] & 0x7f) << 24)
        | (u32::from(digest[offset + 1]) << 16)
        | (u32::from(digest[offset + 2]) << 8)
        | u32::from(digest[offset + 3]);
    format!(
        "{:0width$}",
        value % 10_u32.pow(TOTP_DIGITS),
        width = TOTP_DIGITS as usize
    )
}

#[allow(clippy::too_many_arguments)]
fn audit_event(
    timestamp: i64,
    user_id: Option<String>,
    action: &str,
    resource_type: &str,
    resource_id: Option<String>,
    result: &str,
    source_ip: &str,
    user_agent: &str,
) -> NewAuditEvent {
    NewAuditEvent {
        timestamp,
        user_id,
        role_snapshot: Vec::new(),
        session_id_hash: None,
        source_ip: source_ip.to_owned(),
        user_agent: user_agent.to_owned(),
        action: action.to_owned(),
        resource_type: resource_type.to_owned(),
        resource_id,
        path: None,
        remote_target: None,
        result: result.to_owned(),
        metadata: json!({}),
    }
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| {
            i64::try_from(duration.as_secs()).unwrap_or(i64::MAX)
        })
}

fn backoff_seconds(failure_count: i64, maximum_failures: i64) -> i64 {
    if failure_count < maximum_failures {
        return 0;
    }
    let exponent = u32::try_from((failure_count - maximum_failures).clamp(0, 8)).unwrap_or(8);
    2_i64.pow(exponent) * 30
}

#[derive(Debug, Error)]
pub enum AuthError {
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
    #[error("password hashing error: {0}")]
    PasswordHash(String),
    #[error("secret protection error: {0}")]
    Secret(#[from] clouddesk_secrets::SecretError),
    #[error("audit error: {0}")]
    Audit(#[from] clouddesk_audit::AuditError),
    #[error("username must be 3-64 lowercase-compatible letters, numbers, '.', '_' or '-'")]
    InvalidUsername,
    #[error("password must be between 12 and 1024 bytes")]
    InvalidPassword,
    #[error("initial administrator has already been created")]
    BootstrapComplete,
    #[error("invalid username, password, or second factor")]
    InvalidCredentials,
    #[error("too many login attempts; try again later")]
    RateLimited,
    #[error("session is invalid, expired, or revoked")]
    InvalidSession,
    #[error("TOTP has not been configured")]
    TotpNotConfigured,
    #[error("second factor is invalid")]
    InvalidSecondFactor,
    #[error("permission denied")]
    PermissionDenied,
    #[error("recent step-up authentication is required")]
    StepUpRequired,
    #[error("unknown role")]
    UnknownRole,
    #[error("unknown capability")]
    UnknownCapability,
    #[error("unknown user")]
    UnknownUser,
    #[error("Linux identity mapping is invalid")]
    InvalidLinuxIdentity,
    #[error("UI mode must be 'desktop' or 'dashboard'")]
    InvalidUiMode,
    #[error("user has no mapped Linux identity")]
    LinuxIdentityNotMapped,
    #[error("assigned root is unknown, revoked, or not owned by the caller")]
    UnknownAssignedRoot,
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn service() -> AuthService {
        service_with_policy(AuthPolicy::default()).await
    }

    async fn service_with_policy(policy: AuthPolicy) -> AuthService {
        let pool = clouddesk_db::connect("sqlite::memory:", 1).await.unwrap();
        clouddesk_db::migrate(&pool).await.unwrap();
        AuthService::new(pool, SecretCipher::new(&[42_u8; 32]).unwrap(), policy).unwrap()
    }

    fn request(password: &str) -> LoginRequest<'_> {
        LoginRequest {
            username: "admin",
            password,
            second_factor: None,
            remember_device: false,
            source_ip: "127.0.0.1",
            user_agent: "auth-test",
            device_label: Some("test"),
        }
    }

    #[tokio::test]
    async fn bootstrap_login_authorize_and_revoke() {
        let service = service().await;
        service
            .bootstrap_administrator(
                "Admin",
                "Administrator",
                "correct horse battery staple",
                "127.0.0.1",
                "test",
            )
            .await
            .unwrap();

        assert!(service.login(request("wrong password")).await.is_err());
        let login = service
            .login(request("correct horse battery staple"))
            .await
            .unwrap();
        let principal = service.authenticate(&login.token).await.unwrap();
        assert!(principal.can("users.manage"));

        service
            .revoke(&login.token, "127.0.0.1", "test")
            .await
            .unwrap();
        assert!(service.authenticate(&login.token).await.is_err());
    }

    #[tokio::test]
    async fn throttles_accounts_and_source_ips_independently() {
        let service = service_with_policy(AuthPolicy {
            maximum_failures: 2,
            ..AuthPolicy::default()
        })
        .await;
        service
            .bootstrap_administrator(
                "admin",
                "Administrator",
                "correct horse battery staple",
                "127.0.0.1",
                "test",
            )
            .await
            .unwrap();

        let mut attempt = request("wrong password");
        attempt.source_ip = "192.0.2.1";
        assert!(matches!(
            service.login(attempt).await,
            Err(AuthError::InvalidCredentials)
        ));
        attempt.source_ip = "192.0.2.2";
        assert!(matches!(
            service.login(attempt).await,
            Err(AuthError::InvalidCredentials)
        ));
        attempt.password = "correct horse battery staple";
        attempt.source_ip = "192.0.2.3";
        assert!(matches!(
            service.login(attempt).await,
            Err(AuthError::RateLimited)
        ));

        let another_service = service_with_policy(AuthPolicy {
            maximum_failures: 2,
            ..AuthPolicy::default()
        })
        .await;
        another_service
            .bootstrap_administrator(
                "admin",
                "Administrator",
                "correct horse battery staple",
                "127.0.0.1",
                "test",
            )
            .await
            .unwrap();
        let mut spray = request("wrong password");
        spray.source_ip = "198.51.100.10";
        spray.username = "missing-one";
        assert!(matches!(
            another_service.login(spray).await,
            Err(AuthError::InvalidCredentials)
        ));
        spray.username = "missing-two";
        assert!(matches!(
            another_service.login(spray).await,
            Err(AuthError::InvalidCredentials)
        ));
        spray.username = "admin";
        spray.password = "correct horse battery staple";
        assert!(matches!(
            another_service.login(spray).await,
            Err(AuthError::RateLimited)
        ));
    }

    #[tokio::test]
    async fn configured_bootstrap_rolls_back_every_change_on_failure() {
        let service = service().await;
        sqlx::query(
            "CREATE TRIGGER reject_bootstrap_settings BEFORE UPDATE ON system_settings
             BEGIN SELECT RAISE(ABORT, 'simulated settings failure'); END",
        )
        .execute(&service.pool)
        .await
        .unwrap();

        assert!(service
            .bootstrap_administrator_configured(
                "admin",
                "Administrator",
                "correct horse battery staple",
                BootstrapConfiguration {
                    ui_mode: "dashboard",
                    enable_browser: true,
                    enable_code: true,
                    enable_office: true,
                    linux_identity: Some((1_000, 1_000)),
                },
                "127.0.0.1",
                "test",
            )
            .await
            .is_err());

        let users: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users")
            .fetch_one(&service.pool)
            .await
            .unwrap();
        let roles: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM roles")
            .fetch_one(&service.pool)
            .await
            .unwrap();
        let audit_events: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM audit_events")
            .fetch_one(&service.pool)
            .await
            .unwrap();
        assert_eq!((users, roles, audit_events), (0, 0, 0));
    }

    #[test]
    fn totp_matches_rfc_6238_sha1_vector_shape() {
        let secret = b"12345678901234567890";
        assert_eq!(totp_at(secret, 59), "287082");
        assert!(verify_totp(secret, "287082", 59));
    }

    #[tokio::test]
    async fn second_factor_and_recovery_codes_are_single_use() {
        let service = service().await;
        let user_id = service
            .bootstrap_administrator(
                "admin",
                "Administrator",
                "correct horse battery staple",
                "127.0.0.1",
                "test",
            )
            .await
            .unwrap();
        let login = service
            .login(request("correct horse battery staple"))
            .await
            .unwrap();
        let principal = service.authenticate(&login.token).await.unwrap();
        let secret = service.begin_totp(&principal).await.unwrap();
        let secret = BASE32_NOPAD.decode(secret.as_bytes()).unwrap();
        let timestamp = now();
        let codes = service
            .confirm_totp_at(&principal, &totp_at(&secret, timestamp), timestamp)
            .await
            .unwrap();

        let mut recovery_request = request("correct horse battery staple");
        recovery_request.second_factor = Some(&codes[0]);
        assert!(service.login(recovery_request).await.is_ok());
        assert!(service.login(recovery_request).await.is_err());

        let enabled: bool = sqlx::query_scalar("SELECT totp_enabled FROM users WHERE id = ?")
            .bind(user_id)
            .fetch_one(&service.pool)
            .await
            .unwrap();
        assert!(enabled);
    }

    #[tokio::test]
    async fn granular_permissions_do_not_follow_role_names_in_callers() {
        let service = service().await;
        service
            .bootstrap_administrator(
                "admin",
                "Administrator",
                "correct horse battery staple",
                "127.0.0.1",
                "test",
            )
            .await
            .unwrap();
        let admin_login = service
            .login(request("correct horse battery staple"))
            .await
            .unwrap();
        service
            .step_up(
                &admin_login.token,
                "correct horse battery staple",
                None,
                "127.0.0.1",
                "test",
            )
            .await
            .unwrap();
        let admin = service.authenticate(&admin_login.token).await.unwrap();
        let user_id = service
            .create_user(
                &admin,
                CreateUserRequest {
                    username: "member",
                    display_name: "Member",
                    password: "another correct horse battery staple",
                    role_ids: &["user"],
                },
                "127.0.0.1",
                "test",
            )
            .await
            .unwrap();

        let mut member_request = request("another correct horse battery staple");
        member_request.username = "member";
        let member_login = service.login(member_request).await.unwrap();
        let member = service.authenticate(&member_login.token).await.unwrap();
        assert_eq!(member.user_id, user_id);
        assert!(member.can("files.local.read"));
        assert!(!member.can("users.manage"));
        assert!(matches!(
            service
                .create_user(
                    &member,
                    CreateUserRequest {
                        username: "forbidden",
                        display_name: "Forbidden",
                        password: "yet another secure password",
                        role_ids: &["guest"],
                    },
                    "127.0.0.1",
                    "test",
                )
                .await,
            Err(AuthError::PermissionDenied)
        ));
    }
}
