//! Database connection initialization for the Claude history SQLite store.
//!
//! Provides `init_db` which opens a connection, configures WAL mode and pragmas,
//! runs pending migrations, and returns a tokio-rusqlite async connection handle.

use std::path::Path;
use std::time::Duration;

use crate::schema;

/// Errors that can occur during database initialization.
#[derive(Debug, thiserror::Error)]
pub enum DbError {
    #[error("SQLite connection error: {0}")]
    Sqlite(#[from] rusqlite::Error),

    #[error("Async connection error: {0}")]
    Async(#[from] tokio_rusqlite::Error),

    #[error("Schema migration error: {0}")]
    Schema(#[from] schema::SchemaError),

    #[error("Failed to initialize database: {0}")]
    Init(String),
}

/// Open and initialize the SQLite database at the given path.
///
/// This function:
/// 1. Creates parent directories if they do not exist
/// 2. Opens a tokio-rusqlite async connection
/// 3. Sets WAL journal mode for concurrent read capability
/// 4. Configures busy_timeout to 5 seconds to handle lock contention
/// 5. Sets synchronous = NORMAL (safe with WAL, avoids FULL overhead)
/// 6. Enables foreign key enforcement
/// 7. Runs all pending schema migrations
///
/// The returned connection is ready for use by the ingestion pipeline.
pub async fn init_db(path: &Path) -> Result<tokio_rusqlite::Connection, DbError> {
    // Create parent directories so the database file can be created
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            DbError::Init(format!(
                "Could not create parent directories for {}: {}",
                path.display(),
                e
            ))
        })?;
    }

    let conn = tokio_rusqlite::Connection::open(path)
        .await
        .map_err(DbError::Sqlite)?;

    conn.call(|conn| {
        // WAL mode allows concurrent reads while writing
        conn.pragma_update(None, "journal_mode", "WAL")?;

        // 5-second busy timeout to wait for locks rather than failing immediately
        conn.busy_timeout(Duration::from_secs(5))?;

        // NORMAL synchronous is safe with WAL and avoids the overhead of FULL
        conn.pragma_update(None, "synchronous", "NORMAL")?;

        // Enforce foreign key constraints
        conn.pragma_update(None, "foreign_keys", "ON")?;

        // Apply schema migrations
        schema::run_migrations(conn).map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;

        Ok(())
    })
    .await
    .map_err(|e| match e {
        tokio_rusqlite::Error::Error(re) => DbError::Sqlite(re),
        tokio_rusqlite::Error::ConnectionClosed => {
            DbError::Init("Connection closed during initialization".to_string())
        }
        tokio_rusqlite::Error::Close(_) => {
            DbError::Init("Connection close error during initialization".to_string())
        }
        _ => {
            DbError::Init(format!("Unexpected tokio-rusqlite error during initialization"))
        }
    })?;

    Ok(conn)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[tokio::test]
    async fn test_init_db_creates_schema_and_sets_pragmas() {
        // Use a temporary file so tests are isolated
        let tmp_dir = std::env::temp_dir().join("claude-history-test");
        let _ = std::fs::create_dir_all(&tmp_dir);
        let db_path = tmp_dir.join(format!("test-{}.db", std::process::id()));

        // Clean up any leftover file from a previous run
        let _ = std::fs::remove_file(&db_path);

        let conn = init_db(&db_path).await.expect("init_db should succeed");

        conn.call(|conn| {
            // --- Verify WAL mode ---
            let journal_mode: String = conn
                .pragma_query_value(None, "journal_mode", |row| row.get(0))
                .expect("should query journal_mode");
            assert_eq!(
                journal_mode.to_lowercase(),
                "wal",
                "Journal mode should be WAL"
            );

            // --- Verify foreign keys are enabled ---
            let fk: i64 = conn
                .pragma_query_value(None, "foreign_keys", |row| row.get(0))
                .expect("should query foreign_keys");
            assert_eq!(fk, 1, "Foreign keys should be enabled");

            // --- Verify migration 001 was recorded ---
            let version: String = conn
                .query_row(
                    "SELECT version FROM schema_versions WHERE version = '001'",
                    [],
                    |row| row.get(0),
                )
                .expect("schema_versions should contain version 001");
            assert_eq!(version, "001");

            // --- Verify all 13 expected tables exist ---
            let mut tables: HashSet<String> = HashSet::new();
            let mut stmt = conn
                .prepare("SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%'")
                .expect("should prepare query");
            let rows = stmt
                .query_map([], |row| row.get::<_, String>(0))
                .expect("should query tables");
            for row in rows {
                tables.insert(row.expect("should read table name"));
            }

            let expected_tables = [
                "sessions",
                "messages",
                "message_content",
                "token_usage",
                "tool_executions",
                "agents",
                "queue_operations",
                "progress_events",
                "system_events",
                "summaries",
                "sync_metadata",
                "schema_versions",
                "schema_drift_log",
            ];

            for table in &expected_tables {
                assert!(
                    tables.contains(*table),
                    "Expected table '{}' to exist, found tables: {:?}",
                    table,
                    tables
                );
            }

            // Confirm we have at least 13 tables (schema_versions is created
            // by run_migrations bootstrap, the rest by 001_initial.sql)
            assert!(
                tables.len() >= 13,
                "Expected at least 13 tables, found {}: {:?}",
                tables.len(),
                tables
            );

            // --- Verify indexes exist ---
            let mut indexes: HashSet<String> = HashSet::new();
            let mut stmt = conn
                .prepare("SELECT name FROM sqlite_master WHERE type = 'index' AND name LIKE 'idx_%'")
                .expect("should prepare index query");
            let rows = stmt
                .query_map([], |row| row.get::<_, String>(0))
                .expect("should query indexes");
            for row in rows {
                indexes.insert(row.expect("should read index name"));
            }

            let expected_indexes = [
                "idx_messages_session_id",
                "idx_messages_timestamp",
                "idx_message_content_message_uuid",
                "idx_tool_executions_message_uuid",
                "idx_tool_executions_tool_name",
                "idx_progress_events_session_id",
                "idx_progress_events_data_type",
                "idx_system_events_session_id",
                "idx_system_events_subtype",
                "idx_queue_operations_session_id",
            ];

            for idx in &expected_indexes {
                assert!(
                    indexes.contains(*idx),
                    "Expected index '{}' to exist, found indexes: {:?}",
                    idx,
                    indexes
                );
            }

            Ok::<(), rusqlite::Error>(())
        })
        .await
        .expect("verification queries should succeed");

        // --- Verify idempotency: calling init_db again should not fail ---
        let conn2 = init_db(&db_path).await.expect("second init_db should succeed (idempotent)");

        conn2.call(|conn| {
            // Verify still exactly one version row
            let count: i64 = conn
                .query_row("SELECT COUNT(*) FROM schema_versions", [], |row| row.get(0))
                .expect("should count schema_versions");
            assert_eq!(count, 2, "Should still have exactly 2 migration versions (001+002) after second init");
            Ok::<(), rusqlite::Error>(())
        })
        .await
        .expect("idempotency check should succeed");

        // Clean up
        let _ = std::fs::remove_file(&db_path);
        let _ = std::fs::remove_file(db_path.with_extension("db-wal"));
        let _ = std::fs::remove_file(db_path.with_extension("db-shm"));
    }
}
