//! Schema drift logger for overflow fields.
//!
//! When serde(flatten) captures unknown fields in a HashMap<String, Value>,
//! those fields represent potential schema evolution in Claude Code's JSONL
//! format. This module logs each unique (field_name, record_type, version)
//! combination to the `schema_drift_log` table with a truncated sample value.
//!
//! The UNIQUE(field_name, record_type, version) constraint in the DDL combined
//! with INSERT ... ON CONFLICT DO UPDATE enables occurrence counting: each
//! re-observation increments `occurrence_count` and refreshes `last_seen_at`.
//! This keeps the drift log compact (one row per unique combination) while
//! also tracking frequency of each overflow field.
//!
//! [DECOMP-05, STORE-04]

use std::collections::HashMap;

use claude_history_core::record::JSONLRecord;
use rusqlite::Transaction;

/// Maximum length for sample_value stored in schema_drift_log.
/// Values longer than this are truncated to keep the table manageable.
const MAX_SAMPLE_VALUE_LEN: usize = 500;

/// Log overflow fields from a single HashMap to the schema_drift_log table.
///
/// For each (field_name, value) pair in the overflow map:
/// - Serializes the value to a string
/// - Truncates to MAX_SAMPLE_VALUE_LEN characters
/// - Inserts via INSERT ... ON CONFLICT DO UPDATE, which increments
///   `occurrence_count` and refreshes `last_seen_at` on re-observation
///
/// Returns the number of fields observed (both new inserts and re-observations).
/// This count includes updates to existing rows, since ON CONFLICT DO UPDATE
/// returns 1 for both inserts and updates. The return value is used for
/// SchemaDrift SSE event emission and debug logging.
pub fn log_overflow(
    version: &str,
    record_type: &str,
    overflow: &HashMap<String, serde_json::Value>,
    tx: &Transaction,
) -> Result<usize, rusqlite::Error> {
    if overflow.is_empty() {
        return Ok(0);
    }

    let mut new_entries = 0;

    for (field_name, value) in overflow {
        let sample = value.to_string();
        let truncated = if sample.len() > MAX_SAMPLE_VALUE_LEN {
            format!("{}...", &sample[..MAX_SAMPLE_VALUE_LEN])
        } else {
            sample
        };

        let source_context = format!(
            "overflow capture from {} record v{}",
            record_type, version
        );

        let changed = tx.execute(
            "INSERT INTO schema_drift_log
             (field_name, record_type, version, sample_value, source_context, occurrence_count, last_seen_at)
             VALUES (?1, ?2, ?3, ?4, ?5, 1, datetime('now'))
             ON CONFLICT(field_name, record_type, version) DO UPDATE SET
               occurrence_count = schema_drift_log.occurrence_count + 1,
               last_seen_at = datetime('now')",
            rusqlite::params![field_name, record_type, version, truncated, source_context],
        )?;

        if changed > 0 {
            tracing::debug!(
                field_name = field_name,
                record_type = record_type,
                version = version,
                "Schema drift field observed"
            );
            new_entries += changed;
        }
    }

    if new_entries > 0 {
        tracing::info!(
            count = new_entries,
            record_type = record_type,
            version = version,
            "Observed {} schema drift field(s)",
            new_entries
        );
    }

    Ok(new_entries)
}

/// Convenience wrapper that extracts version, record_type, and all overflow
/// maps from a parsed JSONLRecord and calls log_overflow for each.
///
/// For records with multiple overflow maps (e.g., AssistantRecord has
/// record-level overflow, AssistantMessage overflow, and UsageStats overflow),
/// each map is logged with a descriptive qualified record_type:
/// - "assistant" for outer record overflow
/// - "assistant.message" for inner message overflow
/// - "assistant.message.usage" for usage stats overflow
///
/// Returns the total number of new drift entries logged across all maps.
pub fn log_record_overflow(
    record: &JSONLRecord,
    tx: &Transaction,
) -> Result<usize, rusqlite::Error> {
    match record {
        JSONLRecord::User(r) => {
            log_overflow(&r.base.version, "user", &r.overflow, tx)
        }

        JSONLRecord::Assistant(r) => {
            let mut total = 0;
            total += log_overflow(&r.base.version, "assistant", &r.overflow, tx)?;
            total += log_overflow(
                &r.base.version,
                "assistant.message",
                &r.message.overflow,
                tx,
            )?;
            if let Some(ref usage) = r.message.usage {
                total += log_overflow(
                    &r.base.version,
                    "assistant.message.usage",
                    &usage.overflow,
                    tx,
                )?;
            }
            Ok(total)
        }

        JSONLRecord::Progress(r) => {
            log_overflow(&r.base.version, "progress", &r.overflow, tx)
        }

        JSONLRecord::System(r) => {
            log_overflow(&r.base.version, "system", &r.overflow, tx)
        }

        JSONLRecord::QueueOperation(r) => {
            log_overflow("unknown", "queue-operation", &r.overflow, tx)
        }

        JSONLRecord::Summary(r) => {
            log_overflow("unknown", "summary", &r.overflow, tx)
        }

        JSONLRecord::FileHistorySnapshot(r) => {
            log_overflow("unknown", "file-history-snapshot", &r.overflow, tx)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema;
    use rusqlite::Connection;

    /// Create an in-memory SQLite database with the schema applied.
    fn setup_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        schema::run_migrations(&conn).unwrap();
        conn
    }

    // -----------------------------------------------------------------------
    // Test 1: Two unknown fields -> 2 rows in schema_drift_log
    // -----------------------------------------------------------------------
    #[test]
    fn test_log_overflow_basic() {
        let conn = setup_db();
        let tx = conn.unchecked_transaction().unwrap();

        let mut overflow = HashMap::new();
        overflow.insert(
            "newField1".to_string(),
            serde_json::json!("some value"),
        );
        overflow.insert(
            "newField2".to_string(),
            serde_json::json!(42),
        );

        let logged = log_overflow("2.1.49", "user", &overflow, &tx).unwrap();
        tx.commit().unwrap();

        assert_eq!(logged, 2, "Should log 2 new entries");

        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM schema_drift_log WHERE record_type = 'user' AND version = '2.1.49'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 2);

        // Verify field names
        let mut stmt = conn
            .prepare(
                "SELECT field_name FROM schema_drift_log WHERE record_type = 'user' ORDER BY field_name",
            )
            .unwrap();
        let names: Vec<String> = stmt
            .query_map([], |row| row.get(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        assert!(names.contains(&"newField1".to_string()));
        assert!(names.contains(&"newField2".to_string()));

        // Verify sample_value
        let sample: String = conn
            .query_row(
                "SELECT sample_value FROM schema_drift_log WHERE field_name = 'newField1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(sample, "\"some value\"");
    }

    // -----------------------------------------------------------------------
    // Test 2: Same fields logged again -> occurrence_count increments,
    //         row count stays at 1 (ON CONFLICT DO UPDATE)
    // -----------------------------------------------------------------------
    #[test]
    fn test_log_overflow_idempotent() {
        let conn = setup_db();

        let mut overflow = HashMap::new();
        overflow.insert(
            "repeatedField".to_string(),
            serde_json::json!("first"),
        );

        // First call
        {
            let tx = conn.unchecked_transaction().unwrap();
            let logged = log_overflow("2.1.49", "assistant", &overflow, &tx).unwrap();
            tx.commit().unwrap();
            assert_eq!(logged, 1);
        }

        let count_before: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM schema_drift_log",
                [],
                |row| row.get(0),
            )
            .unwrap();

        // Second call with same field_name + record_type + version
        // ON CONFLICT DO UPDATE returns 1 (update counts as a change)
        {
            let tx = conn.unchecked_transaction().unwrap();
            let logged = log_overflow("2.1.49", "assistant", &overflow, &tx).unwrap();
            tx.commit().unwrap();
            assert_eq!(logged, 1, "ON CONFLICT DO UPDATE returns 1 for the updated row");
        }

        let count_after: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM schema_drift_log",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count_before, count_after, "Row count should be unchanged — update, not insert");

        // Verify occurrence_count is 2 after second observation
        let occurrence_count: i64 = conn
            .query_row(
                "SELECT occurrence_count FROM schema_drift_log WHERE field_name = 'repeatedField'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(occurrence_count, 2, "occurrence_count should be 2 after two observations");

        // Verify last_seen_at is populated
        let last_seen: String = conn
            .query_row(
                "SELECT last_seen_at FROM schema_drift_log WHERE field_name = 'repeatedField'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(!last_seen.is_empty(), "last_seen_at should be populated");
    }

    // -----------------------------------------------------------------------
    // Test 2b: Occurrence count tracks multiple re-observations correctly
    // -----------------------------------------------------------------------
    #[test]
    fn test_log_overflow_occurrence_count() {
        let conn = setup_db();

        let mut overflow = HashMap::new();
        overflow.insert(
            "trackedField".to_string(),
            serde_json::json!("value"),
        );

        // Insert twice
        for _ in 0..2 {
            let tx = conn.unchecked_transaction().unwrap();
            log_overflow("2.1.50", "user", &overflow, &tx).unwrap();
            tx.commit().unwrap();
        }

        // Verify occurrence_count = 2
        let occurrence_count: i64 = conn
            .query_row(
                "SELECT occurrence_count FROM schema_drift_log WHERE field_name = 'trackedField' AND version = '2.1.50'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(occurrence_count, 2, "occurrence_count should be 2 after two observations");

        // Verify last_seen_at is not NULL
        let last_seen: Option<String> = conn
            .query_row(
                "SELECT last_seen_at FROM schema_drift_log WHERE field_name = 'trackedField' AND version = '2.1.50'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(last_seen.is_some(), "last_seen_at should not be NULL");

        // Verify only one row exists (not two)
        let row_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM schema_drift_log WHERE field_name = 'trackedField' AND version = '2.1.50'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(row_count, 1, "Should have exactly one row despite two observations");
    }

    // -----------------------------------------------------------------------
    // Test 3: Value > 500 chars -> sample_value is truncated
    // -----------------------------------------------------------------------
    #[test]
    fn test_log_overflow_truncation() {
        let conn = setup_db();
        let tx = conn.unchecked_transaction().unwrap();

        let long_value = "x".repeat(1000);
        let mut overflow = HashMap::new();
        overflow.insert(
            "longField".to_string(),
            serde_json::Value::String(long_value.clone()),
        );

        let logged = log_overflow("2.1.49", "progress", &overflow, &tx).unwrap();
        tx.commit().unwrap();
        assert_eq!(logged, 1);

        let sample: String = conn
            .query_row(
                "SELECT sample_value FROM schema_drift_log WHERE field_name = 'longField'",
                [],
                |row| row.get(0),
            )
            .unwrap();

        // The JSON string representation includes surrounding quotes, so the
        // truncation happens on the serialized form (which is `"xxx..."`)
        assert!(
            sample.len() <= MAX_SAMPLE_VALUE_LEN + 3 + 2, // +3 for "...", +2 for quotes in JSON
            "Sample value should be truncated: got {} chars",
            sample.len()
        );
        assert!(sample.ends_with("..."), "Truncated value should end with '...'");
    }

    // -----------------------------------------------------------------------
    // Test 4: Empty HashMap -> no rows inserted, returns 0
    // -----------------------------------------------------------------------
    #[test]
    fn test_log_overflow_empty() {
        let conn = setup_db();
        let tx = conn.unchecked_transaction().unwrap();

        let overflow: HashMap<String, serde_json::Value> = HashMap::new();
        let logged = log_overflow("2.1.49", "system", &overflow, &tx).unwrap();
        tx.commit().unwrap();

        assert_eq!(logged, 0);

        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM schema_drift_log WHERE record_type = 'system'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 0);
    }

    // -----------------------------------------------------------------------
    // Test 5: log_record_overflow with AssistantRecord having multiple
    //         overflow maps -> separate entries for each qualified type
    // -----------------------------------------------------------------------
    #[test]
    fn test_log_record_overflow_assistant_multiple_maps() {
        let conn = setup_db();
        let tx = conn.unchecked_transaction().unwrap();

        use claude_history_core::message::{AssistantMessage, ContentBlock, UsageStats};
        use claude_history_core::record::{AssistantRecord, RecordBase};

        let mut record_overflow = HashMap::new();
        record_overflow.insert("apiError".to_string(), serde_json::json!("some error"));

        let mut message_overflow = HashMap::new();
        message_overflow.insert(
            "context_management".to_string(),
            serde_json::json!({"strategy": "truncate"}),
        );

        let mut usage_overflow = HashMap::new();
        usage_overflow.insert(
            "inference_geo".to_string(),
            serde_json::json!("us-west-2"),
        );

        let record = JSONLRecord::Assistant(AssistantRecord {
            base: RecordBase {
                uuid: "assist-drift".to_string(),
                timestamp: "2026-02-20T01:00:00.000Z".to_string(),
                session_id: "sess-drift".to_string(),
                version: "2.1.49".to_string(),
                cwd: "/tmp".to_string(),
                parent_uuid: None,
                is_sidechain: false,
                user_type: "external".to_string(),
                git_branch: "main".to_string(),
                slug: None,
                agent_id: None,
                team_name: None,
                is_meta: None,
            },
            message: AssistantMessage {
                id: "msg_drift".to_string(),
                model: "claude-opus-4-6".to_string(),
                role: "assistant".to_string(),
                content: vec![ContentBlock::Text {
                    text: "Hi".to_string(),
                }],
                stop_reason: Some("end_turn".to_string()),
                stop_sequence: None,
                usage: Some(UsageStats {
                    input_tokens: 100,
                    output_tokens: 50,
                    cache_creation_input_tokens: None,
                    cache_read_input_tokens: None,
                    cache_creation: None,
                    service_tier: None,
                    overflow: usage_overflow,
                }),
                overflow: message_overflow,
            },
            request_id: None,
            is_api_error_message: None,
            error: None,
            overflow: record_overflow,
        });

        let total = log_record_overflow(&record, &tx).unwrap();
        tx.commit().unwrap();

        assert_eq!(total, 3, "Should log 3 entries (1 record + 1 message + 1 usage)");

        // Verify qualified record_type values
        let mut stmt = conn
            .prepare("SELECT record_type FROM schema_drift_log ORDER BY record_type")
            .unwrap();
        let types: Vec<String> = stmt
            .query_map([], |row| row.get(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        assert!(types.contains(&"assistant".to_string()));
        assert!(types.contains(&"assistant.message".to_string()));
        assert!(types.contains(&"assistant.message.usage".to_string()));
    }
}
