//! Schema migration runner for the Claude history SQLite database.
//!
//! Migrations are embedded at compile time via `include_str!` and applied in order.
//! The `schema_versions` table tracks which migrations have been applied, so
//! `run_migrations` is idempotent — safe to call on every connection open.

use rusqlite::Connection;

/// Ordered list of (version_tag, sql_content) pairs.
/// Each migration is applied at most once, tracked by the schema_versions table.
const MIGRATIONS: &[(&str, &str)] = &[
    ("001", include_str!("../migrations/001_initial.sql")),
    ("002", include_str!("../migrations/002_fts5.sql")),
    ("003", include_str!("../migrations/003_artifacts.sql")),
    ("004", include_str!("../migrations/004_modeling.sql")),
    ("005", include_str!("../migrations/005_drop_noise.sql")),
    ("006", include_str!("../migrations/006_version_monitoring.sql")),
    ("007", include_str!("../migrations/007_record_type_drift.sql")),
];

/// Errors that can occur during migration application.
#[derive(Debug, thiserror::Error)]
pub enum SchemaError {
    #[error("SQLite error during migration: {0}")]
    Sqlite(#[from] rusqlite::Error),
}

/// Apply all pending migrations to the given connection.
///
/// Creates the `schema_versions` tracking table if it does not exist, then
/// iterates through MIGRATIONS in order, skipping any already-applied versions.
/// Each unapplied migration runs inside an `unchecked_transaction` so that
/// the DDL and the version-tracking insert are atomic.
pub fn run_migrations(conn: &Connection) -> Result<(), SchemaError> {
    // Bootstrap the version-tracking table. This is idempotent.
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_versions (
            version    TEXT PRIMARY KEY,
            applied_at TEXT NOT NULL DEFAULT (datetime('now'))
        );"
    )?;

    let mut needs_vacuum = false;

    for (version, sql) in MIGRATIONS {
        let already_applied: bool = conn.query_row(
            "SELECT COUNT(*) > 0 FROM schema_versions WHERE version = ?1",
            [version],
            |row| row.get(0),
        )?;

        if !already_applied {
            let tx = conn.unchecked_transaction()?;
            tx.execute_batch(sql)?;
            tx.execute(
                "INSERT INTO schema_versions (version) VALUES (?1)",
                [version],
            )?;
            tx.commit()?;
            tracing::info!(version = *version, "Applied migration");

            // Migration 005 drops large tables — VACUUM to reclaim disk space.
            // VACUUM cannot run inside a transaction, so we defer it.
            if *version == "005" {
                needs_vacuum = true;
            }
        } else {
            tracing::debug!(version = *version, "Migration already applied, skipping");
        }
    }

    if needs_vacuum {
        tracing::info!("Running VACUUM to reclaim space from dropped tables");
        conn.execute_batch("VACUUM;")?;
        tracing::info!("VACUUM complete");
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    /// Helper: create an in-memory database and run all migrations through it.
    fn migrated_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).unwrap();
        conn
    }

    #[test]
    fn migration_004_creates_projects_table() {
        let conn = migrated_conn();
        let exists: bool = conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type = 'table' AND name = 'projects'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(exists, "projects table should exist after migration 004");
    }

    #[test]
    fn migration_004_adds_file_operations_columns() {
        let conn = migrated_conn();
        // Verify result_summary and is_error columns are queryable on file_operations
        conn.execute_batch("SELECT result_summary, is_error FROM file_operations LIMIT 0")
            .expect("file_operations should have result_summary and is_error columns");
    }

    #[test]
    fn migration_004_adds_git_operations_columns() {
        let conn = migrated_conn();
        // Verify result_summary and is_error columns are queryable on git_operations
        conn.execute_batch("SELECT result_summary, is_error FROM git_operations LIMIT 0")
            .expect("git_operations should have result_summary and is_error columns");
    }

    #[test]
    fn migration_004_creates_all_seven_views() {
        let conn = migrated_conn();
        let expected_views = [
            "v_file_token_cost",
            "v_file_conversation_context",
            "v_project_summary",
            "v_file_provenance",
            "v_git_commit_context",
            "v_tool_errors",
            "v_session_cost",
        ];

        let mut stmt = conn
            .prepare("SELECT name FROM sqlite_master WHERE type = 'view'")
            .unwrap();
        let actual_views: Vec<String> = stmt
            .query_map([], |row| row.get(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();

        for view_name in &expected_views {
            assert!(
                actual_views.contains(&view_name.to_string()),
                "view {view_name} should exist in sqlite_master; found: {actual_views:?}"
            );
        }
    }

    #[test]
    fn migration_004_views_are_queryable() {
        let conn = migrated_conn();
        let views = [
            "v_file_token_cost",
            "v_file_conversation_context",
            "v_project_summary",
            "v_file_provenance",
            "v_git_commit_context",
            "v_tool_errors",
            "v_session_cost",
        ];

        for view_name in &views {
            conn.execute_batch(&format!("SELECT * FROM {view_name} LIMIT 0"))
                .unwrap_or_else(|e| panic!("view {view_name} should be queryable: {e}"));
        }
    }

    #[test]
    fn run_migrations_is_idempotent() {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).unwrap();
        // Running again should succeed without errors
        run_migrations(&conn).unwrap();

        // Verify migration 004 is recorded exactly once
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM schema_versions WHERE version = '004'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1, "migration 004 should be recorded exactly once");
    }

    #[test]
    fn migration_006_creates_version_history_table() {
        let conn = migrated_conn();
        let exists: bool = conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type = 'table' AND name = 'version_history'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(exists, "version_history table should exist after migration 006");
    }

    #[test]
    fn migration_006_adds_messages_columns() {
        let conn = migrated_conn();
        conn.execute_batch(
            "SELECT is_compact_summary, source_tool_use_id, extra_json FROM messages LIMIT 0",
        )
        .expect("messages should have is_compact_summary, source_tool_use_id, extra_json columns");
    }

    #[test]
    fn migration_006_adds_schema_drift_log_columns() {
        let conn = migrated_conn();
        conn.execute_batch(
            "SELECT occurrence_count, last_seen_at FROM schema_drift_log LIMIT 0",
        )
        .expect("schema_drift_log should have occurrence_count and last_seen_at columns");
    }

    #[test]
    fn migration_006_views_are_queryable() {
        let conn = migrated_conn();
        let views = [
            "v_file_token_cost",
            "v_file_conversation_context",
            "v_project_summary",
            "v_file_provenance",
            "v_git_commit_context",
            "v_tool_errors",
            "v_session_cost",
        ];

        for view_name in &views {
            conn.execute_batch(&format!("SELECT * FROM {view_name} LIMIT 0"))
                .unwrap_or_else(|e| {
                    panic!("view {view_name} should be queryable after migration 006: {e}")
                });
        }
    }

    #[test]
    fn migration_006_idempotent() {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).unwrap();
        run_migrations(&conn).unwrap();

        // Verify migration 006 is recorded exactly once in schema_versions
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM schema_versions WHERE version = '006'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1, "migration 006 should be recorded exactly once");
    }

    #[test]
    fn migration_006_version_history_not_schema_versions() {
        let conn = migrated_conn();
        // version_history should be independently queryable and not conflict
        // with the schema_versions migration tracker table
        let vh_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM version_history", [], |row| row.get(0))
            .unwrap();
        let sv_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM schema_versions", [], |row| row.get(0))
            .unwrap();
        // version_history is empty on a fresh in-memory DB (no sessions to backfill from)
        assert_eq!(vh_count, 0, "version_history should be empty on fresh DB");
        // schema_versions should have 7 entries (one per migration)
        assert_eq!(
            sv_count, 7,
            "schema_versions should track all 7 applied migrations"
        );
    }

    // -----------------------------------------------------------------------
    // Migration 007: record_type_drift_log table for variant-level drift
    // -----------------------------------------------------------------------

    #[test]
    fn migration_007_creates_record_type_drift_log_table() {
        let conn = migrated_conn();
        let exists: bool = conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type = 'table' AND name = 'record_type_drift_log'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(
            exists,
            "record_type_drift_log table should exist after migration 007"
        );
    }

    #[test]
    fn migration_007_columns_queryable() {
        let conn = migrated_conn();
        conn.execute_batch(
            "SELECT id, type_name, version, sample_value, first_seen_at,
                    last_seen_at, occurrence_count
             FROM record_type_drift_log LIMIT 0",
        )
        .expect(
            "record_type_drift_log should have all expected columns after migration 007",
        );
    }

    #[test]
    fn migration_007_unique_constraint_on_type_name_version() {
        let conn = migrated_conn();
        // Two distinct (type_name, version) pairs should both insert.
        conn.execute(
            "INSERT INTO record_type_drift_log (type_name, version, sample_value)
             VALUES ('attachment', '2.1.126', '{}')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO record_type_drift_log (type_name, version, sample_value)
             VALUES ('attachment', '2.1.91', '{}')",
            [],
        )
        .unwrap();
        // Re-inserting the same (type_name, version) should fail without
        // ON CONFLICT — proving the UNIQUE constraint is in place.
        let dup_result = conn.execute(
            "INSERT INTO record_type_drift_log (type_name, version, sample_value)
             VALUES ('attachment', '2.1.126', '{}')",
            [],
        );
        assert!(
            dup_result.is_err(),
            "duplicate (type_name, version) should be rejected by UNIQUE constraint"
        );
    }

    #[test]
    fn migration_007_idempotent() {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).unwrap();
        run_migrations(&conn).unwrap();

        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM schema_versions WHERE version = '007'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1, "migration 007 should be recorded exactly once");
    }
}
