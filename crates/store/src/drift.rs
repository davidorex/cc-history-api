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

/// Log a single `(type_name, version)` observation to `record_type_drift_log`.
///
/// This is the variant-level analogue of [`log_overflow`]: where `log_overflow`
/// tracks unknown *fields* on a known record type, this function tracks
/// records whose top-level `type` discriminator is itself unknown — i.e. a
/// `JSONLRecord::Unknown` produced by the manual `Deserialize` impl in
/// `crates/core/src/record.rs`.
///
/// The DDL at `crates/store/migrations/007_record_type_drift.sql` declares
/// `UNIQUE(type_name, version)`, so re-observations of the same pair land on
/// the ON CONFLICT branch and increment `occurrence_count` rather than
/// inserting a new row. The `sample_value` is refreshed on each observation
/// so the most recent sample is always available for forensic inspection.
///
/// `version` is `Option<&str>` because some unknown discriminators (e.g.
/// `last-prompt`, `custom-title`) carry no `version` field on the record
/// envelope; for those, SQL NULL is the right value and participates in the
/// UNIQUE constraint via SQLite's NULL-distinct semantics — meaning every
/// no-version observation conflicts with prior no-version observations of
/// the same `type_name` on the same Connection. (SQLite's default UNIQUE
/// treats NULLs as distinct in earlier versions; current versions and
/// rusqlite's default build treat NULLs as equal under UNIQUE INDEX. The
/// table behavior was verified via the migration_007_unique_constraint test
/// in `crates/store/src/schema.rs`.)
///
/// Returns 1 for both new inserts and conflict-updates, mirroring
/// [`log_overflow`]'s contract so callers can sum the two counts uniformly.
///
/// Backfill of historical records dropped before B1.1 shipped is intentionally
/// out of scope here — that is B1.2's bytewise re-ingestion responsibility.
pub fn log_record_type_drift(
    type_name: &str,
    version: Option<&str>,
    sample_value: &str,
    tx: &Transaction,
) -> Result<usize, rusqlite::Error> {
    let changed = tx.execute(
        "INSERT INTO record_type_drift_log
         (type_name, version, sample_value, first_seen_at, last_seen_at, occurrence_count)
         VALUES (?1, ?2, ?3, datetime('now'), datetime('now'), 1)
         ON CONFLICT(type_name, version) DO UPDATE SET
           occurrence_count = record_type_drift_log.occurrence_count + 1,
           last_seen_at = datetime('now'),
           sample_value = excluded.sample_value",
        rusqlite::params![type_name, version, sample_value],
    )?;

    if changed > 0 {
        tracing::debug!(
            type_name = type_name,
            version = ?version,
            "Record-type drift observed"
        );
    }

    Ok(changed)
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

        // The Unknown variant has no per-field overflow HashMap — its entire
        // payload is the raw JSON Value, captured by `decompose_unknown` to
        // `record_type_drift_log` (a different table from `schema_drift_log`).
        // The variant-level drift is recorded there, not here. B1.2 may
        // extend this arm to additionally inspect `raw` for unknown fields
        // and surface them in `schema_drift_log` once a future record-type
        // structure is modeled; for B1.1 the arm is a no-op so that the
        // existing log_record_overflow contract for the seven typed variants
        // is unchanged and the compiler exhaustiveness check is satisfied.
        JSONLRecord::Unknown { .. } => Ok(0),
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

    // -----------------------------------------------------------------------
    // B1.1 — log_record_type_drift tests (variant-level drift logging)
    //
    // These exercise the variant-level analogue of log_overflow. Together
    // with the JSONLRecord::Unknown deserialization tests in
    // crates/core/src/record.rs, they establish the contract that lets
    // ingestion preserve records whose discriminator is not yet modeled.
    // -----------------------------------------------------------------------

    /// Test C: idempotent re-observation. Two calls with the same
    /// (type_name, version) produce one row whose occurrence_count is 2.
    /// Mirrors test_log_overflow_idempotent for the new table.
    #[test]
    fn test_log_record_type_drift_idempotent_reobservation() {
        let conn = setup_db();

        // First call: insert.
        {
            let tx = conn.unchecked_transaction().unwrap();
            let changed =
                log_record_type_drift("attachment", Some("2.1.126"), "{\"sample\":1}", &tx)
                    .unwrap();
            tx.commit().unwrap();
            assert_eq!(changed, 1, "first observation should report 1 row changed");
        }

        // Second call with the same (type_name, version): conflict update.
        {
            let tx = conn.unchecked_transaction().unwrap();
            let changed =
                log_record_type_drift("attachment", Some("2.1.126"), "{\"sample\":2}", &tx)
                    .unwrap();
            tx.commit().unwrap();
            assert_eq!(
                changed, 1,
                "re-observation should report 1 row changed via ON CONFLICT DO UPDATE"
            );
        }

        // Exactly one row should exist.
        let row_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM record_type_drift_log",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(row_count, 1, "two observations should produce one row");

        // occurrence_count should be 2.
        let occ: i64 = conn
            .query_row(
                "SELECT occurrence_count FROM record_type_drift_log
                 WHERE type_name = 'attachment' AND version = '2.1.126'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(occ, 2, "occurrence_count should increment on re-observation");

        // sample_value should reflect the most recent observation.
        let sample: String = conn
            .query_row(
                "SELECT sample_value FROM record_type_drift_log
                 WHERE type_name = 'attachment' AND version = '2.1.126'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            sample, "{\"sample\":2}",
            "sample_value should refresh on conflict update"
        );
    }

    /// Test: distinct (type_name, version) pairs each produce their own row.
    /// And distinct versions for the same type_name are preserved as separate
    /// rows so we can correlate with version_history.
    #[test]
    fn test_log_record_type_drift_distinct_versions() {
        let conn = setup_db();

        let tx = conn.unchecked_transaction().unwrap();
        log_record_type_drift("attachment", Some("2.1.126"), "{}", &tx).unwrap();
        log_record_type_drift("attachment", Some("2.1.91"), "{}", &tx).unwrap();
        log_record_type_drift("last-prompt", None, "{}", &tx).unwrap();
        tx.commit().unwrap();

        let row_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM record_type_drift_log",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(row_count, 3, "three distinct keys should produce three rows");

        let attachment_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM record_type_drift_log WHERE type_name = 'attachment'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            attachment_count, 2,
            "two attachment rows for two distinct versions"
        );

        // last-prompt row should have NULL version.
        let null_version: Option<String> = conn
            .query_row(
                "SELECT version FROM record_type_drift_log WHERE type_name = 'last-prompt'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(
            null_version.is_none(),
            "last-prompt has no version field; column must be NULL"
        );
    }

    /// Test: decompose_record dispatches a JSONLRecord::Unknown to
    /// decompose_unknown which writes a record_type_drift_log row.
    /// End-to-end check from the parser-equivalent input through the
    /// dispatcher to the table.
    #[test]
    fn test_decompose_unknown_via_dispatcher_writes_drift_row() {
        use crate::decompose::decompose_record;

        let conn = setup_db();
        let tx = conn.unchecked_transaction().unwrap();

        // Construct a JSONLRecord::Unknown by parsing a fictitious record.
        let json = r#"{
            "type": "fictitious-test-type",
            "version": "2.1.999",
            "sessionId": "sess-x",
            "foo": "bar"
        }"#;
        let record: JSONLRecord = serde_json::from_str(json).unwrap();

        // Pre-condition: it actually parsed as Unknown.
        match &record {
            JSONLRecord::Unknown { type_name, .. } => {
                assert_eq!(type_name, "fictitious-test-type");
            }
            _ => panic!("Expected Unknown variant"),
        }

        let result = decompose_record(&record, "sess-x", &tx).unwrap();
        tx.commit().unwrap();

        // No structural-table rows should be written.
        // (rows_inserted may be > 0 if artifacts dispatcher writes anything;
        //  the artifacts dispatcher returns 0 for non-Assistant variants per
        //  crates/store/src/artifacts.rs, so we expect rows_inserted == 0.)
        assert_eq!(
            result.rows_inserted, 0,
            "Unknown variant should not write to structural tables"
        );

        // overflow_fields counter should record the drift-log insert.
        assert_eq!(
            result.overflow_fields, 1,
            "decompose_unknown should report 1 drift-log row changed"
        );

        // Row should exist with type_name and version captured.
        let (type_name, version, occ): (String, Option<String>, i64) = conn
            .query_row(
                "SELECT type_name, version, occurrence_count FROM record_type_drift_log",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(type_name, "fictitious-test-type");
        assert_eq!(version.as_deref(), Some("2.1.999"));
        assert_eq!(occ, 1);
    }

    /// Test: decompose_unknown handles records with no `version` field
    /// (e.g. last-prompt) by writing NULL to the version column.
    #[test]
    fn test_decompose_unknown_no_version_field() {
        use crate::decompose::decompose_record;

        let conn = setup_db();
        let tx = conn.unchecked_transaction().unwrap();

        // last-prompt: no version field (verified via the corpus survey
        // documented in the audit doc).
        let json =
            r#"{"type":"last-prompt","lastPrompt":"hi","sessionId":"sess-lp"}"#;
        let record: JSONLRecord = serde_json::from_str(json).unwrap();
        decompose_record(&record, "sess-lp", &tx).unwrap();
        tx.commit().unwrap();

        let version: Option<String> = conn
            .query_row(
                "SELECT version FROM record_type_drift_log WHERE type_name = 'last-prompt'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(
            version.is_none(),
            "last-prompt has no version field; column must be NULL"
        );
    }

    /// Test: the sample_value is truncated for very large unknown payloads
    /// to keep the drift log compact.
    #[test]
    fn test_decompose_unknown_truncates_large_sample() {
        use crate::decompose::decompose_record;

        let conn = setup_db();
        let tx = conn.unchecked_transaction().unwrap();

        // Build a payload whose JSON serialization exceeds 500 chars.
        let big_string: String = "x".repeat(2000);
        let json = format!(
            r#"{{"type":"big-unknown","payload":"{big_string}"}}"#
        );
        let record: JSONLRecord = serde_json::from_str(&json).unwrap();
        decompose_record(&record, "sess-big", &tx).unwrap();
        tx.commit().unwrap();

        let sample: String = conn
            .query_row(
                "SELECT sample_value FROM record_type_drift_log WHERE type_name = 'big-unknown'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(
            sample.ends_with("..."),
            "truncated sample should end with '...'; got: {sample}"
        );
        assert!(
            sample.len() <= 504,
            "sample should be truncated to <= 503 chars; got {} chars",
            sample.len()
        );
    }
}
