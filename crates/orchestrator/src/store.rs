//! `SQLite` persistence for runtime configuration and instance metadata
//! (Task 26). Persisted state is management/recovery bookkeeping only --
//! `crate::manager` treats a DB row as a *hint* to reconcile against
//! live process reality on startup (Task 27), never as authoritative by
//! itself.

use crate::model::{InstanceId, InstanceState, Persistence, RuntimeKind};
use serde::Serialize;
use sqlx::{Row, SqlitePool};

fn now() -> i64 {
    i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
    )
    .unwrap_or(0)
}

#[derive(Clone)]
pub struct RuntimeStore {
    pool: SqlitePool,
}

#[derive(Clone, Debug, Serialize)]
pub struct InstanceRow {
    pub kind: RuntimeKind,
    pub owner_user_id: String,
    pub instance_id: String,
    pub generation: i64,
    pub state: InstanceState,
    pub persistence: Persistence,
    pub port: Option<i64>,
    pub pid: Option<i64>,
    pub created_at: i64,
    pub updated_at: i64,
    pub last_activity_at: i64,
    pub restart_count: i64,
    pub exit_code: Option<i64>,
    pub exit_signal: Option<String>,
    pub failure_message: Option<String>,
}

fn row_to_instance(row: &sqlx::sqlite::SqliteRow) -> Option<InstanceRow> {
    Some(InstanceRow {
        kind: RuntimeKind::parse(&row.get::<String, _>("kind"))?,
        owner_user_id: row.get("owner_user_id"),
        instance_id: row.get("instance_id"),
        generation: row.get("generation"),
        state: InstanceState::parse(&row.get::<String, _>("state")),
        persistence: if row.get::<String, _>("persistence") == "persistent" {
            Persistence::Persistent
        } else {
            Persistence::Ephemeral
        },
        port: row.get("port"),
        pid: row.get("pid"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
        last_activity_at: row.get("last_activity_at"),
        restart_count: row.get("restart_count"),
        exit_code: row.get("exit_code"),
        exit_signal: row.get("exit_signal"),
        failure_message: row.get("failure_message"),
    })
}

impl RuntimeStore {
    #[must_use]
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    // -- global config ---------------------------------------------------

    pub async fn is_enabled(&self, kind: RuntimeKind) -> Result<bool, sqlx::Error> {
        let row: Option<i64> =
            sqlx::query_scalar("SELECT enabled FROM runtime_config WHERE kind = ?")
                .bind(kind.as_str())
                .fetch_optional(&self.pool)
                .await?;
        Ok(row.unwrap_or(0) != 0)
    }

    pub async fn set_enabled(&self, kind: RuntimeKind, enabled: bool) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO runtime_config (kind, enabled, updated_at) VALUES (?, ?, ?)
             ON CONFLICT (kind) DO UPDATE SET enabled = excluded.enabled, updated_at = excluded.updated_at",
        )
        .bind(kind.as_str())
        .bind(i64::from(enabled))
        .bind(now())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    // -- instances --------------------------------------------------------

    #[allow(clippy::too_many_arguments)]
    pub async fn upsert_instance(
        &self,
        id: &InstanceId,
        generation: u64,
        state: InstanceState,
        persistence: Persistence,
        port: Option<u16>,
        pid: Option<u32>,
    ) -> Result<(), sqlx::Error> {
        let ts = now();
        sqlx::query(
            "INSERT INTO runtime_instances (
                kind, owner_user_id, instance_id, generation, state, persistence, port, pid,
                created_at, updated_at, last_activity_at, restart_count
             ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 0)
             ON CONFLICT (kind, owner_user_id, instance_id) DO UPDATE SET
                generation = excluded.generation,
                state = excluded.state,
                persistence = excluded.persistence,
                port = excluded.port,
                pid = excluded.pid,
                updated_at = excluded.updated_at",
        )
        .bind(id.kind.as_str())
        .bind(&id.owner_user_id)
        .bind(&id.instance_id)
        .bind(i64::try_from(generation).unwrap_or(i64::MAX))
        .bind(state.as_str())
        .bind(match persistence {
            Persistence::Persistent => "persistent",
            Persistence::Ephemeral => "ephemeral",
        })
        .bind(port.map(i64::from))
        .bind(pid.map(i64::from))
        .bind(ts)
        .bind(ts)
        .bind(ts)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn set_state(
        &self,
        id: &InstanceId,
        state: InstanceState,
        failure_message: Option<&str>,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "UPDATE runtime_instances SET state = ?, failure_message = ?, updated_at = ?
             WHERE kind = ? AND owner_user_id = ? AND instance_id = ?",
        )
        .bind(state.as_str())
        .bind(failure_message)
        .bind(now())
        .bind(id.kind.as_str())
        .bind(&id.owner_user_id)
        .bind(&id.instance_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn record_exit(
        &self,
        id: &InstanceId,
        exit_code: Option<i32>,
        exit_signal: Option<&str>,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "UPDATE runtime_instances SET exit_code = ?, exit_signal = ?, updated_at = ?
             WHERE kind = ? AND owner_user_id = ? AND instance_id = ?",
        )
        .bind(exit_code.map(i64::from))
        .bind(exit_signal)
        .bind(now())
        .bind(id.kind.as_str())
        .bind(&id.owner_user_id)
        .bind(&id.instance_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn touch_activity(&self, id: &InstanceId) -> Result<(), sqlx::Error> {
        sqlx::query(
            "UPDATE runtime_instances SET last_activity_at = ?
             WHERE kind = ? AND owner_user_id = ? AND instance_id = ?",
        )
        .bind(now())
        .bind(id.kind.as_str())
        .bind(&id.owner_user_id)
        .bind(&id.instance_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn increment_restart_count(&self, id: &InstanceId) -> Result<i64, sqlx::Error> {
        sqlx::query(
            "UPDATE runtime_instances SET restart_count = restart_count + 1, updated_at = ?
             WHERE kind = ? AND owner_user_id = ? AND instance_id = ?",
        )
        .bind(now())
        .bind(id.kind.as_str())
        .bind(&id.owner_user_id)
        .bind(&id.instance_id)
        .execute(&self.pool)
        .await?;
        self.get(id)
            .await
            .map(|row| row.map_or(0, |r| r.restart_count))
    }

    pub async fn get(&self, id: &InstanceId) -> Result<Option<InstanceRow>, sqlx::Error> {
        let row = sqlx::query(
            "SELECT * FROM runtime_instances WHERE kind = ? AND owner_user_id = ? AND instance_id = ?",
        )
        .bind(id.kind.as_str())
        .bind(&id.owner_user_id)
        .bind(&id.instance_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.and_then(|row| row_to_instance(&row)))
    }

    /// Ownership-scoped: only ever returns `owner_user_id`'s own rows.
    pub async fn list_for_owner(
        &self,
        owner_user_id: &str,
    ) -> Result<Vec<InstanceRow>, sqlx::Error> {
        let rows = sqlx::query(
            "SELECT * FROM runtime_instances WHERE owner_user_id = ? ORDER BY created_at",
        )
        .bind(owner_user_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.iter().filter_map(row_to_instance).collect())
    }

    /// Unscoped -- for startup reconciliation only, never exposed
    /// through an HTTP handler.
    pub async fn list_all(&self) -> Result<Vec<InstanceRow>, sqlx::Error> {
        let rows = sqlx::query("SELECT * FROM runtime_instances")
            .fetch_all(&self.pool)
            .await?;
        Ok(rows.iter().filter_map(row_to_instance).collect())
    }

    pub async fn delete(&self, id: &InstanceId) -> Result<(), sqlx::Error> {
        sqlx::query(
            "DELETE FROM runtime_instances WHERE kind = ? AND owner_user_id = ? AND instance_id = ?",
        )
        .bind(id.kind.as_str())
        .bind(&id.owner_user_id)
        .bind(&id.instance_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}
