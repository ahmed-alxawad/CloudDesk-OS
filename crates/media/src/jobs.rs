//! Persisted media job lifecycle.
//!
//! A job row is the source of truth for state; nothing about a job's
//! *existence* is ever inferred from in-memory-only state, so a crash mid
//! job leaves a `running` row a restart can detect and expire rather than
//! an invisible dangling process.

use serde::Serialize;
use sqlx::{Row, SqlitePool};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum JobOperation {
    Remux,
    Transcode,
}

impl JobOperation {
    fn as_str(self) -> &'static str {
        match self {
            Self::Remux => "remux",
            Self::Transcode => "transcode",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum JobState {
    Queued,
    Probing,
    Running,
    Completed,
    Failed,
    Cancelled,
    Expired,
}

impl JobState {
    fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Probing => "probing",
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Expired => "expired",
        }
    }

    fn from_str(value: &str) -> Self {
        match value {
            "probing" => Self::Probing,
            "running" => Self::Running,
            "completed" => Self::Completed,
            "failed" => Self::Failed,
            "cancelled" => Self::Cancelled,
            "expired" => Self::Expired,
            _ => Self::Queued,
        }
    }

    /// Terminal states cannot transition further and are safe to skip
    /// during cleanup sweeps.
    #[must_use]
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Failed | Self::Cancelled | Self::Expired
        )
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct MediaJob {
    pub id: String,
    pub owner_user_id: String,
    pub source_virtual_path: String,
    pub operation: JobOperation,
    pub state: JobState,
    pub error_class: Option<String>,
    pub output_path: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub completed_at: Option<i64>,
}

fn row_to_job(row: &sqlx::sqlite::SqliteRow) -> MediaJob {
    let operation = match row.get::<String, _>("operation").as_str() {
        "transcode" => JobOperation::Transcode,
        _ => JobOperation::Remux,
    };
    MediaJob {
        id: row.get("id"),
        owner_user_id: row.get("owner_user_id"),
        source_virtual_path: row.get("source_virtual_path"),
        operation,
        state: JobState::from_str(row.get::<String, _>("state").as_str()),
        error_class: row.get("error_class"),
        output_path: row.get("output_path"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
        completed_at: row.get("completed_at"),
    }
}

#[derive(Clone)]
pub struct MediaJobStore {
    pool: SqlitePool,
}

impl MediaJobStore {
    #[must_use]
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn create(
        &self,
        owner_user_id: &str,
        source_virtual_path: &str,
        operation: JobOperation,
    ) -> Result<MediaJob, sqlx::Error> {
        let id = clouddesk_auth::random_identifier(16);
        let now = now_unix();
        sqlx::query(
            "INSERT INTO media_jobs
                (id, owner_user_id, source_virtual_path, operation, state, created_at, updated_at)
             VALUES (?, ?, ?, ?, 'queued', ?, ?)",
        )
        .bind(&id)
        .bind(owner_user_id)
        .bind(source_virtual_path)
        .bind(operation.as_str())
        .bind(now)
        .bind(now)
        .execute(&self.pool)
        .await?;
        Ok(MediaJob {
            id,
            owner_user_id: owner_user_id.to_owned(),
            source_virtual_path: source_virtual_path.to_owned(),
            operation,
            state: JobState::Queued,
            error_class: None,
            output_path: None,
            created_at: now,
            updated_at: now,
            completed_at: None,
        })
    }

    /// Ownership-scoped lookup: a job owned by someone else is
    /// indistinguishable from a nonexistent job (`None`), never a
    /// separate "forbidden" outcome that would confirm the ID exists.
    pub async fn get(
        &self,
        owner_user_id: &str,
        job_id: &str,
    ) -> Result<Option<MediaJob>, sqlx::Error> {
        let row = sqlx::query("SELECT * FROM media_jobs WHERE id = ? AND owner_user_id = ?")
            .bind(job_id)
            .bind(owner_user_id)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.as_ref().map(row_to_job))
    }

    pub async fn set_state(
        &self,
        job_id: &str,
        state: JobState,
        error_class: Option<&str>,
    ) -> Result<(), sqlx::Error> {
        let now = now_unix();
        let completed_at = state.is_terminal().then_some(now);
        sqlx::query(
            "UPDATE media_jobs SET state = ?, error_class = ?, updated_at = ?, completed_at = COALESCE(?, completed_at)
             WHERE id = ?",
        )
        .bind(state.as_str())
        .bind(error_class)
        .bind(now)
        .bind(completed_at)
        .bind(job_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn set_output(&self, job_id: &str, output_path: &str) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE media_jobs SET output_path = ?, updated_at = ? WHERE id = ?")
            .bind(output_path)
            .bind(now_unix())
            .bind(job_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Rows still in a non-terminal state whose `updated_at` predates
    /// `older_than_unix` are jobs that can no longer have a live process
    /// behind them (the process that owned them died with the server) --
    /// startup cleanup marks them `expired` so their temp output can be
    /// reclaimed and clients polling them get a real terminal answer
    /// instead of a permanently "running" job.
    pub async fn expire_stale(&self, older_than_unix: i64) -> Result<Vec<MediaJob>, sqlx::Error> {
        let rows = sqlx::query(
            "SELECT * FROM media_jobs
             WHERE state IN ('queued', 'probing', 'running') AND updated_at < ?",
        )
        .bind(older_than_unix)
        .fetch_all(&self.pool)
        .await?;
        let jobs: Vec<MediaJob> = rows.iter().map(row_to_job).collect();
        for job in &jobs {
            self.set_state(&job.id, JobState::Expired, Some("server_restarted"))
                .await?;
        }
        Ok(jobs)
    }
}

fn now_unix() -> i64 {
    i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
    )
    .unwrap_or(0)
}
