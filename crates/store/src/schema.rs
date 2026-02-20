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
        } else {
            tracing::debug!(version = *version, "Migration already applied, skipping");
        }
    }

    Ok(())
}
