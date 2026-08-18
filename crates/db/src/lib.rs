use std::{str::FromStr, time::Duration};

use sqlx::{
    migrate::{MigrateError, Migrator},
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions},
    SqlitePool,
};

// NOTE: `sqlx::migrate!` reads the `migrations/` directory at this
// macro's expansion time, but cargo has no dependency edge on that
// directory's contents -- only on this crate's own source files. Adding
// or editing a migration `.sql` file alone will NOT trigger a
// recompile, and a stale cached build silently keeps using the old
// migration set. Touch this file (e.g. bump the line below) whenever a
// migration changes so cargo actually re-invokes the macro.
static MIGRATOR: Migrator = sqlx::migrate!("../../migrations");

pub async fn connect(database_url: &str, max_connections: u32) -> Result<SqlitePool, sqlx::Error> {
    let options = SqliteConnectOptions::from_str(database_url)?
        .create_if_missing(true)
        .foreign_keys(true)
        .journal_mode(SqliteJournalMode::Wal)
        .busy_timeout(Duration::from_secs(30));

    SqlitePoolOptions::new()
        .max_connections(max_connections)
        .connect_with(options)
        .await
}

pub async fn migrate(pool: &SqlitePool) -> Result<(), MigrateError> {
    MIGRATOR.run(pool).await
}

#[cfg(test)]
mod tests {
    use sqlx::Row;

    use super::*;

    #[tokio::test]
    async fn migrations_create_the_expected_baseline_and_shell_tables() {
        let pool = connect("sqlite::memory:", 1).await.unwrap();
        migrate(&pool).await.unwrap();

        let row = sqlx::query("SELECT value FROM system_metadata WHERE key = 'schema_baseline'")
            .fetch_one(&pool)
            .await
            .unwrap();

        assert_eq!(row.get::<String, _>("value"), "phase-0");
        let shell_table: String = sqlx::query_scalar(
            "SELECT name FROM sqlite_master WHERE type = 'table' AND name = 'user_preferences'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(shell_table, "user_preferences");
        let vault_table: String = sqlx::query_scalar(
            "SELECT name FROM sqlite_master WHERE type = 'table' AND name = 'vault_secrets'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(vault_table, "vault_secrets");
        let transfer_table: String = sqlx::query_scalar(
            "SELECT name FROM sqlite_master WHERE type = 'table' AND name = 'transfer_jobs'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(transfer_table, "transfer_jobs");
        let throttle_table: String = sqlx::query_scalar(
            "SELECT name FROM sqlite_master
             WHERE type = 'table' AND name = 'login_throttle_buckets'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(throttle_table, "login_throttle_buckets");
        let audit_head: String = sqlx::query_scalar(
            "SELECT name FROM sqlite_master WHERE type = 'table' AND name = 'audit_chain_head'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(audit_head, "audit_chain_head");
        let remote_servers: String = sqlx::query_scalar(
            "SELECT name FROM sqlite_master WHERE type = 'table' AND name = 'remote_servers'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(remote_servers, "remote_servers");
        let envelope_column: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM pragma_table_info('vault_secrets')
             WHERE name = 'encrypted_data_key'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(envelope_column, 1);
    }
}
