use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use sqlx::{Row, Sqlite, SqlitePool, Transaction};
use thiserror::Error;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct NewAuditEvent {
    pub timestamp: i64,
    pub user_id: Option<String>,
    pub role_snapshot: Vec<String>,
    pub session_id_hash: Option<String>,
    pub source_ip: String,
    pub user_agent: String,
    pub action: String,
    pub resource_type: String,
    pub resource_id: Option<String>,
    pub path: Option<String>,
    pub remote_target: Option<String>,
    pub result: String,
    pub metadata: Value,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuditReceipt {
    pub id: i64,
    pub event_hash: String,
}

pub async fn append(pool: &SqlitePool, event: &NewAuditEvent) -> Result<AuditReceipt, AuditError> {
    let mut transaction = pool.begin().await?;
    let receipt = append_in_transaction(&mut transaction, event).await?;
    transaction.commit().await?;
    Ok(receipt)
}

pub async fn append_in_transaction(
    transaction: &mut Transaction<'_, Sqlite>,
    event: &NewAuditEvent,
) -> Result<AuditReceipt, AuditError> {
    // Taking the singleton row's write lock before reading the head prevents two
    // concurrent writers from creating valid-looking branches from one hash.
    sqlx::query("UPDATE audit_chain_head SET last_hash = last_hash WHERE singleton = 1")
        .execute(&mut **transaction)
        .await?;
    let previous_hash: String =
        sqlx::query_scalar("SELECT last_hash FROM audit_chain_head WHERE singleton = 1")
            .fetch_one(&mut **transaction)
            .await?;
    let role_snapshot = serde_json::to_string(&event.role_snapshot)?;
    let metadata_json = canonical_json(&event.metadata)?;
    let event_hash = hash_event(event, &role_snapshot, &metadata_json, &previous_hash)?;

    let result = sqlx::query(
        "INSERT INTO audit_events (
            timestamp, user_id, role_snapshot, session_id_hash, source_ip, user_agent,
            action, resource_type, resource_id, path, remote_target, result,
            metadata_json, previous_hash, event_hash
         ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(event.timestamp)
    .bind(&event.user_id)
    .bind(&role_snapshot)
    .bind(&event.session_id_hash)
    .bind(&event.source_ip)
    .bind(&event.user_agent)
    .bind(&event.action)
    .bind(&event.resource_type)
    .bind(&event.resource_id)
    .bind(&event.path)
    .bind(&event.remote_target)
    .bind(&event.result)
    .bind(&metadata_json)
    .bind(&previous_hash)
    .bind(&event_hash)
    .execute(&mut **transaction)
    .await?;

    sqlx::query("UPDATE audit_chain_head SET last_hash = ? WHERE singleton = 1")
        .bind(&event_hash)
        .execute(&mut **transaction)
        .await?;
    Ok(AuditReceipt {
        id: result.last_insert_rowid(),
        event_hash,
    })
}

fn canonical_json(value: &Value) -> Result<String, serde_json::Error> {
    let value = sort_json(value.clone());
    serde_json::to_string(&value)
}

fn sort_json(value: Value) -> Value {
    match value {
        Value::Object(object) => {
            let mut entries: Vec<_> = object.into_iter().collect();
            entries.sort_by(|left, right| left.0.cmp(&right.0));
            Value::Object(
                entries
                    .into_iter()
                    .map(|(key, value)| (key, sort_json(value)))
                    .collect(),
            )
        }
        Value::Array(values) => Value::Array(values.into_iter().map(sort_json).collect()),
        other => other,
    }
}

fn hash_event(
    event: &NewAuditEvent,
    role_snapshot: &str,
    metadata_json: &str,
    previous_hash: &str,
) -> Result<String, serde_json::Error> {
    #[derive(Serialize)]
    struct CanonicalEvent<'a> {
        timestamp: i64,
        user_id: &'a Option<String>,
        role_snapshot: &'a str,
        session_id_hash: &'a Option<String>,
        source_ip: &'a str,
        user_agent: &'a str,
        action: &'a str,
        resource_type: &'a str,
        resource_id: &'a Option<String>,
        path: &'a Option<String>,
        remote_target: &'a Option<String>,
        result: &'a str,
        metadata_json: &'a str,
        previous_hash: &'a str,
    }

    let canonical = serde_json::to_vec(&CanonicalEvent {
        timestamp: event.timestamp,
        user_id: &event.user_id,
        role_snapshot,
        session_id_hash: &event.session_id_hash,
        source_ip: &event.source_ip,
        user_agent: &event.user_agent,
        action: &event.action,
        resource_type: &event.resource_type,
        resource_id: &event.resource_id,
        path: &event.path,
        remote_target: &event.remote_target,
        result: &event.result,
        metadata_json,
        previous_hash,
    })?;
    Ok(hex::encode(Sha256::digest(canonical)))
}

pub async fn verify_chain(pool: &SqlitePool) -> Result<(), AuditError> {
    let rows = sqlx::query(
        "SELECT id, timestamp, user_id, role_snapshot, session_id_hash, source_ip,
                user_agent, action, resource_type, resource_id, path, remote_target,
                result, metadata_json, previous_hash, event_hash
         FROM audit_events ORDER BY id",
    )
    .fetch_all(pool)
    .await?;

    let mut expected_previous = String::new();
    for row in rows {
        let id: i64 = row.get("id");
        let previous_hash: String = row.get("previous_hash");
        if previous_hash != expected_previous {
            return Err(AuditError::BrokenChain(id));
        }
        let role_snapshot: String = row.get("role_snapshot");
        let metadata_json: String = row.get("metadata_json");
        let event = NewAuditEvent {
            timestamp: row.get("timestamp"),
            user_id: row.get("user_id"),
            role_snapshot: serde_json::from_str(&role_snapshot)?,
            session_id_hash: row.get("session_id_hash"),
            source_ip: row.get("source_ip"),
            user_agent: row.get("user_agent"),
            action: row.get("action"),
            resource_type: row.get("resource_type"),
            resource_id: row.get("resource_id"),
            path: row.get("path"),
            remote_target: row.get("remote_target"),
            result: row.get("result"),
            metadata: serde_json::from_str(&metadata_json)?,
        };
        let actual_hash: String = row.get("event_hash");
        let calculated = hash_event(&event, &role_snapshot, &metadata_json, &previous_hash)?;
        if calculated != actual_hash {
            return Err(AuditError::InvalidHash(id));
        }
        expected_previous = actual_hash;
    }
    Ok(())
}

#[derive(Debug, Error)]
pub enum AuditError {
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
    #[error("audit serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("audit chain is broken before event {0}")]
    BrokenChain(i64),
    #[error("audit hash is invalid for event {0}")]
    InvalidHash(i64),
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    async fn database() -> SqlitePool {
        let pool = clouddesk_db::connect("sqlite::memory:", 1).await.unwrap();
        clouddesk_db::migrate(&pool).await.unwrap();
        pool
    }

    fn event(action: &str) -> NewAuditEvent {
        NewAuditEvent {
            timestamp: 1_700_000_000,
            user_id: Some("user-1".to_owned()),
            role_snapshot: vec!["Administrator".to_owned()],
            session_id_hash: Some("session-hash".to_owned()),
            source_ip: "127.0.0.1".to_owned(),
            user_agent: "test".to_owned(),
            action: action.to_owned(),
            resource_type: "session".to_owned(),
            resource_id: None,
            path: None,
            remote_target: None,
            result: "success".to_owned(),
            metadata: json!({"z": 2, "a": 1}),
        }
    }

    #[tokio::test]
    async fn events_form_a_verifiable_hash_chain() {
        let pool = database().await;
        let first = append(&pool, &event("auth.login")).await.unwrap();
        let second = append(&pool, &event("auth.logout")).await.unwrap();

        assert_ne!(first.event_hash, second.event_hash);
        verify_chain(&pool).await.unwrap();
    }

    #[tokio::test]
    async fn audit_rows_cannot_be_updated_or_deleted() {
        let pool = database().await;
        append(&pool, &event("auth.login")).await.unwrap();

        assert!(
            sqlx::query("UPDATE audit_events SET action = 'tampered' WHERE id = 1")
                .execute(&pool)
                .await
                .is_err()
        );
        assert!(sqlx::query("DELETE FROM audit_events WHERE id = 1")
            .execute(&pool)
            .await
            .is_err());
    }

    #[tokio::test]
    async fn concurrent_writers_produce_one_linear_chain() {
        let directory = tempfile::tempdir().unwrap();
        let url = format!("sqlite://{}", directory.path().join("audit.db").display());
        let pool = clouddesk_db::connect(&url, 8).await.unwrap();
        clouddesk_db::migrate(&pool).await.unwrap();

        let mut tasks = Vec::new();
        for index in 0..24 {
            let pool = pool.clone();
            tasks.push(tokio::spawn(async move {
                append(&pool, &event(&format!("concurrent.{index}"))).await
            }));
        }
        for task in tasks {
            task.await.unwrap().unwrap();
        }

        verify_chain(&pool).await.unwrap();
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM audit_events")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 24);
    }
}
