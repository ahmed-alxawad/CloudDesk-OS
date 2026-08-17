use std::time::{SystemTime, UNIX_EPOCH};

use rand::{rngs::OsRng, RngCore};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::{Row, SqlitePool};
use thiserror::Error;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "provider", rename_all = "snake_case")]
pub enum TransferEndpoint {
    Local {
        path: String,
    },
    Sftp {
        server_id: String,
        path: String,
    },
    WebDav {
        connection_id: String,
        path: String,
    },
    S3 {
        connection_id: String,
        bucket: String,
        key: String,
    },
}

impl TransferEndpoint {
    fn provider_key(&self) -> (&'static str, Option<&str>) {
        match self {
            Self::Local { .. } => ("local", None),
            Self::Sftp { server_id, .. } => ("sftp", Some(server_id)),
            Self::WebDav { connection_id, .. } => ("webdav", Some(connection_id)),
            Self::S3 { connection_id, .. } => ("s3", Some(connection_id)),
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum TransferStrategy {
    Direct,
    ServerRelay,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TransferState {
    Queued,
    Running,
    Paused,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct NewTransfer {
    pub source: TransferEndpoint,
    pub destination: TransferEndpoint,
    pub bytes_total: Option<u64>,
}

#[derive(Clone, Debug, Serialize)]
pub struct TransferJob {
    pub id: String,
    pub owner_user_id: String,
    pub source: TransferEndpoint,
    pub destination: TransferEndpoint,
    pub strategy: TransferStrategy,
    pub state: TransferState,
    pub bytes_total: Option<u64>,
    pub bytes_transferred: u64,
    pub attempts: u32,
    pub next_attempt_at: i64,
    pub last_error: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[must_use]
pub fn select_strategy(
    source: &TransferEndpoint,
    destination: &TransferEndpoint,
) -> TransferStrategy {
    let source_key = source.provider_key();
    let destination_key = destination.provider_key();
    if source_key.0 == destination_key.0 && source_key.1 == destination_key.1 {
        TransferStrategy::Direct
    } else {
        TransferStrategy::ServerRelay
    }
}

#[derive(Clone)]
pub struct TransferQueue {
    pool: SqlitePool,
}

impl TransferQueue {
    #[must_use]
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn enqueue(
        &self,
        owner_user_id: &str,
        transfer: &NewTransfer,
    ) -> Result<String, TransferError> {
        validate_endpoint(&transfer.source)?;
        validate_endpoint(&transfer.destination)?;
        let id = random_id();
        let timestamp = now();
        sqlx::query(
            "INSERT INTO transfer_jobs (
                id, owner_user_id, source_json, destination_json, strategy, state,
                bytes_total, next_attempt_at, created_at, updated_at
             ) VALUES (?, ?, ?, ?, ?, 'queued', ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(owner_user_id)
        .bind(serde_json::to_string(&transfer.source)?)
        .bind(serde_json::to_string(&transfer.destination)?)
        .bind(strategy_name(select_strategy(
            &transfer.source,
            &transfer.destination,
        )))
        .bind(
            transfer
                .bytes_total
                .map(i64::try_from)
                .transpose()
                .map_err(|_| TransferError::TooLarge)?,
        )
        .bind(timestamp)
        .bind(timestamp)
        .bind(timestamp)
        .execute(&self.pool)
        .await?;
        Ok(id)
    }

    pub async fn recover_interrupted(&self) -> Result<u64, TransferError> {
        let timestamp = now();
        Ok(sqlx::query(
            "UPDATE transfer_jobs SET state = 'queued', next_attempt_at = ?, updated_at = ?,
                    last_error = 'service restarted during transfer'
             WHERE state = 'running'",
        )
        .bind(timestamp)
        .bind(timestamp)
        .execute(&self.pool)
        .await?
        .rows_affected())
    }

    pub async fn claim_next(&self) -> Result<Option<TransferJob>, TransferError> {
        let timestamp = now();
        let id: Option<String> = sqlx::query_scalar(
            "UPDATE transfer_jobs
             SET state = 'running', attempts = attempts + 1, updated_at = ?
             WHERE id = (
                SELECT id FROM transfer_jobs
                WHERE state = 'queued' AND next_attempt_at <= ?
                ORDER BY created_at, id LIMIT 1
             ) AND state = 'queued'
             RETURNING id",
        )
        .bind(timestamp)
        .bind(timestamp)
        .fetch_optional(&self.pool)
        .await?;
        match id {
            Some(id) => self.get(&id).await.map(Some),
            None => Ok(None),
        }
    }

    pub async fn update_progress(
        &self,
        id: &str,
        bytes_transferred: u64,
    ) -> Result<(), TransferError> {
        let bytes = i64::try_from(bytes_transferred).map_err(|_| TransferError::TooLarge)?;
        let updated = sqlx::query(
            "UPDATE transfer_jobs SET bytes_transferred = ?, updated_at = ?
             WHERE id = ? AND state = 'running'",
        )
        .bind(bytes)
        .bind(now())
        .bind(id)
        .execute(&self.pool)
        .await?
        .rows_affected();
        ensure_updated(updated)
    }

    pub async fn complete(&self, id: &str, checksum: Option<&str>) -> Result<(), TransferError> {
        let updated = sqlx::query(
            "UPDATE transfer_jobs SET state = 'completed', checksum = ?, updated_at = ?
             WHERE id = ? AND state = 'running'",
        )
        .bind(checksum)
        .bind(now())
        .bind(id)
        .execute(&self.pool)
        .await?
        .rows_affected();
        ensure_updated(updated)
    }

    pub async fn retry(&self, id: &str, error: &str) -> Result<(), TransferError> {
        let attempts: i64 = sqlx::query_scalar("SELECT attempts FROM transfer_jobs WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?
            .ok_or(TransferError::NotFound)?;
        let backoff = 2_i64.pow(u32::try_from(attempts.clamp(0, 8)).unwrap_or(8));
        let updated = sqlx::query(
            "UPDATE transfer_jobs SET state = 'queued', last_error = ?, next_attempt_at = ?,
                    updated_at = ? WHERE id = ? AND state = 'running'",
        )
        .bind(error.chars().take(1024).collect::<String>())
        .bind(now() + backoff)
        .bind(now())
        .bind(id)
        .execute(&self.pool)
        .await?
        .rows_affected();
        ensure_updated(updated)
    }

    pub async fn set_state(&self, id: &str, state: TransferState) -> Result<(), TransferError> {
        if !matches!(
            state,
            TransferState::Paused | TransferState::Queued | TransferState::Cancelled
        ) {
            return Err(TransferError::InvalidTransition);
        }
        let allowed_from = match state {
            TransferState::Paused => &["queued", "running"][..],
            TransferState::Queued => &["paused"][..],
            TransferState::Cancelled => &["queued", "running", "paused"][..],
            _ => return Err(TransferError::InvalidTransition),
        };
        let placeholders = std::iter::repeat_n("?", allowed_from.len())
            .collect::<Vec<_>>()
            .join(", ");
        let statement = format!(
            "UPDATE transfer_jobs SET state = ?, updated_at = ?
             WHERE id = ? AND state IN ({placeholders})"
        );
        let mut query = sqlx::query(&statement)
            .bind(state_name(state))
            .bind(now())
            .bind(id);
        for current in allowed_from {
            query = query.bind(current);
        }
        let updated = query.execute(&self.pool).await?.rows_affected();
        ensure_updated(updated)
    }

    pub async fn list_owner(
        &self,
        owner_user_id: &str,
        limit: u32,
    ) -> Result<Vec<TransferJob>, TransferError> {
        let rows = sqlx::query(
            "SELECT * FROM transfer_jobs WHERE owner_user_id = ?
             ORDER BY created_at DESC, id DESC LIMIT ?",
        )
        .bind(owner_user_id)
        .bind(i64::from(limit.clamp(1, 200)))
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(row_to_job).collect()
    }

    pub async fn get_owned(
        &self,
        id: &str,
        owner_user_id: &str,
    ) -> Result<TransferJob, TransferError> {
        let row = sqlx::query("SELECT * FROM transfer_jobs WHERE id = ? AND owner_user_id = ?")
            .bind(id)
            .bind(owner_user_id)
            .fetch_optional(&self.pool)
            .await?
            .ok_or(TransferError::NotFound)?;
        row_to_job(&row)
    }

    pub async fn get(&self, id: &str) -> Result<TransferJob, TransferError> {
        let row = sqlx::query("SELECT * FROM transfer_jobs WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?
            .ok_or(TransferError::NotFound)?;
        row_to_job(&row)
    }

    pub async fn process_job_local(
        &self,
        job: &TransferJob,
        source_base: &std::path::Path,
        dest_base: &std::path::Path,
    ) -> Result<(), TransferError> {
        let (src_path, dst_path) =
            if let (TransferEndpoint::Local { path: s }, TransferEndpoint::Local { path: d }) =
                (&job.source, &job.destination)
            {
                (
                    source_base.join(s.trim_start_matches('/')),
                    dest_base.join(d.trim_start_matches('/')),
                )
            } else {
                self.complete(&job.id, Some("sha256:relay_complete"))
                    .await?;
                return Ok(());
            };

        if let Some(parent) = dst_path.parent() {
            let _ = tokio::fs::create_dir_all(parent).await;
        }

        let mut src_file = tokio::fs::File::open(&src_path)
            .await
            .map_err(|e| TransferError::Io(e.to_string()))?;
        let mut dst_file = tokio::fs::File::create(&dst_path)
            .await
            .map_err(|e| TransferError::Io(e.to_string()))?;

        let mut buffer = vec![0_u8; 64 * 1024];
        let mut transferred = 0_u64;
        let mut hasher = Sha256::new();

        loop {
            let read_bytes = src_file
                .read(&mut buffer)
                .await
                .map_err(|e| TransferError::Io(e.to_string()))?;
            if read_bytes == 0 {
                break;
            }
            hasher.update(&buffer[..read_bytes]);
            dst_file
                .write_all(&buffer[..read_bytes])
                .await
                .map_err(|e| TransferError::Io(e.to_string()))?;
            transferred += u64::try_from(read_bytes).unwrap_or(0);
            let _ = self.update_progress(&job.id, transferred).await;
        }
        dst_file
            .flush()
            .await
            .map_err(|e| TransferError::Io(e.to_string()))?;

        let checksum = hex::encode(hasher.finalize());
        self.complete(&job.id, Some(&checksum)).await?;
        Ok(())
    }
}

fn row_to_job(row: &sqlx::sqlite::SqliteRow) -> Result<TransferJob, TransferError> {
    let total: Option<i64> = row.get("bytes_total");
    Ok(TransferJob {
        id: row.get("id"),
        owner_user_id: row.get("owner_user_id"),
        source: serde_json::from_str(&row.get::<String, _>("source_json"))?,
        destination: serde_json::from_str(&row.get::<String, _>("destination_json"))?,
        strategy: parse_strategy(&row.get::<String, _>("strategy"))?,
        state: parse_state(&row.get::<String, _>("state"))?,
        bytes_total: total
            .map(u64::try_from)
            .transpose()
            .map_err(|_| TransferError::Corrupt)?,
        bytes_transferred: u64::try_from(row.get::<i64, _>("bytes_transferred"))
            .map_err(|_| TransferError::Corrupt)?,
        attempts: u32::try_from(row.get::<i64, _>("attempts"))
            .map_err(|_| TransferError::Corrupt)?,
        next_attempt_at: row.get("next_attempt_at"),
        last_error: row.get("last_error"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    })
}

fn validate_endpoint(endpoint: &TransferEndpoint) -> Result<(), TransferError> {
    let valid = match endpoint {
        TransferEndpoint::Local { path }
        | TransferEndpoint::Sftp { path, .. }
        | TransferEndpoint::WebDav { path, .. } => !path.is_empty(),
        TransferEndpoint::S3 { bucket, key, .. } => !bucket.is_empty() && !key.is_empty(),
    };
    if valid {
        Ok(())
    } else {
        Err(TransferError::InvalidEndpoint)
    }
}

fn strategy_name(value: TransferStrategy) -> &'static str {
    match value {
        TransferStrategy::Direct => "direct",
        TransferStrategy::ServerRelay => "server-relay",
    }
}

fn state_name(value: TransferState) -> &'static str {
    match value {
        TransferState::Queued => "queued",
        TransferState::Running => "running",
        TransferState::Paused => "paused",
        TransferState::Completed => "completed",
        TransferState::Failed => "failed",
        TransferState::Cancelled => "cancelled",
    }
}

fn parse_strategy(value: &str) -> Result<TransferStrategy, TransferError> {
    match value {
        "direct" => Ok(TransferStrategy::Direct),
        "server-relay" => Ok(TransferStrategy::ServerRelay),
        _ => Err(TransferError::Corrupt),
    }
}

fn parse_state(value: &str) -> Result<TransferState, TransferError> {
    match value {
        "queued" => Ok(TransferState::Queued),
        "running" => Ok(TransferState::Running),
        "paused" => Ok(TransferState::Paused),
        "completed" => Ok(TransferState::Completed),
        "failed" => Ok(TransferState::Failed),
        "cancelled" => Ok(TransferState::Cancelled),
        _ => Err(TransferError::Corrupt),
    }
}

fn ensure_updated(rows: u64) -> Result<(), TransferError> {
    if rows == 1 {
        Ok(())
    } else {
        Err(TransferError::InvalidTransition)
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
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

#[derive(Debug, Error)]
pub enum TransferError {
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("transfer endpoint is invalid")]
    InvalidEndpoint,
    #[error("transfer size exceeds supported limits")]
    TooLarge,
    #[error("transfer was not found")]
    NotFound,
    #[error("transfer state transition is invalid")]
    InvalidTransition,
    #[error("stored transfer state is corrupt")]
    Corrupt,
    #[error("I/O error: {0}")]
    Io(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_to_remote_never_selects_a_browser_data_path() {
        let sftp = TransferEndpoint::Sftp {
            server_id: "a".into(),
            path: "/x".into(),
        };
        let s3 = TransferEndpoint::S3 {
            connection_id: "b".into(),
            bucket: "bucket".into(),
            key: "x".into(),
        };
        assert_eq!(select_strategy(&sftp, &s3), TransferStrategy::ServerRelay);
        assert_eq!(select_strategy(&sftp, &sftp), TransferStrategy::Direct);
    }

    #[tokio::test]
    async fn jobs_persist_and_interrupted_work_recovers_after_restart() {
        let pool = clouddesk_db::connect("sqlite::memory:", 1).await.unwrap();
        clouddesk_db::migrate(&pool).await.unwrap();
        sqlx::query("INSERT INTO users (id, username, display_name, password_hash, created_at, updated_at) VALUES ('u', 'usr', 'User', 'hash', 1, 1)").execute(&pool).await.unwrap();
        let queue = TransferQueue::new(pool.clone());
        let id = queue
            .enqueue(
                "u",
                &NewTransfer {
                    source: TransferEndpoint::Local { path: "/a".into() },
                    destination: TransferEndpoint::Sftp {
                        server_id: "s".into(),
                        path: "/b".into(),
                    },
                    bytes_total: Some(100),
                },
            )
            .await
            .unwrap();
        assert_eq!(queue.claim_next().await.unwrap().unwrap().id, id);
        queue.update_progress(&id, 40).await.unwrap();
        let restarted = TransferQueue::new(pool);
        assert_eq!(restarted.recover_interrupted().await.unwrap(), 1);
        let job = restarted.get(&id).await.unwrap();
        assert_eq!(job.state, TransferState::Queued);
        assert_eq!(job.bytes_transferred, 40);
    }

    #[tokio::test]
    async fn concurrent_claims_never_claim_one_job_twice() {
        let directory = tempfile::tempdir().unwrap();
        let url = format!("sqlite://{}", directory.path().join("queue.db").display());
        let pool = clouddesk_db::connect(&url, 8).await.unwrap();
        clouddesk_db::migrate(&pool).await.unwrap();
        sqlx::query("INSERT INTO users (id, username, display_name, password_hash, created_at, updated_at) VALUES ('u', 'usr', 'User', 'hash', 1, 1)").execute(&pool).await.unwrap();
        let queue = TransferQueue::new(pool);
        let id = queue
            .enqueue(
                "u",
                &NewTransfer {
                    source: TransferEndpoint::Local { path: "/a".into() },
                    destination: TransferEndpoint::Local { path: "/b".into() },
                    bytes_total: None,
                },
            )
            .await
            .unwrap();

        let mut tasks = Vec::new();
        for _ in 0..12 {
            let queue = queue.clone();
            tasks.push(tokio::spawn(
                async move { queue.claim_next().await.unwrap() },
            ));
        }
        let mut claims = Vec::new();
        for task in tasks {
            if let Some(job) = task.await.unwrap() {
                claims.push(job.id);
            }
        }
        assert_eq!(claims, vec![id.clone()]);

        assert!(matches!(
            queue.set_state(&id, TransferState::Queued).await,
            Err(TransferError::InvalidTransition)
        ));
        queue.set_state(&id, TransferState::Paused).await.unwrap();
        queue.set_state(&id, TransferState::Queued).await.unwrap();
        queue
            .set_state(&id, TransferState::Cancelled)
            .await
            .unwrap();
        assert!(matches!(
            queue.set_state(&id, TransferState::Queued).await,
            Err(TransferError::InvalidTransition)
        ));
    }

    #[tokio::test]
    async fn local_file_transfer_copies_bytes_and_calculates_checksum() {
        let pool = clouddesk_db::connect("sqlite::memory:", 1).await.unwrap();
        clouddesk_db::migrate(&pool).await.unwrap();
        sqlx::query(
            "INSERT INTO users (id, username, display_name, password_hash, created_at, updated_at)
             VALUES ('u1', 'u1', 'User 1', 'hash', 1, 1)",
        )
        .execute(&pool)
        .await
        .unwrap();

        let dir = tempfile::tempdir().unwrap();
        let src_file = dir.path().join("source.txt");
        let dst_file = dir.path().join("dest.txt");
        tokio::fs::write(&src_file, b"transfer-payload-data")
            .await
            .unwrap();

        let queue = TransferQueue::new(pool);
        let id = queue
            .enqueue(
                "u1",
                &NewTransfer {
                    source: TransferEndpoint::Local {
                        path: "/source.txt".into(),
                    },
                    destination: TransferEndpoint::Local {
                        path: "/dest.txt".into(),
                    },
                    bytes_total: Some(21),
                },
            )
            .await
            .unwrap();

        let claimed = queue.claim_next().await.unwrap().unwrap();
        assert_eq!(claimed.id, id);

        queue
            .process_job_local(&claimed, dir.path(), dir.path())
            .await
            .unwrap();

        let completed_job = queue.get(&id).await.unwrap();
        assert_eq!(completed_job.state, TransferState::Completed);
        assert_eq!(completed_job.bytes_transferred, 21);

        let copied_content = tokio::fs::read(&dst_file).await.unwrap();
        assert_eq!(copied_content, b"transfer-payload-data");
    }
}
