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
    ("008", include_str!("../migrations/008_attachments.sql")),
    ("009", include_str!("../migrations/009_attachment_fts.sql")),
    ("010", include_str!("../migrations/010_plan_content.sql")),
    ("011", include_str!("../migrations/011_plan_content_fts.sql")),
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
        // schema_versions should have 11 entries (one per migration after C2.3)
        assert_eq!(
            sv_count, 11,
            "schema_versions should track all 11 applied migrations"
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

    // -----------------------------------------------------------------------
    // Migration 008: attachments + hook_executions tables (C1.1 structural
    // foundation; decomposer routing lands in C1.2).
    // -----------------------------------------------------------------------

    #[test]
    fn migration_008_creates_attachments_table() {
        let conn = migrated_conn();
        let exists: bool = conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type = 'table' AND name = 'attachments'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(exists, "attachments table should exist after migration 008");
    }

    #[test]
    fn migration_008_creates_hook_executions_table() {
        let conn = migrated_conn();
        let exists: bool = conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type = 'table' AND name = 'hook_executions'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(
            exists,
            "hook_executions table should exist after migration 008"
        );
    }

    #[test]
    fn migration_008_attachments_columns_queryable() {
        let conn = migrated_conn();
        conn.execute_batch(
            "SELECT uuid, session_id, parent_uuid, timestamp, cwd, version,
                    git_branch, slug, entrypoint, inner_type, body_json
             FROM attachments LIMIT 0",
        )
        .expect("attachments should have all expected columns after migration 008");
    }

    #[test]
    fn migration_008_hook_executions_columns_queryable() {
        let conn = migrated_conn();
        conn.execute_batch(
            "SELECT id, attachment_uuid, hook_name, hook_event, tool_use_id,
                    exit_code, duration_ms, stdout, stderr, command, decision
             FROM hook_executions LIMIT 0",
        )
        .expect("hook_executions should have all expected columns after migration 008");
    }

    #[test]
    fn migration_008_attachments_indices_present() {
        let conn = migrated_conn();
        let mut stmt = conn
            .prepare("SELECT name FROM sqlite_master WHERE type = 'index' AND tbl_name = 'attachments'")
            .unwrap();
        let names: Vec<String> = stmt
            .query_map([], |row| row.get(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        for expected in [
            "idx_attachments_session_id",
            "idx_attachments_inner_type",
            "idx_attachments_timestamp",
        ] {
            assert!(
                names.contains(&expected.to_string()),
                "missing index {expected}; have {names:?}"
            );
        }
    }

    #[test]
    fn migration_008_hook_executions_indices_present() {
        let conn = migrated_conn();
        let mut stmt = conn
            .prepare("SELECT name FROM sqlite_master WHERE type = 'index' AND tbl_name = 'hook_executions'")
            .unwrap();
        let names: Vec<String> = stmt
            .query_map([], |row| row.get(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        for expected in [
            "idx_hook_executions_tool_use_id",
            "idx_hook_executions_attachment_uuid",
            "idx_hook_executions_hook_event",
        ] {
            assert!(
                names.contains(&expected.to_string()),
                "missing index {expected}; have {names:?}"
            );
        }
    }

    #[test]
    fn migration_008_idempotent() {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).unwrap();
        run_migrations(&conn).unwrap();

        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM schema_versions WHERE version = '008'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1, "migration 008 should be recorded exactly once");
    }

    /// Integrity check after all migrations, verifying that schema_versions
    /// itself reflects the new count and that PRAGMA integrity_check still
    /// returns "ok". Replaces the implicit "should still have exactly 7"
    /// expectation that B1.1 set after migration 007 with the C1.1-era count.
    #[test]
    fn migration_008_post_apply_integrity() {
        let conn = migrated_conn();
        let sv_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM schema_versions", [], |row| row.get(0))
            .unwrap();
        assert_eq!(sv_count, 11, "all 11 migrations should be recorded post-011");

        let integrity: String = conn
            .query_row("PRAGMA integrity_check", [], |row| row.get(0))
            .unwrap();
        assert_eq!(
            integrity, "ok",
            "PRAGMA integrity_check should return 'ok' after all migrations"
        );
    }

    // -----------------------------------------------------------------------
    // Migration 009: fts_attachment_text_content FTS5 virtual table for
    // textual payloads from four AttachmentBody subtypes (C1.3).
    // -----------------------------------------------------------------------

    #[test]
    fn migration_009_creates_fts_attachment_text_content_table() {
        let conn = migrated_conn();
        // FTS5 virtual tables register themselves in sqlite_master with
        // type='table' and an "USING fts5(...)" sql expression.
        let exists: bool = conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM sqlite_master
                 WHERE type = 'table' AND name = 'fts_attachment_text_content'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(
            exists,
            "fts_attachment_text_content virtual table should exist after migration 009"
        );
    }

    #[test]
    fn migration_009_columns_queryable() {
        let conn = migrated_conn();
        // The 4 columns: attachment_uuid, session_id, inner_type, text_content.
        // FTS5 selectability is the operational test that the schema is intact.
        conn.execute_batch(
            "SELECT attachment_uuid, session_id, inner_type, text_content
             FROM fts_attachment_text_content LIMIT 0",
        )
        .expect(
            "fts_attachment_text_content should have all expected columns after migration 009",
        );
    }

    #[test]
    fn migration_009_idempotent() {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).unwrap();
        run_migrations(&conn).unwrap();

        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM schema_versions WHERE version = '009'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1, "migration 009 should be recorded exactly once");
    }

    // -----------------------------------------------------------------------
    // Migration 010: messages.plan_content TEXT column + idempotent backfill
    // from extra_json + idx_messages_plan_content_present partial index (C2.1).
    // -----------------------------------------------------------------------

    #[test]
    fn migration_010_adds_plan_content_column() {
        let conn = migrated_conn();
        // Column-shape check: SELECT against the new column should succeed
        // post-migration. The column is nullable TEXT.
        conn.execute_batch("SELECT plan_content FROM messages LIMIT 0")
            .expect("messages should have plan_content column after migration 010");
    }

    #[test]
    fn migration_010_creates_partial_index() {
        let conn = migrated_conn();
        let exists: bool = conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM sqlite_master
                 WHERE type = 'index' AND name = 'idx_messages_plan_content_present'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(
            exists,
            "idx_messages_plan_content_present index should exist after migration 010"
        );
    }

    #[test]
    fn migration_010_partial_index_predicate_filters_nulls() {
        // Verify the partial-index WHERE clause is captured in sqlite_master.sql
        // (SQLite stores the original DDL for indexes including the partial
        // predicate; checking the substring guards against accidental loss
        // of the partial-index property in future migration edits).
        let conn = migrated_conn();
        let sql: String = conn
            .query_row(
                "SELECT sql FROM sqlite_master
                 WHERE type = 'index' AND name = 'idx_messages_plan_content_present'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(
            sql.contains("plan_content IS NOT NULL"),
            "partial index predicate should reference plan_content IS NOT NULL; got: {sql}"
        );
    }

    #[test]
    fn migration_010_idempotent() {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).unwrap();
        run_migrations(&conn).unwrap();

        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM schema_versions WHERE version = '010'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1, "migration 010 should be recorded exactly once");
    }

    #[test]
    fn migration_010_backfills_plan_content_from_extra_json() {
        // The migration-runner-level idempotency guard is independently
        // covered by migration_010_idempotent. This test exercises the
        // backfill + cleanup UPDATE statements directly: it seeds a row
        // carrying $.planContent in extra_json BEFORE migration 010 has
        // run, then applies migration 010's bare SQL (the same SQL the
        // runner applies during migration 010 application).
        //
        // Approach: replay migrations 001..009 manually (without the
        // runner's schema_versions guard), seed a row, then apply 010's
        // SQL directly.
        let conn = Connection::open_in_memory().unwrap();
        // Predicate-based slice rather than .take(9): if a future migration
        // is inserted at position <010, .take(9) would silently apply the
        // wrong subset. take_while compares the version string lexically,
        // which is correct for the zero-padded "001".."010" naming scheme
        // and stays correct as new pre-010 migrations would be impossible
        // (010 is already shipped) — the predicate documents the boundary.
        // [C2.1.1 / audit #45]
        for (_version, sql) in MIGRATIONS.iter().take_while(|(v, _)| *v < "010") {
            conn.execute_batch(sql).unwrap();
        }

        // Seed a session + message row carrying $.planContent in extra_json.
        conn.execute(
            "INSERT INTO sessions (session_id, project_path, first_seen_at)
             VALUES ('sess-plan', '/test/project', '2026-05-09T00:00:00Z')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO messages (uuid, session_id, type, timestamp, extra_json)
             VALUES ('msg-plan', 'sess-plan', 'user', '2026-05-09T00:00:00Z',
                     '{\"planContent\":\"# Plan\\n\\nDo the thing.\",\"otherField\":42}')",
            [],
        )
        .unwrap();

        // Now apply migration 010 (the ALTER + two UPDATEs + index).
        let migration_010 = include_str!("../migrations/010_plan_content.sql");
        conn.execute_batch(migration_010).unwrap();

        // Backfill assertion: plan_content column populated from extra_json.
        let plan_content: Option<String> = conn
            .query_row(
                "SELECT plan_content FROM messages WHERE uuid = 'msg-plan'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            plan_content.as_deref(),
            Some("# Plan\n\nDo the thing."),
            "backfill UPDATE should copy $.planContent into plan_content column"
        );

        // Cleanup assertion: planContent removed from extra_json, otherField
        // preserved. json_remove returns the residual JSON object.
        let extra_json: String = conn
            .query_row(
                "SELECT extra_json FROM messages WHERE uuid = 'msg-plan'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&extra_json).unwrap();
        assert!(
            parsed.get("planContent").is_none(),
            "cleanup UPDATE should strip $.planContent from extra_json; got {parsed}"
        );
        assert_eq!(
            parsed.get("otherField"),
            Some(&serde_json::json!(42)),
            "cleanup UPDATE should preserve sibling extra_json keys"
        );

        // Idempotent-replay assertion: re-running the cleanup UPDATE leaves
        // both columns unchanged because the WHERE clause no longer matches.
        conn.execute_batch(
            "UPDATE messages
             SET extra_json = json_remove(extra_json, '$.planContent')
             WHERE json_extract(extra_json, '$.planContent') IS NOT NULL",
        )
        .unwrap();
        let extra_json_2: String = conn
            .query_row(
                "SELECT extra_json FROM messages WHERE uuid = 'msg-plan'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            extra_json, extra_json_2,
            "cleanup UPDATE should be idempotent on replay"
        );
    }

    // -----------------------------------------------------------------------
    // Migration 011: Synthetic message_content rows for plan_content FTS5
    // coverage. Backfill inserts (message_uuid, -1, 'plan_content',
    // plan_content) for every messages row with non-NULL plan_content (C2.3).
    // -----------------------------------------------------------------------

    #[test]
    fn migration_011_idempotent() {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).unwrap();
        run_migrations(&conn).unwrap();

        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM schema_versions WHERE version = '011'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1, "migration 011 should be recorded exactly once");
    }

    #[test]
    fn migration_011_backfills_synthetic_message_content_rows() {
        // The migration runner's idempotency is independently covered by
        // migration_011_idempotent. This test exercises the backfill INSERT
        // OR IGNORE statement directly: it seeds messages rows with
        // plan_content BEFORE migration 011 has run, then applies 011's
        // bare SQL (the same SQL the runner applies during 011 application).
        //
        // Approach: replay migrations 001..010 manually (without the
        // runner's schema_versions guard), seed plan-bearing rows, then
        // apply 011's SQL directly. Mirrors the migration_010_backfill...
        // test's predicate-based take_while bound for stability against
        // future inserted-mid-sequence migrations.
        let conn = Connection::open_in_memory().unwrap();
        for (_version, sql) in MIGRATIONS.iter().take_while(|(v, _)| *v < "011") {
            conn.execute_batch(sql).unwrap();
        }

        // Seed two sessions + three messages (two with plan_content, one
        // without) so the backfill has both target and non-target rows.
        conn.execute(
            "INSERT INTO sessions (session_id, project_path, first_seen_at)
             VALUES ('sess-a', '/p/a', '2026-05-09T00:00:00Z')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO messages (uuid, session_id, type, timestamp, plan_content)
             VALUES ('msg-1', 'sess-a', 'user', '2026-05-09T00:00:00Z',
                     '# Plan A\n\n- step one')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO messages (uuid, session_id, type, timestamp, plan_content)
             VALUES ('msg-2', 'sess-a', 'user', '2026-05-09T00:00:01Z',
                     '# Plan B\n\nfinal')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO messages (uuid, session_id, type, timestamp)
             VALUES ('msg-3', 'sess-a', 'user', '2026-05-09T00:00:02Z')",
            [],
        )
        .unwrap();

        // Apply migration 011 (the INSERT OR IGNORE backfill).
        let migration_011 = include_str!("../migrations/011_plan_content_fts.sql");
        conn.execute_batch(migration_011).unwrap();

        // Backfill assertion: exactly two synthetic message_content rows,
        // one per plan-bearing message, both at block_index = -1 with
        // block_type = 'plan_content'.
        let synth_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM message_content
                 WHERE block_index = -1 AND block_type = 'plan_content'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            synth_count, 2,
            "expected one synthetic row per plan-bearing message; got {synth_count}"
        );

        // Per-row text_content matches the source plan_content value.
        let plan_a: String = conn
            .query_row(
                "SELECT text_content FROM message_content
                 WHERE message_uuid = 'msg-1' AND block_index = -1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(plan_a, "# Plan A\n\n- step one");

        let plan_b: String = conn
            .query_row(
                "SELECT text_content FROM message_content
                 WHERE message_uuid = 'msg-2' AND block_index = -1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(plan_b, "# Plan B\n\nfinal");

        // No synthetic row for the no-plan message.
        let none_for_msg_3: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM message_content
                 WHERE message_uuid = 'msg-3'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            none_for_msg_3, 0,
            "no synthetic row should be inserted for messages without plan_content"
        );

        // Idempotent-replay assertion: re-running the backfill leaves the
        // count unchanged because UNIQUE(message_uuid, block_index) absorbs
        // the duplicate via INSERT OR IGNORE.
        conn.execute_batch(migration_011).unwrap();
        let synth_count_after_replay: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM message_content
                 WHERE block_index = -1 AND block_type = 'plan_content'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            synth_count_after_replay, 2,
            "INSERT OR IGNORE should keep the synthetic row count stable on replay; got {synth_count_after_replay}"
        );
    }

    #[test]
    fn migration_011_synthetic_row_visible_to_fts_after_rebuild() {
        // End-to-end: backfill synthetic rows, rebuild fts_message_content,
        // confirm a phrase from the seeded plan content matches via FTS5.
        // This is the load-bearing search-gate that C2.3 promises.
        let conn = migrated_conn();

        conn.execute(
            "INSERT INTO sessions (session_id, project_path, first_seen_at)
             VALUES ('sess-fts', '/p/fts', '2026-05-09T00:00:00Z')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO messages (uuid, session_id, type, timestamp, plan_content)
             VALUES ('msg-fts', 'sess-fts', 'user', '2026-05-09T00:00:00Z',
                     '# Plan\n\nimplement zilliqotic-marker subsystem')",
            [],
        )
        .unwrap();

        // Backfill synthetic row directly (the migration's own SELECT/INSERT
        // shape, mirrored here so the test does not depend on re-running
        // the migration after seeding).
        conn.execute(
            "INSERT OR IGNORE INTO message_content
                (message_uuid, block_index, block_type, text_content)
             SELECT uuid, -1, 'plan_content', plan_content
             FROM messages
             WHERE plan_content IS NOT NULL",
            [],
        )
        .unwrap();

        // Rebuild fts_message_content so the synthetic row is tokenized.
        conn.execute_batch(
            "INSERT INTO fts_message_content(fts_message_content) VALUES('rebuild');",
        )
        .unwrap();

        // FTS5 MATCH against a phrase known to be in the synthetic row.
        // The unusual token zilliqotic-marker is unlikely to collide with
        // any other test fixture and is wrapped in double quotes per the
        // existing search-input sanitization convention.
        let hit_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM fts_message_content
                 WHERE fts_message_content MATCH ?1",
                ["\"zilliqotic-marker\""],
                |row| row.get(0),
            )
            .unwrap();
        assert!(
            hit_count >= 1,
            "FTS5 should match the synthetic plan_content phrase; got {hit_count}"
        );
    }
}
