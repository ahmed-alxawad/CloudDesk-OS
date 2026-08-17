use std::time::{SystemTime, UNIX_EPOCH};

use clouddesk_secrets::SecretCipher;
use rand::{rngs::OsRng, RngCore};
use serde::Serialize;
use sqlx::{Row, SqlitePool};
use thiserror::Error;
use zeroize::Zeroizing;

#[derive(Clone)]
pub struct Vault {
    pool: SqlitePool,
    cipher: SecretCipher,
}

impl Vault {
    #[must_use]
    pub fn new(pool: SqlitePool, cipher: SecretCipher) -> Self {
        Self { pool, cipher }
    }

    pub async fn create(
        &self,
        owner_user_id: &str,
        kind: &str,
        label: &str,
        value: &[u8],
    ) -> Result<String, VaultError> {
        validate_metadata(kind, label, value)?;
        let id = random_id();
        let data_key = random_data_key();
        let data_cipher = SecretCipher::new(data_key.as_ref())?;
        let encrypted = data_cipher.encrypt(value, &value_context(owner_user_id, &id))?;
        let encrypted_data_key = self
            .cipher
            .encrypt(data_key.as_ref(), &key_context(owner_user_id, &id))?;
        let timestamp = now();
        sqlx::query(
            "INSERT INTO vault_secrets (
                id, owner_user_id, kind, label, encrypted_value, encrypted_data_key,
                created_at, updated_at
             ) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(owner_user_id)
        .bind(kind)
        .bind(label.trim())
        .bind(encrypted)
        .bind(encrypted_data_key)
        .bind(timestamp)
        .bind(timestamp)
        .execute(&self.pool)
        .await?;
        Ok(id)
    }

    pub async fn list(&self, owner_user_id: &str) -> Result<Vec<SecretMetadata>, VaultError> {
        let rows = sqlx::query(
            "SELECT id, kind, label, created_at, updated_at, last_revealed_at
             FROM vault_secrets WHERE owner_user_id = ? ORDER BY label, id",
        )
        .bind(owner_user_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|row| SecretMetadata {
                id: row.get("id"),
                kind: row.get("kind"),
                label: row.get("label"),
                created_at: row.get("created_at"),
                updated_at: row.get("updated_at"),
                last_revealed_at: row.get("last_revealed_at"),
            })
            .collect())
    }

    pub async fn reveal(
        &self,
        owner_user_id: &str,
        id: &str,
    ) -> Result<Zeroizing<Vec<u8>>, VaultError> {
        let row = sqlx::query(
            "SELECT encrypted_value, encrypted_data_key FROM vault_secrets
             WHERE id = ? AND owner_user_id = ?",
        )
        .bind(id)
        .bind(owner_user_id)
        .fetch_optional(&self.pool)
        .await?
        .ok_or(VaultError::NotFound)?;
        let encrypted: String = row.get("encrypted_value");
        let encrypted_data_key: Option<String> = row.get("encrypted_data_key");
        let plaintext = if let Some(encrypted_data_key) = encrypted_data_key {
            let data_key = self
                .cipher
                .decrypt(&encrypted_data_key, &key_context(owner_user_id, id))?;
            let data_cipher = SecretCipher::new(data_key.as_ref())?;
            data_cipher.decrypt(&encrypted, &value_context(owner_user_id, id))?
        } else {
            // Read compatibility for records created before envelope keys were introduced.
            let plaintext = self
                .cipher
                .decrypt(&encrypted, &legacy_context(owner_user_id, id))?;
            // Lazy migration: upgrade legacy direct-key record to envelope encryption
            // so subsequent reveals use per-record DEKs. Failure is non-fatal since
            // decryption already succeeded — the legacy record remains readable.
            if let Err(error) = self
                .upgrade_to_envelope(owner_user_id, id, &plaintext)
                .await
            {
                tracing::warn!(
                    %error,
                    owner = owner_user_id,
                    id,
                    "lazy vault envelope migration failed"
                );
            }
            plaintext
        };
        sqlx::query("UPDATE vault_secrets SET last_revealed_at = ? WHERE id = ?")
            .bind(now())
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(plaintext)
    }

    pub async fn rotate(
        &self,
        owner_user_id: &str,
        id: &str,
        value: &[u8],
    ) -> Result<(), VaultError> {
        if value.is_empty() || value.len() > 1024 * 1024 {
            return Err(VaultError::InvalidValue);
        }
        let data_key = random_data_key();
        let data_cipher = SecretCipher::new(data_key.as_ref())?;
        let encrypted = data_cipher.encrypt(value, &value_context(owner_user_id, id))?;
        let encrypted_data_key = self
            .cipher
            .encrypt(data_key.as_ref(), &key_context(owner_user_id, id))?;
        let updated = sqlx::query(
            "UPDATE vault_secrets SET encrypted_value = ?, encrypted_data_key = ?, updated_at = ?
             WHERE id = ? AND owner_user_id = ?",
        )
        .bind(encrypted)
        .bind(encrypted_data_key)
        .bind(now())
        .bind(id)
        .bind(owner_user_id)
        .execute(&self.pool)
        .await?
        .rows_affected();
        if updated == 1 {
            Ok(())
        } else {
            Err(VaultError::NotFound)
        }
    }

    pub async fn delete(&self, owner_user_id: &str, id: &str) -> Result<(), VaultError> {
        let mut transaction = self.pool.begin().await?;
        let overwritten = sqlx::query(
            "UPDATE vault_secrets
             SET encrypted_value = ?, encrypted_data_key = ?, updated_at = ?
             WHERE id = ? AND owner_user_id = ?",
        )
        .bind(random_tombstone())
        .bind(random_tombstone())
        .bind(now())
        .bind(id)
        .bind(owner_user_id)
        .execute(&mut *transaction)
        .await?
        .rows_affected();
        if overwritten != 1 {
            return Err(VaultError::NotFound);
        }
        let deleted = sqlx::query("DELETE FROM vault_secrets WHERE id = ? AND owner_user_id = ?")
            .bind(id)
            .bind(owner_user_id)
            .execute(&mut *transaction)
            .await?
            .rows_affected();
        if deleted != 1 {
            return Err(VaultError::NotFound);
        }
        transaction.commit().await?;
        Ok(())
    }

    /// Upgrade a legacy direct-key record to envelope encryption.
    /// The plaintext must already have been verified via the legacy path.
    /// This generates a new random DEK, re-encrypts the value with it, and
    /// wraps the DEK with the master key — all while preserving the record ID,
    /// owner, kind, label, and `created_at`.
    async fn upgrade_to_envelope(
        &self,
        owner_user_id: &str,
        id: &str,
        plaintext: &[u8],
    ) -> Result<(), VaultError> {
        let data_key = random_data_key();
        let data_cipher = SecretCipher::new(data_key.as_ref())?;
        let encrypted = data_cipher.encrypt(plaintext, &value_context(owner_user_id, id))?;
        let encrypted_data_key = self
            .cipher
            .encrypt(data_key.as_ref(), &key_context(owner_user_id, id))?;
        sqlx::query(
            "UPDATE vault_secrets SET encrypted_value = ?, encrypted_data_key = ?, updated_at = ?
             WHERE id = ? AND owner_user_id = ?",
        )
        .bind(encrypted)
        .bind(encrypted_data_key)
        .bind(now())
        .bind(id)
        .bind(owner_user_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Rewrap all envelope DEKs with a new master key cipher.
    /// This allows zero-downtime master key (KEK) rotation without decrypting or
    /// exposing the secret payloads. Any remaining legacy direct-key records are
    /// upgraded to envelope encryption during rewrapping.
    pub async fn rewrap_all_keys(
        &self,
        old_cipher: &SecretCipher,
        new_cipher: &SecretCipher,
    ) -> Result<usize, VaultError> {
        let mut transaction = self.pool.begin().await?;
        let rows = sqlx::query(
            "SELECT id, owner_user_id, encrypted_value, encrypted_data_key FROM vault_secrets",
        )
        .fetch_all(&mut *transaction)
        .await?;

        let mut rewrapped_count = 0;
        for row in rows {
            let id: String = row.get("id");
            let owner_user_id: String = row.get("owner_user_id");
            let encrypted: String = row.get("encrypted_value");
            let encrypted_data_key: Option<String> = row.get("encrypted_data_key");

            if let Some(edk) = encrypted_data_key {
                let data_key = old_cipher.decrypt(&edk, &key_context(&owner_user_id, &id))?;
                let new_edk =
                    new_cipher.encrypt(data_key.as_ref(), &key_context(&owner_user_id, &id))?;
                sqlx::query(
                    "UPDATE vault_secrets SET encrypted_data_key = ?, updated_at = ? WHERE id = ?",
                )
                .bind(new_edk)
                .bind(now())
                .bind(&id)
                .execute(&mut *transaction)
                .await?;
            } else {
                let plaintext =
                    old_cipher.decrypt(&encrypted, &legacy_context(&owner_user_id, &id))?;
                let data_key = random_data_key();
                let data_cipher = SecretCipher::new(data_key.as_ref())?;
                let new_encrypted =
                    data_cipher.encrypt(&plaintext, &value_context(&owner_user_id, &id))?;
                let new_edk =
                    new_cipher.encrypt(data_key.as_ref(), &key_context(&owner_user_id, &id))?;
                sqlx::query(
                    "UPDATE vault_secrets SET encrypted_value = ?, encrypted_data_key = ?, updated_at = ? WHERE id = ?",
                )
                .bind(new_encrypted)
                .bind(new_edk)
                .bind(now())
                .bind(&id)
                .execute(&mut *transaction)
                .await?;
            }
            rewrapped_count += 1;
        }

        transaction.commit().await?;
        Ok(rewrapped_count)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct SecretMetadata {
    pub id: String,
    pub kind: String,
    pub label: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub last_revealed_at: Option<i64>,
}

fn validate_metadata(kind: &str, label: &str, value: &[u8]) -> Result<(), VaultError> {
    if kind.is_empty()
        || kind.len() > 64
        || !kind
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        return Err(VaultError::InvalidKind);
    }
    if label.trim().is_empty() || label.len() > 256 {
        return Err(VaultError::InvalidLabel);
    }
    if value.is_empty() || value.len() > 1024 * 1024 {
        return Err(VaultError::InvalidValue);
    }
    Ok(())
}

fn legacy_context(owner_user_id: &str, id: &str) -> Vec<u8> {
    format!("vault:{owner_user_id}:{id}").into_bytes()
}

fn key_context(owner_user_id: &str, id: &str) -> Vec<u8> {
    format!("vault:key:{owner_user_id}:{id}").into_bytes()
}

fn value_context(owner_user_id: &str, id: &str) -> Vec<u8> {
    format!("vault:value:{owner_user_id}:{id}").into_bytes()
}

fn random_data_key() -> Zeroizing<[u8; 32]> {
    let mut bytes = [0_u8; 32];
    OsRng.fill_bytes(&mut bytes);
    Zeroizing::new(bytes)
}

fn random_tombstone() -> String {
    let mut bytes = [0_u8; 64];
    OsRng.fill_bytes(&mut bytes);
    hex::encode(bytes)
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
pub enum VaultError {
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
    #[error("secret protection failed: {0}")]
    Secret(#[from] clouddesk_secrets::SecretError),
    #[error("secret kind is invalid")]
    InvalidKind,
    #[error("secret label is invalid")]
    InvalidLabel,
    #[error("secret value is invalid")]
    InvalidValue,
    #[error("secret was not found")]
    NotFound,
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};

    /// Set up an in-memory database with migrated schema and two test users.
    /// Returns two vaults with different master keys plus the shared pool.
    async fn setup() -> (Vault, Vault, SqlitePool) {
        let pool = clouddesk_db::connect("sqlite::memory:", 1).await.unwrap();
        clouddesk_db::migrate(&pool).await.unwrap();
        sqlx::query(
            "INSERT INTO users (id, username, display_name, password_hash, created_at, updated_at)
             VALUES ('one', 'one', 'One', 'hash', 1, 1), ('two', 'two', 'Two', 'hash', 1, 1)",
        )
        .execute(&pool)
        .await
        .unwrap();
        let vault_a = Vault::new(pool.clone(), SecretCipher::new(&[19_u8; 32]).unwrap());
        let vault_b = Vault::new(pool.clone(), SecretCipher::new(&[29_u8; 32]).unwrap());
        (vault_a, vault_b, pool)
    }

    /// Flip a single byte in a base64-encoded envelope at the given position.
    fn tamper_envelope(encoded: &str, byte_position: usize) -> String {
        let mut decoded = URL_SAFE_NO_PAD.decode(encoded.as_bytes()).unwrap();
        decoded[byte_position] ^= 0xFF;
        URL_SAFE_NO_PAD.encode(&decoded)
    }

    // -----------------------------------------------------------------------
    // 1. Plaintext absent from DB
    // -----------------------------------------------------------------------
    #[tokio::test]
    async fn plaintext_never_appears_in_database() {
        let (vault, _, pool) = setup().await;
        let id = vault
            .create("one", "ssh.password", "Server", b"super-secret-value")
            .await
            .unwrap();

        let row = sqlx::query(
            "SELECT encrypted_value, encrypted_data_key, kind, label FROM vault_secrets WHERE id = ?",
        )
        .bind(&id)
        .fetch_one(&pool)
        .await
        .unwrap();

        let stored: String = row.get("encrypted_value");
        let edk: String = row.get("encrypted_data_key");
        let kind: String = row.get("kind");
        let label: String = row.get("label");

        assert!(!stored.contains("super-secret-value"));
        assert!(!edk.contains("super-secret-value"));
        assert!(!kind.contains("super-secret-value"));
        assert!(!label.contains("super-secret-value"));
    }

    // -----------------------------------------------------------------------
    // 2. Per-record DEKs differ
    // -----------------------------------------------------------------------
    #[tokio::test]
    async fn per_record_deks_differ() {
        let (vault, _, pool) = setup().await;
        let id1 = vault
            .create("one", "ssh.password", "Server1", b"secret-1")
            .await
            .unwrap();
        let id2 = vault
            .create("one", "ssh.password", "Server2", b"secret-2")
            .await
            .unwrap();

        let edk1: String =
            sqlx::query_scalar("SELECT encrypted_data_key FROM vault_secrets WHERE id = ?")
                .bind(&id1)
                .fetch_one(&pool)
                .await
                .unwrap();
        let edk2: String =
            sqlx::query_scalar("SELECT encrypted_data_key FROM vault_secrets WHERE id = ?")
                .bind(&id2)
                .fetch_one(&pool)
                .await
                .unwrap();

        assert_ne!(edk1, edk2, "per-record DEKs must differ");
    }

    // -----------------------------------------------------------------------
    // 3. Same plaintext produces different ciphertext
    // -----------------------------------------------------------------------
    #[tokio::test]
    async fn same_plaintext_produces_different_ciphertext() {
        let (vault, _, pool) = setup().await;
        let id1 = vault
            .create("one", "ssh.password", "Server1", b"identical-secret")
            .await
            .unwrap();
        let id2 = vault
            .create("one", "ssh.password", "Server2", b"identical-secret")
            .await
            .unwrap();

        let enc1: String =
            sqlx::query_scalar("SELECT encrypted_value FROM vault_secrets WHERE id = ?")
                .bind(&id1)
                .fetch_one(&pool)
                .await
                .unwrap();
        let enc2: String =
            sqlx::query_scalar("SELECT encrypted_value FROM vault_secrets WHERE id = ?")
                .bind(&id2)
                .fetch_one(&pool)
                .await
                .unwrap();

        assert_ne!(
            enc1, enc2,
            "same plaintext must produce different ciphertext"
        );

        // Both should still decrypt to the same plaintext
        assert_eq!(
            vault.reveal("one", &id1).await.unwrap().as_slice(),
            b"identical-secret"
        );
        assert_eq!(
            vault.reveal("one", &id2).await.unwrap().as_slice(),
            b"identical-secret"
        );
    }

    // -----------------------------------------------------------------------
    // 4. Cross-user reveal denied
    // -----------------------------------------------------------------------
    #[tokio::test]
    async fn cross_user_reveal_denied() {
        let (vault_a, vault_b, _pool) = setup().await;
        let id = vault_a
            .create("one", "ssh.password", "Server", b"secret")
            .await
            .unwrap();

        // User "two" cannot reveal user "one"'s secret
        assert!(matches!(
            vault_b.reveal("two", &id).await,
            Err(VaultError::NotFound)
        ));

        // User "two" cannot list user "one"'s secrets
        let user_two_secrets = vault_b.list("two").await.unwrap();
        assert!(user_two_secrets.is_empty());
    }

    // -----------------------------------------------------------------------
    // 5. AAD / owner binding — ciphertext tampering fails
    // -----------------------------------------------------------------------
    #[tokio::test]
    async fn ciphertext_tampering_fails() {
        let (vault, _, pool) = setup().await;
        let id = vault
            .create("one", "ssh.password", "Server", b"secret")
            .await
            .unwrap();

        let encrypted: String =
            sqlx::query_scalar("SELECT encrypted_value FROM vault_secrets WHERE id = ?")
                .bind(&id)
                .fetch_one(&pool)
                .await
                .unwrap();
        // Byte 15 is in the ciphertext+tag region (envelope: version(1)+nonce(12)+ct+tag)
        let tampered = tamper_envelope(&encrypted, 15);
        sqlx::query("UPDATE vault_secrets SET encrypted_value = ? WHERE id = ?")
            .bind(&tampered)
            .bind(&id)
            .execute(&pool)
            .await
            .unwrap();

        assert!(matches!(
            vault.reveal("one", &id).await,
            Err(VaultError::Secret(_))
        ));
    }

    // -----------------------------------------------------------------------
    // 6. Wrapped-DEK tampering fails
    // -----------------------------------------------------------------------
    #[tokio::test]
    async fn wrapped_dek_tampering_fails() {
        let (vault, _, pool) = setup().await;
        let id = vault
            .create("one", "ssh.password", "Server", b"secret")
            .await
            .unwrap();

        let edk: String =
            sqlx::query_scalar("SELECT encrypted_data_key FROM vault_secrets WHERE id = ?")
                .bind(&id)
                .fetch_one(&pool)
                .await
                .unwrap();
        let tampered = tamper_envelope(&edk, 15);
        sqlx::query("UPDATE vault_secrets SET encrypted_data_key = ? WHERE id = ?")
            .bind(&tampered)
            .bind(&id)
            .execute(&pool)
            .await
            .unwrap();

        assert!(matches!(
            vault.reveal("one", &id).await,
            Err(VaultError::Secret(_))
        ));
    }

    // -----------------------------------------------------------------------
    // 7. Wrong master key fails
    // -----------------------------------------------------------------------
    #[tokio::test]
    async fn wrong_master_key_fails() {
        let (vault_a, vault_b, _pool) = setup().await;
        let id = vault_a
            .create("one", "ssh.password", "Server", b"secret")
            .await
            .unwrap();

        // vault_b uses a different master key
        assert!(matches!(
            vault_b.reveal("one", &id).await,
            Err(VaultError::Secret(_))
        ));
    }

    // -----------------------------------------------------------------------
    // 8. Rotation generates new DEK and ciphertext
    // -----------------------------------------------------------------------
    #[tokio::test]
    async fn rotation_changes_dek_and_ciphertext() {
        let (vault, _, pool) = setup().await;
        let id = vault
            .create("one", "ssh.password", "Server", b"original-secret")
            .await
            .unwrap();

        let old_value: String =
            sqlx::query_scalar("SELECT encrypted_value FROM vault_secrets WHERE id = ?")
                .bind(&id)
                .fetch_one(&pool)
                .await
                .unwrap();
        let old_dek: String =
            sqlx::query_scalar("SELECT encrypted_data_key FROM vault_secrets WHERE id = ?")
                .bind(&id)
                .fetch_one(&pool)
                .await
                .unwrap();

        vault.rotate("one", &id, b"rotated-secret").await.unwrap();

        let new_value: String =
            sqlx::query_scalar("SELECT encrypted_value FROM vault_secrets WHERE id = ?")
                .bind(&id)
                .fetch_one(&pool)
                .await
                .unwrap();
        let new_dek: String =
            sqlx::query_scalar("SELECT encrypted_data_key FROM vault_secrets WHERE id = ?")
                .bind(&id)
                .fetch_one(&pool)
                .await
                .unwrap();

        assert_ne!(old_value, new_value, "rotation must change ciphertext");
        assert_ne!(old_dek, new_dek, "rotation must change wrapped DEK");

        // Reveal should return the rotated value
        assert_eq!(
            vault.reveal("one", &id).await.unwrap().as_slice(),
            b"rotated-secret"
        );
    }

    // -----------------------------------------------------------------------
    // 9. Deletion prevents recovery
    // -----------------------------------------------------------------------
    #[tokio::test]
    async fn deletion_prevents_recovery() {
        let (vault, _, pool) = setup().await;
        let id = vault
            .create("one", "ssh.password", "Server", b"secret")
            .await
            .unwrap();
        vault.delete("one", &id).await.unwrap();

        // Reveal should fail
        assert!(matches!(
            vault.reveal("one", &id).await,
            Err(VaultError::NotFound)
        ));

        // Row should be physically gone
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM vault_secrets WHERE id = ?")
            .bind(&id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 0);
    }

    // -----------------------------------------------------------------------
    // 10. Legacy record migration (backward compatibility + lazy upgrade)
    // -----------------------------------------------------------------------
    #[tokio::test]
    async fn legacy_record_migrated_on_reveal() {
        let pool = clouddesk_db::connect("sqlite::memory:", 1).await.unwrap();
        clouddesk_db::migrate(&pool).await.unwrap();
        sqlx::query(
            "INSERT INTO users (id, username, display_name, password_hash, created_at, updated_at)
             VALUES ('owner', 'owner', 'Owner', 'hash', 1, 1)",
        )
        .execute(&pool)
        .await
        .unwrap();

        let cipher = SecretCipher::new(&[19_u8; 32]).unwrap();
        let vault = Vault::new(pool.clone(), cipher.clone());

        // Insert a legacy record: encrypted directly with master key, encrypted_data_key = NULL
        let legend = "legacyrecord0000000000000000";
        let encrypted = cipher
            .encrypt(b"legacy-secret-value", &legacy_context("owner", legend))
            .unwrap();
        sqlx::query(
            "INSERT INTO vault_secrets (
                id, owner_user_id, kind, label, encrypted_value, encrypted_data_key,
                created_at, updated_at
             ) VALUES (?, ?, ?, ?, ?, NULL, ?, ?)",
        )
        .bind(legend)
        .bind("owner")
        .bind("legacy.kind")
        .bind("Legacy Secret")
        .bind(&encrypted)
        .bind(1)
        .bind(1)
        .execute(&pool)
        .await
        .unwrap();

        // Reveal should decrypt the legacy record AND lazily migrate it
        let plaintext = vault.reveal("owner", legend).await.unwrap();
        assert_eq!(plaintext.as_slice(), b"legacy-secret-value");

        // After lazy migration, encrypted_data_key should no longer be NULL
        let edk: Option<String> =
            sqlx::query_scalar("SELECT encrypted_data_key FROM vault_secrets WHERE id = ?")
                .bind(legend)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(
            edk.is_some(),
            "legacy record should be lazily migrated to envelope encryption"
        );

        // Subsequent reveal should use the envelope path
        let plaintext2 = vault.reveal("owner", legend).await.unwrap();
        assert_eq!(plaintext2.as_slice(), b"legacy-secret-value");
    }

    // -----------------------------------------------------------------------
    // 11. Error messages do not leak secret values
    // -----------------------------------------------------------------------
    #[tokio::test]
    async fn error_messages_do_not_leak_secret_values() {
        let (vault, _, pool) = setup().await;
        let id = vault
            .create("one", "ssh.password", "Server", b"very-unique-secret-value")
            .await
            .unwrap();

        // Tamper to trigger a decryption error
        let encrypted: String =
            sqlx::query_scalar("SELECT encrypted_value FROM vault_secrets WHERE id = ?")
                .bind(&id)
                .fetch_one(&pool)
                .await
                .unwrap();
        let tampered = tamper_envelope(&encrypted, 15);
        sqlx::query("UPDATE vault_secrets SET encrypted_value = ? WHERE id = ?")
            .bind(&tampered)
            .bind(&id)
            .execute(&pool)
            .await
            .unwrap();

        let result = vault.reveal("one", &id).await;
        let err = result.expect_err("tampered ciphertext should fail");
        let error_string = format!("{err:?}");
        assert!(
            !error_string.contains("very-unique-secret-value"),
            "error must not leak secret value: {error_string}"
        );
        let display_string = format!("{err}");
        assert!(
            !display_string.contains("very-unique-secret-value"),
            "error Display must not leak secret value: {display_string}"
        );
    }

    // -----------------------------------------------------------------------
    // 12. List returns no secret values (secret redaction)
    // -----------------------------------------------------------------------
    #[tokio::test]
    async fn list_returns_no_secret_values() {
        let (vault, _, _pool) = setup().await;
        let id = vault
            .create("one", "ssh.password", "Server", b"top-secret-data")
            .await
            .unwrap();

        let secrets = vault.list("one").await.unwrap();
        assert_eq!(secrets.len(), 1);
        // Metadata should not contain the secret value
        assert_eq!(secrets[0].id, id);
        assert_eq!(secrets[0].kind, "ssh.password");
        assert_eq!(secrets[0].label, "Server");
        assert_ne!(secrets[0].id, "top-secret-data");
        assert_ne!(secrets[0].kind, "top-secret-data");
    }

    // -----------------------------------------------------------------------
    // 13. Owner tampering in DB fails decryption (AAD protection)
    // -----------------------------------------------------------------------
    #[tokio::test]
    async fn owner_tampering_in_database_fails_decryption() {
        let (vault, _, pool) = setup().await;
        let id = vault
            .create("one", "ssh.password", "Server", b"owner-bound-secret")
            .await
            .unwrap();

        // Directly mutate owner_user_id in SQLite to bypass row-level checks
        sqlx::query("UPDATE vault_secrets SET owner_user_id = 'two' WHERE id = ?")
            .bind(&id)
            .execute(&pool)
            .await
            .unwrap();

        // Reveal as user two: finds the row, but fails cryptographically because AAD is bound to 'one'
        let result = vault.reveal("two", &id).await;
        assert!(matches!(result, Err(VaultError::Secret(_))));
    }

    // -----------------------------------------------------------------------
    // 14. Master key (KEK) rotation rewraps all DEKs safely
    // -----------------------------------------------------------------------
    #[tokio::test]
    async fn master_kek_rotation_rewraps_all_records() {
        let pool = clouddesk_db::connect("sqlite::memory:", 1).await.unwrap();
        clouddesk_db::migrate(&pool).await.unwrap();
        sqlx::query(
            "INSERT INTO users (id, username, display_name, password_hash, created_at, updated_at)
             VALUES ('user1', 'user1', 'User 1', 'hash', 1, 1)",
        )
        .execute(&pool)
        .await
        .unwrap();

        let old_cipher = SecretCipher::new(&[11_u8; 32]).unwrap();
        let new_cipher = SecretCipher::new(&[22_u8; 32]).unwrap();

        let old_vault = Vault::new(pool.clone(), old_cipher.clone());
        let id1 = old_vault
            .create("user1", "ssh.key", "Key 1", b"secret-key-1")
            .await
            .unwrap();
        let id2 = old_vault
            .create("user1", "ssh.password", "Pass 2", b"secret-pass-2")
            .await
            .unwrap();

        // Add a legacy direct-key record as well
        let legacy_id = "legacyrecord0000000000000002";
        let legacy_enc = old_cipher
            .encrypt(b"legacy-val", &legacy_context("user1", legacy_id))
            .unwrap();
        sqlx::query(
            "INSERT INTO vault_secrets (
                id, owner_user_id, kind, label, encrypted_value, encrypted_data_key,
                created_at, updated_at
             ) VALUES (?, 'user1', 'legacy.kind', 'Legacy', ?, NULL, 1, 1)",
        )
        .bind(legacy_id)
        .bind(&legacy_enc)
        .execute(&pool)
        .await
        .unwrap();

        // Perform KEK rewrapping
        let count = old_vault
            .rewrap_all_keys(&old_cipher, &new_cipher)
            .await
            .unwrap();
        assert_eq!(count, 3);

        // New vault initialized with new master key can reveal everything
        let new_vault = Vault::new(pool.clone(), new_cipher.clone());
        assert_eq!(
            new_vault.reveal("user1", &id1).await.unwrap().as_slice(),
            b"secret-key-1"
        );
        assert_eq!(
            new_vault.reveal("user1", &id2).await.unwrap().as_slice(),
            b"secret-pass-2"
        );
        assert_eq!(
            new_vault
                .reveal("user1", legacy_id)
                .await
                .unwrap()
                .as_slice(),
            b"legacy-val"
        );

        // Old vault with old key now fails on all records
        assert!(matches!(
            old_vault.reveal("user1", &id1).await,
            Err(VaultError::Secret(_))
        ));
        assert!(matches!(
            old_vault.reveal("user1", &id2).await,
            Err(VaultError::Secret(_))
        ));
        assert!(matches!(
            old_vault.reveal("user1", legacy_id).await,
            Err(VaultError::Secret(_))
        ));
    }

    // -----------------------------------------------------------------------
    // 15. Input validation rejects invalid metadata
    // -----------------------------------------------------------------------
    #[tokio::test]
    async fn metadata_validation_rejects_invalid_inputs() {
        let (vault, _, _) = setup().await;
        assert!(matches!(
            vault.create("one", "", "Label", b"val").await,
            Err(VaultError::InvalidKind)
        ));
        assert!(matches!(
            vault.create("one", "invalid kind!", "Label", b"val").await,
            Err(VaultError::InvalidKind)
        ));
        assert!(matches!(
            vault.create("one", "kind", "   ", b"val").await,
            Err(VaultError::InvalidLabel)
        ));
        assert!(matches!(
            vault.create("one", "kind", "Label", b"").await,
            Err(VaultError::InvalidValue)
        ));
    }
}
