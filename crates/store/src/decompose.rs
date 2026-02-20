//! Record decomposition engine.
//!
//! Transforms parsed JSONL records into normalized SQLite rows across all target
//! tables. Each record type has a dedicated decompose function that inserts into
//! the appropriate tables. All decomposition uses `INSERT OR IGNORE` for
//! idempotency (safe re-sync without duplicating rows).
//!
//! The session_id_from_file parameter is passed from the sync layer for
//! lightweight records (summary, file-history-snapshot) that lack a sessionId
//! field in their JSON body. Full-base records carry their own session_id in
//! RecordBase.
//!
//! All inserts operate on a `&rusqlite::Transaction` reference for atomic
//! batch writes — the caller is responsible for committing or rolling back.

use std::collections::HashMap;

use claude_history_core::message::{ContentBlock, MessageContent, UsageStats};
use claude_history_core::progress::ProgressRecord;
use claude_history_core::record::{
    AssistantRecord, FileHistorySnapshotRecord, JSONLRecord, QueueOperationRecord, RecordBase,
    SummaryRecord, UserRecord,
};
use claude_history_core::system::SystemRecord;
use rusqlite::Transaction;

use crate::drift;

/// Result statistics returned from a single record decomposition.
#[derive(Debug, Clone, Default)]
pub struct DecomposeResult {
    /// Number of rows inserted across all tables for this record
    pub rows_inserted: usize,
    /// Number of overflow fields logged to schema_drift_log
    pub overflow_fields: usize,
}

/// Errors that can occur during record decomposition.
#[derive(Debug, thiserror::Error)]
pub enum DecomposeError {
    #[error("SQLite error during decomposition: {0}")]
    Sqlite(#[from] rusqlite::Error),

    #[error("JSON serialization error: {0}")]
    Json(#[from] serde_json::Error),
}

/// Top-level dispatcher that routes a parsed JSONL record to the appropriate
/// per-type decomposition function.
///
/// The `session_id_from_file` parameter provides the session ID extracted from
/// the JSONL filename — used by lightweight records (Summary, FileHistorySnapshot)
/// that do not carry a sessionId in their JSON body.
pub fn decompose_record(
    record: &JSONLRecord,
    session_id_from_file: &str,
    tx: &Transaction,
) -> Result<DecomposeResult, DecomposeError> {
    let mut result = match record {
        JSONLRecord::User(r) => decompose_user(r, tx)?,
        JSONLRecord::Assistant(r) => decompose_assistant(r, tx)?,
        JSONLRecord::Progress(r) => decompose_progress(r, tx)?,
        JSONLRecord::System(r) => decompose_system(r, tx)?,
        JSONLRecord::QueueOperation(r) => decompose_queue_operation(r, tx)?,
        JSONLRecord::Summary(r) => decompose_summary(r, session_id_from_file, tx)?,
        JSONLRecord::FileHistorySnapshot(r) => {
            decompose_file_history_snapshot(r, session_id_from_file, tx)?
        }
    };

    // Second pass: artifact extraction from tool_use blocks.
    // Runs in same transaction for atomicity. Produces file_operations
    // and git_operations rows from Write/Edit/Read/Bash tool inputs.
    // Returns 0 for record types without tool_use blocks (non-assistant).
    result.rows_inserted +=
        crate::artifacts::decompose_artifacts(record, session_id_from_file, tx)?;

    Ok(result)
}

/// Upsert a session row from a full-base record's RecordBase.
///
/// Uses INSERT OR IGNORE so the first record seen for a session creates the row,
/// and subsequent records for the same session are no-ops. We intentionally do not
/// UPDATE on conflict because session-level metadata (project_path, version) can
/// vary across records within the same session, and the first-seen value is the
/// most useful for provenance tracking.
fn upsert_session(base: &RecordBase, tx: &Transaction) -> Result<usize, rusqlite::Error> {
    let changed = tx.execute(
        "INSERT OR IGNORE INTO sessions (session_id, project_path, first_seen_at, version, slug, git_branch)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        rusqlite::params![
            base.session_id,
            base.cwd,
            base.timestamp,
            base.version,
            base.slug,
            base.git_branch,
        ],
    )?;
    if changed == 0 {
        tracing::debug!(
            session_id = %base.session_id,
            "Session already exists, INSERT OR IGNORE skipped duplicate"
        );
    }
    Ok(changed)
}

/// Upsert an agent row if agent_id is present on the record.
fn upsert_agent(
    base: &RecordBase,
    tx: &Transaction,
) -> Result<usize, rusqlite::Error> {
    if let Some(ref agent_id) = base.agent_id {
        let changed = tx.execute(
            "INSERT OR IGNORE INTO agents (agent_id, session_id, first_seen_at)
             VALUES (?1, ?2, ?3)",
            rusqlite::params![agent_id, base.session_id, base.timestamp],
        )?;
        Ok(changed)
    } else {
        Ok(0)
    }
}

/// Insert a message row for a full-base record type.
///
/// Returns the number of rows changed (0 if duplicate uuid detected by INSERT OR IGNORE).
fn insert_message(
    base: &RecordBase,
    msg_type: &str,
    model: Option<&str>,
    stop_reason: Option<&str>,
    request_id: Option<&str>,
    subtype: Option<&str>,
    tx: &Transaction,
) -> Result<usize, rusqlite::Error> {
    let changed = tx.execute(
        "INSERT OR IGNORE INTO messages (uuid, session_id, type, timestamp, parent_uuid,
         is_sidechain, user_type, cwd, git_branch, version, slug, agent_id, is_meta,
         model, stop_reason, request_id, subtype)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)",
        rusqlite::params![
            base.uuid,
            base.session_id,
            msg_type,
            base.timestamp,
            base.parent_uuid,
            base.is_sidechain as i32,
            base.user_type,
            base.cwd,
            base.git_branch,
            base.version,
            base.slug,
            base.agent_id,
            base.is_meta.map(|v| v as i32),
            model,
            stop_reason,
            request_id,
            subtype,
        ],
    )?;
    if changed == 0 {
        tracing::debug!(
            uuid = %base.uuid,
            msg_type = msg_type,
            "Message already exists, INSERT OR IGNORE skipped duplicate"
        );
    }
    Ok(changed)
}

/// Decompose a single content block into a message_content row.
///
/// For ToolUse blocks, also inserts a row into tool_executions.
fn decompose_content_block(
    message_uuid: &str,
    index: usize,
    block: &ContentBlock,
    tx: &Transaction,
) -> Result<usize, DecomposeError> {
    let mut rows = 0;

    match block {
        ContentBlock::Text { text } => {
            let changed = tx.execute(
                "INSERT OR IGNORE INTO message_content
                 (message_uuid, block_index, block_type, text_content)
                 VALUES (?1, ?2, 'text', ?3)",
                rusqlite::params![message_uuid, index as i64, text],
            )?;
            rows += changed;
        }

        ContentBlock::Thinking {
            thinking,
            signature,
        } => {
            let changed = tx.execute(
                "INSERT OR IGNORE INTO message_content
                 (message_uuid, block_index, block_type, text_content, thinking_signature)
                 VALUES (?1, ?2, 'thinking', ?3, ?4)",
                rusqlite::params![message_uuid, index as i64, thinking, signature],
            )?;
            rows += changed;
        }

        ContentBlock::ToolUse {
            id,
            name,
            input,
            ..
        } => {
            let input_json = serde_json::to_string(input)?;

            // Insert content block row
            let changed = tx.execute(
                "INSERT OR IGNORE INTO message_content
                 (message_uuid, block_index, block_type, tool_use_id, tool_name, tool_input)
                 VALUES (?1, ?2, 'tool_use', ?3, ?4, ?5)",
                rusqlite::params![message_uuid, index as i64, id, name, input_json],
            )?;
            rows += changed;

            // Also insert into tool_executions for dedicated tool tracking
            let te_changed = tx.execute(
                "INSERT OR IGNORE INTO tool_executions
                 (message_uuid, tool_use_id, tool_name, input_json)
                 VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![message_uuid, id, name, input_json],
            )?;
            rows += te_changed;
        }

        ContentBlock::ToolResult {
            tool_use_id,
            content,
            is_error,
        } => {
            let content_str = content.to_string();

            let changed = tx.execute(
                "INSERT OR IGNORE INTO message_content
                 (message_uuid, block_index, block_type, tool_use_id, text_content, is_error)
                 VALUES (?1, ?2, 'tool_result', ?3, ?4, ?5)",
                rusqlite::params![
                    message_uuid,
                    index as i64,
                    tool_use_id,
                    content_str,
                    is_error.map(|v| v as i32),
                ],
            )?;
            rows += changed;

            // ART-04: Link tool_result to its tool_use by populating result_content.
            // The tool_executions row was created during assistant record decomposition
            // with the same tool_use_id. Now we populate its result fields.
            // The UPDATE matches on tool_use_id alone because the tool_executions row
            // belongs to the assistant message, not this user message.
            // Safe even if no matching row exists (0 rows affected).
            let result_str = serde_json::to_string(content)?;
            tx.execute(
                "UPDATE tool_executions SET result_content = ?1, is_error = ?2 WHERE tool_use_id = ?3",
                rusqlite::params![
                    result_str,
                    is_error.map(|v| v as i32),
                    tool_use_id
                ],
            )?;
        }
    }

    Ok(rows)
}

/// Decompose user message content, handling both plain text and block array forms.
fn decompose_message_content(
    message_uuid: &str,
    content: &MessageContent,
    tx: &Transaction,
) -> Result<usize, DecomposeError> {
    let mut rows = 0;
    match content {
        MessageContent::Text(text) => {
            let changed = tx.execute(
                "INSERT OR IGNORE INTO message_content
                 (message_uuid, block_index, block_type, text_content)
                 VALUES (?1, 0, 'text', ?2)",
                rusqlite::params![message_uuid, text],
            )?;
            rows += changed;
        }
        MessageContent::Blocks(blocks) => {
            for (i, block) in blocks.iter().enumerate() {
                rows += decompose_content_block(message_uuid, i, block, tx)?;
            }
        }
    }
    Ok(rows)
}

/// Insert a token_usage row from assistant message usage stats.
fn insert_token_usage(
    message_uuid: &str,
    usage: &UsageStats,
    tx: &Transaction,
) -> Result<usize, DecomposeError> {
    let cache_creation_json = usage
        .cache_creation
        .as_ref()
        .map(|v| serde_json::to_string(v))
        .transpose()?;

    let extra_json = if usage.overflow.is_empty() {
        None
    } else {
        Some(serde_json::to_string(&usage.overflow)?)
    };

    let changed = tx.execute(
        "INSERT OR IGNORE INTO token_usage
         (message_uuid, input_tokens, output_tokens,
          cache_creation_input_tokens, cache_read_input_tokens,
          service_tier, cache_creation_json, extra_json)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        rusqlite::params![
            message_uuid,
            usage.input_tokens as i64,
            usage.output_tokens as i64,
            usage.cache_creation_input_tokens.map(|v| v as i64),
            usage.cache_read_input_tokens.map(|v| v as i64),
            usage.service_tier,
            cache_creation_json,
            extra_json,
        ],
    )?;
    Ok(changed)
}

// ---------------------------------------------------------------------------
// Per-type decomposition functions
// ---------------------------------------------------------------------------

/// Decompose a user record into sessions + messages + message_content rows.
/// [DECOMP-01]
fn decompose_user(
    r: &UserRecord,
    tx: &Transaction,
) -> Result<DecomposeResult, DecomposeError> {
    let mut result = DecomposeResult::default();

    // 1. Upsert session
    result.rows_inserted += upsert_session(&r.base, tx)?;

    // 2. Insert message
    result.rows_inserted += insert_message(&r.base, "user", None, None, None, None, tx)?;

    // 3. Decompose message content (string or blocks)
    result.rows_inserted += decompose_message_content(&r.base.uuid, &r.message.content, tx)?;

    // 4. Upsert agent if present
    result.rows_inserted += upsert_agent(&r.base, tx)?;

    // 5. Log overflow for drift detection
    result.overflow_fields += drift::log_overflow(
        &r.base.version,
        "user",
        &r.overflow,
        tx,
    )?;

    Ok(result)
}

/// Decompose an assistant record into sessions + messages + message_content
/// + token_usage + tool_executions rows.
/// [DECOMP-02]
fn decompose_assistant(
    r: &AssistantRecord,
    tx: &Transaction,
) -> Result<DecomposeResult, DecomposeError> {
    let mut result = DecomposeResult::default();

    // 1. Upsert session
    result.rows_inserted += upsert_session(&r.base, tx)?;

    // 2. Insert message with assistant-specific fields
    result.rows_inserted += insert_message(
        &r.base,
        "assistant",
        Some(&r.message.model),
        r.message.stop_reason.as_deref(),
        r.request_id.as_deref(),
        None,
        tx,
    )?;

    // 3. Decompose content blocks (assistant always has Vec<ContentBlock>)
    for (i, block) in r.message.content.iter().enumerate() {
        result.rows_inserted += decompose_content_block(&r.base.uuid, i, block, tx)?;
    }

    // 4. Insert token usage if present
    if let Some(ref usage) = r.message.usage {
        result.rows_inserted += insert_token_usage(&r.base.uuid, usage, tx)?;

        // Log usage overflow (server_tool_use, iterations, inference_geo, etc.)
        result.overflow_fields += drift::log_overflow(
            &r.base.version,
            "assistant.message.usage",
            &usage.overflow,
            tx,
        )?;
    }

    // 5. Upsert agent if present
    result.rows_inserted += upsert_agent(&r.base, tx)?;

    // 6. Log overflow from both AssistantRecord and AssistantMessage levels
    result.overflow_fields += drift::log_overflow(
        &r.base.version,
        "assistant",
        &r.overflow,
        tx,
    )?;
    result.overflow_fields += drift::log_overflow(
        &r.base.version,
        "assistant.message",
        &r.message.overflow,
        tx,
    )?;

    Ok(result)
}

/// Decompose a progress record into sessions + progress_events rows.
/// [DECOMP-03]
fn decompose_progress(
    r: &ProgressRecord,
    tx: &Transaction,
) -> Result<DecomposeResult, DecomposeError> {
    let mut result = DecomposeResult::default();

    // 1. Upsert session
    result.rows_inserted += upsert_session(&r.base, tx)?;

    // 2. Extract data_type from the data JSON object
    let data_type = r
        .data
        .get("type")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");

    let data_json = serde_json::to_string(&r.data)?;

    // 3. Insert progress_events row
    let changed = tx.execute(
        "INSERT OR IGNORE INTO progress_events (uuid, session_id, timestamp, data_type, data_json)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        rusqlite::params![
            r.base.uuid,
            r.base.session_id,
            r.base.timestamp,
            data_type,
            data_json,
        ],
    )?;
    result.rows_inserted += changed;

    // 4. Log overflow
    result.overflow_fields += drift::log_overflow(
        &r.base.version,
        "progress",
        &r.overflow,
        tx,
    )?;

    Ok(result)
}

/// Decompose a queue-operation record into a queue_operations row.
/// [DECOMP-04]
///
/// No session upsert — queue operations have session_id but lack the full
/// session metadata (project_path, version, etc.) needed for a meaningful
/// sessions row.
fn decompose_queue_operation(
    r: &QueueOperationRecord,
    tx: &Transaction,
) -> Result<DecomposeResult, DecomposeError> {
    let mut result = DecomposeResult::default();

    let changed = tx.execute(
        "INSERT INTO queue_operations (session_id, operation, timestamp, content)
         VALUES (?1, ?2, ?3, ?4)",
        rusqlite::params![r.session_id, r.operation, r.timestamp, r.content],
    )?;
    result.rows_inserted += changed;

    // Log overflow — version is not available on queue-operation records,
    // so we use "unknown" as the version context
    result.overflow_fields += drift::log_overflow(
        "unknown",
        "queue-operation",
        &r.overflow,
        tx,
    )?;

    Ok(result)
}

/// Decompose a system record into sessions + system_events rows.
fn decompose_system(
    r: &SystemRecord,
    tx: &Transaction,
) -> Result<DecomposeResult, DecomposeError> {
    let mut result = DecomposeResult::default();

    // 1. Upsert session
    result.rows_inserted += upsert_session(&r.base, tx)?;

    // 2. Build extra_json from overflow + explicit optional fields like hook_count
    let mut extra: HashMap<String, serde_json::Value> = HashMap::new();
    if let Some(hc) = r.hook_count {
        extra.insert("hookCount".to_string(), serde_json::Value::from(hc));
    }
    for (k, v) in &r.overflow {
        extra.insert(k.clone(), v.clone());
    }
    let extra_json = if extra.is_empty() {
        None
    } else {
        Some(serde_json::to_string(&extra)?)
    };

    // 3. Insert system_events row
    let changed = tx.execute(
        "INSERT OR IGNORE INTO system_events
         (uuid, session_id, timestamp, subtype, level, duration_ms, content, extra_json)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        rusqlite::params![
            r.base.uuid,
            r.base.session_id,
            r.base.timestamp,
            r.subtype,
            r.level,
            r.duration_ms.map(|v| v as i64),
            r.content,
            extra_json,
        ],
    )?;
    result.rows_inserted += changed;

    // 4. Upsert agent if present
    result.rows_inserted += upsert_agent(&r.base, tx)?;

    // 5. Log overflow for drift detection
    result.overflow_fields += drift::log_overflow(
        &r.base.version,
        "system",
        &r.overflow,
        tx,
    )?;

    Ok(result)
}

/// Decompose a summary record into a summaries row.
///
/// Summary records are lightweight — they have no uuid or sessionId field.
/// The session_id is derived from the JSONL filename by the sync layer.
fn decompose_summary(
    r: &SummaryRecord,
    session_id_from_file: &str,
    tx: &Transaction,
) -> Result<DecomposeResult, DecomposeError> {
    let mut result = DecomposeResult::default();

    let changed = tx.execute(
        "INSERT INTO summaries (session_id, summary, leaf_uuid)
         VALUES (?1, ?2, ?3)",
        rusqlite::params![session_id_from_file, r.summary, r.leaf_uuid],
    )?;
    result.rows_inserted += changed;

    // Log overflow — no version available on lightweight records
    result.overflow_fields += drift::log_overflow(
        "unknown",
        "summary",
        &r.overflow,
        tx,
    )?;

    Ok(result)
}

/// Handle a file-history-snapshot record.
///
/// Per plan: the spec tables don't include a file_history_snapshots table.
/// For Phase 1, we log that we encountered it at debug level and log any
/// overflow fields. The data is not silently dropped — it's acknowledged —
/// but full decomposition into a dedicated table is deferred to a later phase.
fn decompose_file_history_snapshot(
    r: &FileHistorySnapshotRecord,
    _session_id_from_file: &str,
    tx: &Transaction,
) -> Result<DecomposeResult, DecomposeError> {
    let mut result = DecomposeResult::default();

    tracing::debug!(
        message_id = %r.message_id,
        is_update = r.is_snapshot_update,
        "Encountered file-history-snapshot record; skipping decomposition (no target table in Phase 1)"
    );

    // Log overflow if present
    result.overflow_fields += drift::log_overflow(
        "unknown",
        "file-history-snapshot",
        &r.overflow,
        tx,
    )?;

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema;
    use claude_history_core::message::{
        AssistantMessage, ContentBlock, MessageContent, UsageStats, UserMessage,
    };
    use claude_history_core::progress::ProgressRecord;
    use claude_history_core::record::{
        AssistantRecord, QueueOperationRecord, RecordBase, SummaryRecord, UserRecord,
    };
    use claude_history_core::system::SystemRecord;
    use rusqlite::Connection;

    /// Create an in-memory SQLite database with the schema applied.
    fn setup_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        schema::run_migrations(&conn).unwrap();
        conn
    }

    /// Helper to create a standard RecordBase for tests.
    fn test_base(uuid: &str, session_id: &str) -> RecordBase {
        RecordBase {
            uuid: uuid.to_string(),
            timestamp: "2026-02-20T01:00:00.000Z".to_string(),
            session_id: session_id.to_string(),
            version: "2.1.49".to_string(),
            cwd: "/home/user/project".to_string(),
            parent_uuid: None,
            is_sidechain: false,
            user_type: "external".to_string(),
            git_branch: "main".to_string(),
            slug: Some("test-session".to_string()),
            agent_id: None,
            team_name: None,
            is_meta: None,
        }
    }

    // -----------------------------------------------------------------------
    // Test 1: UserRecord with string content -> messages table row exists
    // -----------------------------------------------------------------------
    #[test]
    fn test_decompose_user_string_content() {
        let conn = setup_db();
        let tx = conn.unchecked_transaction().unwrap();

        let record = UserRecord {
            base: test_base("user-001", "sess-001"),
            message: UserMessage {
                role: "user".to_string(),
                content: MessageContent::Text("Hello, Claude!".to_string()),
            },
            source_tool_assistant_uuid: None,
            tool_use_result: None,
            thinking_metadata: None,
            todos: None,
            permission_mode: None,
            overflow: HashMap::new(),
        };

        let result = decompose_user(&record, &tx).unwrap();
        tx.commit().unwrap();

        // Verify message row exists
        let msg_type: String = conn
            .query_row(
                "SELECT type FROM messages WHERE uuid = 'user-001'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(msg_type, "user");

        // Verify message_content row exists with text
        let text: String = conn
            .query_row(
                "SELECT text_content FROM message_content WHERE message_uuid = 'user-001'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(text, "Hello, Claude!");

        // Verify session was created
        let sess_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sessions WHERE session_id = 'sess-001'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(sess_count, 1);

        assert!(result.rows_inserted >= 3, "Should insert session + message + content");
    }

    // -----------------------------------------------------------------------
    // Test 2: AssistantRecord with text + tool_use -> message_content +
    //         tool_executions + token_usage rows
    // -----------------------------------------------------------------------
    #[test]
    fn test_decompose_assistant_with_blocks() {
        let conn = setup_db();
        let tx = conn.unchecked_transaction().unwrap();

        let record = AssistantRecord {
            base: test_base("assist-001", "sess-002"),
            message: AssistantMessage {
                id: "msg_001".to_string(),
                model: "claude-opus-4-6".to_string(),
                role: "assistant".to_string(),
                content: vec![
                    ContentBlock::Text {
                        text: "Here is my response.".to_string(),
                    },
                    ContentBlock::ToolUse {
                        id: "tool-001".to_string(),
                        name: "Read".to_string(),
                        input: serde_json::json!({"file_path": "/tmp/test.txt"}),
                        caller: None,
                    },
                    ContentBlock::Thinking {
                        thinking: "Let me think...".to_string(),
                        signature: Some("sig-abc".to_string()),
                    },
                ],
                stop_reason: Some("tool_use".to_string()),
                stop_sequence: None,
                usage: Some(UsageStats {
                    input_tokens: 1000,
                    output_tokens: 500,
                    cache_creation_input_tokens: Some(200),
                    cache_read_input_tokens: Some(800),
                    cache_creation: None,
                    service_tier: Some("standard".to_string()),
                    overflow: HashMap::new(),
                }),
                overflow: HashMap::new(),
            },
            request_id: Some("req_011CYJ".to_string()),
            is_api_error_message: None,
            error: None,
            overflow: HashMap::new(),
        };

        let result = decompose_assistant(&record, &tx).unwrap();
        tx.commit().unwrap();

        // Verify message row
        let model: String = conn
            .query_row(
                "SELECT model FROM messages WHERE uuid = 'assist-001'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(model, "claude-opus-4-6");

        // Verify message_content rows — should be 3 blocks
        let content_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM message_content WHERE message_uuid = 'assist-001'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(content_count, 3, "Should have 3 content blocks");

        // Verify block types
        let mut stmt = conn
            .prepare(
                "SELECT block_type FROM message_content WHERE message_uuid = 'assist-001' ORDER BY block_index",
            )
            .unwrap();
        let types: Vec<String> = stmt
            .query_map([], |row| row.get(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        assert_eq!(types, vec!["text", "tool_use", "thinking"]);

        // Verify tool_executions row
        let tool_name: String = conn
            .query_row(
                "SELECT tool_name FROM tool_executions WHERE message_uuid = 'assist-001'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(tool_name, "Read");

        // Verify token_usage row
        let input_tokens: i64 = conn
            .query_row(
                "SELECT input_tokens FROM token_usage WHERE message_uuid = 'assist-001'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(input_tokens, 1000);

        let service_tier: String = conn
            .query_row(
                "SELECT service_tier FROM token_usage WHERE message_uuid = 'assist-001'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(service_tier, "standard");

        assert!(result.rows_inserted >= 7, "session + message + 3 content + 1 tool_exec + 1 token_usage");
    }

    // -----------------------------------------------------------------------
    // Test 3: ProgressRecord -> progress_events row with correct data_type
    // -----------------------------------------------------------------------
    #[test]
    fn test_decompose_progress() {
        let conn = setup_db();
        let tx = conn.unchecked_transaction().unwrap();

        let record = ProgressRecord {
            base: test_base("prog-001", "sess-003"),
            data: serde_json::json!({
                "type": "hook_progress",
                "hookEvent": "pre-commit",
                "hookName": "lint"
            }),
            overflow: HashMap::new(),
        };

        let result = decompose_progress(&record, &tx).unwrap();
        tx.commit().unwrap();

        let data_type: String = conn
            .query_row(
                "SELECT data_type FROM progress_events WHERE uuid = 'prog-001'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(data_type, "hook_progress");

        let data_json: String = conn
            .query_row(
                "SELECT data_json FROM progress_events WHERE uuid = 'prog-001'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&data_json).unwrap();
        assert_eq!(parsed["hookEvent"], "pre-commit");

        assert!(result.rows_inserted >= 2, "session + progress_events");
    }

    // -----------------------------------------------------------------------
    // Test 4: QueueOperationRecord -> queue_operations row
    // -----------------------------------------------------------------------
    #[test]
    fn test_decompose_queue_operation() {
        let conn = setup_db();
        let tx = conn.unchecked_transaction().unwrap();

        let record = QueueOperationRecord {
            operation: "enqueue".to_string(),
            timestamp: "2026-02-20T01:00:00.000Z".to_string(),
            session_id: "sess-004".to_string(),
            content: Some("Fix the bug".to_string()),
            overflow: HashMap::new(),
        };

        let result = decompose_queue_operation(&record, &tx).unwrap();
        tx.commit().unwrap();

        let operation: String = conn
            .query_row(
                "SELECT operation FROM queue_operations WHERE session_id = 'sess-004'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(operation, "enqueue");

        let content: String = conn
            .query_row(
                "SELECT content FROM queue_operations WHERE session_id = 'sess-004'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(content, "Fix the bug");

        assert_eq!(result.rows_inserted, 1);
    }

    // -----------------------------------------------------------------------
    // Test 5: SystemRecord with subtype "turn_duration" -> system_events row
    // -----------------------------------------------------------------------
    #[test]
    fn test_decompose_system() {
        let conn = setup_db();
        let tx = conn.unchecked_transaction().unwrap();

        let record = SystemRecord {
            base: test_base("sys-001", "sess-005"),
            subtype: "turn_duration".to_string(),
            level: None,
            duration_ms: Some(4532),
            hook_count: None,
            content: None,
            overflow: HashMap::new(),
        };

        let result = decompose_system(&record, &tx).unwrap();
        tx.commit().unwrap();

        let subtype: String = conn
            .query_row(
                "SELECT subtype FROM system_events WHERE uuid = 'sys-001'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(subtype, "turn_duration");

        let duration: i64 = conn
            .query_row(
                "SELECT duration_ms FROM system_events WHERE uuid = 'sys-001'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(duration, 4532);

        assert!(result.rows_inserted >= 2, "session + system_events");
    }

    // -----------------------------------------------------------------------
    // Test 6: SummaryRecord with fake session_id -> summaries row
    // -----------------------------------------------------------------------
    #[test]
    fn test_decompose_summary() {
        let conn = setup_db();
        let tx = conn.unchecked_transaction().unwrap();

        let record = SummaryRecord {
            summary: "User asked to refactor auth module.".to_string(),
            leaf_uuid: "leaf-uuid-001".to_string(),
            overflow: HashMap::new(),
        };

        let result = decompose_summary(&record, "sess-from-filename", &tx).unwrap();
        tx.commit().unwrap();

        let summary_text: String = conn
            .query_row(
                "SELECT summary FROM summaries WHERE session_id = 'sess-from-filename'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(summary_text, "User asked to refactor auth module.");

        let leaf: String = conn
            .query_row(
                "SELECT leaf_uuid FROM summaries WHERE session_id = 'sess-from-filename'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(leaf, "leaf-uuid-001");

        assert_eq!(result.rows_inserted, 1);
    }

    // -----------------------------------------------------------------------
    // Test 7: Idempotency — decompose same record twice, no error, row count
    //         unchanged (for INSERT OR IGNORE paths)
    // -----------------------------------------------------------------------
    #[test]
    fn test_decompose_idempotency() {
        let conn = setup_db();

        let record = UserRecord {
            base: test_base("user-idem", "sess-idem"),
            message: UserMessage {
                role: "user".to_string(),
                content: MessageContent::Text("test".to_string()),
            },
            source_tool_assistant_uuid: None,
            tool_use_result: None,
            thinking_metadata: None,
            todos: None,
            permission_mode: None,
            overflow: HashMap::new(),
        };

        // First decomposition
        {
            let tx = conn.unchecked_transaction().unwrap();
            decompose_user(&record, &tx).unwrap();
            tx.commit().unwrap();
        }

        let count_after_first: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM messages WHERE uuid = 'user-idem'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count_after_first, 1);

        // Second decomposition — should not error, should not create duplicate
        {
            let tx = conn.unchecked_transaction().unwrap();
            let result = decompose_user(&record, &tx).unwrap();
            tx.commit().unwrap();
            // INSERT OR IGNORE means rows_inserted may be 0 for duplicates
            // but no error should occur
            assert_eq!(result.rows_inserted, 0, "No new rows on duplicate decomposition");
        }

        let count_after_second: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM messages WHERE uuid = 'user-idem'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count_after_second, 1, "Row count unchanged after duplicate");
    }

    // -----------------------------------------------------------------------
    // Test 8: Top-level dispatcher routes all 7 record types correctly
    // -----------------------------------------------------------------------
    #[test]
    fn test_decompose_record_dispatcher() {
        let conn = setup_db();
        let tx = conn.unchecked_transaction().unwrap();

        // User record
        let user = JSONLRecord::User(UserRecord {
            base: test_base("disp-user", "sess-disp"),
            message: UserMessage {
                role: "user".to_string(),
                content: MessageContent::Text("test".to_string()),
            },
            source_tool_assistant_uuid: None,
            tool_use_result: None,
            thinking_metadata: None,
            todos: None,
            permission_mode: None,
            overflow: HashMap::new(),
        });
        decompose_record(&user, "sess-disp", &tx).unwrap();

        // Assistant record
        let assistant = JSONLRecord::Assistant(AssistantRecord {
            base: test_base("disp-assist", "sess-disp"),
            message: AssistantMessage {
                id: "msg_disp".to_string(),
                model: "claude-opus-4-6".to_string(),
                role: "assistant".to_string(),
                content: vec![ContentBlock::Text {
                    text: "Hello".to_string(),
                }],
                stop_reason: Some("end_turn".to_string()),
                stop_sequence: None,
                usage: None,
                overflow: HashMap::new(),
            },
            request_id: None,
            is_api_error_message: None,
            error: None,
            overflow: HashMap::new(),
        });
        decompose_record(&assistant, "sess-disp", &tx).unwrap();

        // Progress record
        let progress = JSONLRecord::Progress(ProgressRecord {
            base: test_base("disp-prog", "sess-disp"),
            data: serde_json::json!({"type": "agent_progress", "message": "working"}),
            overflow: HashMap::new(),
        });
        decompose_record(&progress, "sess-disp", &tx).unwrap();

        // System record
        let system = JSONLRecord::System(SystemRecord {
            base: test_base("disp-sys", "sess-disp"),
            subtype: "turn_duration".to_string(),
            level: None,
            duration_ms: Some(100),
            hook_count: None,
            content: None,
            overflow: HashMap::new(),
        });
        decompose_record(&system, "sess-disp", &tx).unwrap();

        // Queue operation
        let queue = JSONLRecord::QueueOperation(QueueOperationRecord {
            operation: "enqueue".to_string(),
            timestamp: "2026-02-20T01:00:00.000Z".to_string(),
            session_id: "sess-disp".to_string(),
            content: None,
            overflow: HashMap::new(),
        });
        decompose_record(&queue, "sess-disp", &tx).unwrap();

        // Summary
        let summary = JSONLRecord::Summary(SummaryRecord {
            summary: "A summary.".to_string(),
            leaf_uuid: "leaf-001".to_string(),
            overflow: HashMap::new(),
        });
        decompose_record(&summary, "sess-disp", &tx).unwrap();

        // File history snapshot
        let fhs = JSONLRecord::FileHistorySnapshot(FileHistorySnapshotRecord {
            message_id: "msg-snap-001".to_string(),
            snapshot: serde_json::json!({"trackedFileBackups": {}}),
            is_snapshot_update: false,
            overflow: HashMap::new(),
        });
        decompose_record(&fhs, "sess-disp", &tx).unwrap();

        tx.commit().unwrap();

        // Verify: 3 messages (user, assistant, system via messages table — progress
        // goes to progress_events, queue to queue_operations, summary to summaries)
        // Actually: user, assistant go to messages. system_events has its own table.
        // Progress goes to progress_events. Let me verify each table.

        let msg_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM messages", [], |row| row.get(0))
            .unwrap();
        assert_eq!(msg_count, 2, "user + assistant in messages table");

        let prog_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM progress_events", [], |row| row.get(0))
            .unwrap();
        assert_eq!(prog_count, 1, "1 progress event");

        let sys_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM system_events", [], |row| row.get(0))
            .unwrap();
        assert_eq!(sys_count, 1, "1 system event");

        let q_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM queue_operations", [], |row| row.get(0))
            .unwrap();
        assert_eq!(q_count, 1, "1 queue operation");

        let sum_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM summaries", [], |row| row.get(0))
            .unwrap();
        assert_eq!(sum_count, 1, "1 summary");
    }

    // -----------------------------------------------------------------------
    // Test 9: User record with block array content (tool_result blocks)
    // -----------------------------------------------------------------------
    #[test]
    fn test_decompose_user_block_content() {
        let conn = setup_db();
        let tx = conn.unchecked_transaction().unwrap();

        let record = UserRecord {
            base: test_base("user-blocks", "sess-blocks"),
            message: UserMessage {
                role: "user".to_string(),
                content: MessageContent::Blocks(vec![
                    ContentBlock::ToolResult {
                        tool_use_id: "tool-001".to_string(),
                        content: serde_json::Value::String("File written.".to_string()),
                        is_error: Some(false),
                    },
                    ContentBlock::Text {
                        text: "Here is the result.".to_string(),
                    },
                ]),
            },
            source_tool_assistant_uuid: Some("assist-ref".to_string()),
            tool_use_result: None,
            thinking_metadata: None,
            todos: None,
            permission_mode: None,
            overflow: HashMap::new(),
        };

        decompose_user(&record, &tx).unwrap();
        tx.commit().unwrap();

        let content_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM message_content WHERE message_uuid = 'user-blocks'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(content_count, 2, "Should have 2 content blocks");

        // Verify block_type for the tool_result
        let block_type: String = conn
            .query_row(
                "SELECT block_type FROM message_content WHERE message_uuid = 'user-blocks' AND block_index = 0",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(block_type, "tool_result");
    }

    // -----------------------------------------------------------------------
    // Test 9b: Tool result matching — tool_result UPDATE populates
    //          tool_executions.result_content and is_error via tool_use_id
    //          [ART-04]
    // -----------------------------------------------------------------------
    #[test]
    fn test_tool_result_updates_tool_executions() {
        let conn = setup_db();

        // Step 1: Decompose an assistant record with a tool_use block.
        // This creates the tool_executions row with result_content = NULL.
        {
            let tx = conn.unchecked_transaction().unwrap();
            let assistant = AssistantRecord {
                base: test_base("assist-tr", "sess-tr"),
                message: AssistantMessage {
                    id: "msg_tr".to_string(),
                    model: "claude-opus-4-6".to_string(),
                    role: "assistant".to_string(),
                    content: vec![ContentBlock::ToolUse {
                        id: "tool-tr-001".to_string(),
                        name: "Read".to_string(),
                        input: serde_json::json!({"file_path": "/tmp/test.txt"}),
                        caller: None,
                    }],
                    stop_reason: Some("tool_use".to_string()),
                    stop_sequence: None,
                    usage: None,
                    overflow: HashMap::new(),
                },
                request_id: None,
                is_api_error_message: None,
                error: None,
                overflow: HashMap::new(),
            };
            decompose_assistant(&assistant, &tx).unwrap();
            tx.commit().unwrap();
        }

        // Verify tool_executions row exists with NULL result_content
        let result_before: Option<String> = conn
            .query_row(
                "SELECT result_content FROM tool_executions WHERE tool_use_id = 'tool-tr-001'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(result_before.is_none(), "result_content should be NULL before tool_result");

        // Step 2: Decompose a user record with a tool_result block referencing
        // the same tool_use_id. This should UPDATE the tool_executions row.
        {
            let tx = conn.unchecked_transaction().unwrap();
            let user = UserRecord {
                base: test_base("user-tr", "sess-tr"),
                message: UserMessage {
                    role: "user".to_string(),
                    content: MessageContent::Blocks(vec![ContentBlock::ToolResult {
                        tool_use_id: "tool-tr-001".to_string(),
                        content: serde_json::json!("File contents: hello world"),
                        is_error: Some(false),
                    }]),
                },
                source_tool_assistant_uuid: Some("assist-tr".to_string()),
                tool_use_result: None,
                thinking_metadata: None,
                todos: None,
                permission_mode: None,
                overflow: HashMap::new(),
            };
            decompose_user(&user, &tx).unwrap();
            tx.commit().unwrap();
        }

        // Verify tool_executions row now has result_content populated
        let (result_content, is_error): (String, i32) = conn
            .query_row(
                "SELECT result_content, is_error FROM tool_executions WHERE tool_use_id = 'tool-tr-001'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert!(
            result_content.contains("hello world"),
            "result_content should contain the tool result, got: {}",
            result_content
        );
        assert_eq!(is_error, 0, "is_error should be 0 (false)");
    }

    // -----------------------------------------------------------------------
    // Test 10: Assistant record with agent_id -> agents table upsert
    // -----------------------------------------------------------------------
    #[test]
    fn test_decompose_assistant_with_agent() {
        let conn = setup_db();
        let tx = conn.unchecked_transaction().unwrap();

        let mut base = test_base("assist-agent", "sess-agent");
        base.agent_id = Some("agent-001".to_string());

        let record = AssistantRecord {
            base,
            message: AssistantMessage {
                id: "msg_agent".to_string(),
                model: "claude-opus-4-6".to_string(),
                role: "assistant".to_string(),
                content: vec![ContentBlock::Text {
                    text: "Done.".to_string(),
                }],
                stop_reason: Some("end_turn".to_string()),
                stop_sequence: None,
                usage: None,
                overflow: HashMap::new(),
            },
            request_id: None,
            is_api_error_message: None,
            error: None,
            overflow: HashMap::new(),
        };

        decompose_assistant(&record, &tx).unwrap();
        tx.commit().unwrap();

        let agent_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM agents WHERE agent_id = 'agent-001'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(agent_count, 1, "Agent should be upserted");
    }

    // -----------------------------------------------------------------------
    // Test 11: Progress record with missing data.type -> "unknown"
    // -----------------------------------------------------------------------
    #[test]
    fn test_decompose_progress_unknown_data_type() {
        let conn = setup_db();
        let tx = conn.unchecked_transaction().unwrap();

        let record = ProgressRecord {
            base: test_base("prog-nodata", "sess-prog2"),
            data: serde_json::json!({"message": "no type field here"}),
            overflow: HashMap::new(),
        };

        decompose_progress(&record, &tx).unwrap();
        tx.commit().unwrap();

        let data_type: String = conn
            .query_row(
                "SELECT data_type FROM progress_events WHERE uuid = 'prog-nodata'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(data_type, "unknown");
    }

    // -----------------------------------------------------------------------
    // Test 12: System record with hook_count -> extra_json contains hookCount
    // -----------------------------------------------------------------------
    #[test]
    fn test_decompose_system_with_extra_json() {
        let conn = setup_db();
        let tx = conn.unchecked_transaction().unwrap();

        let mut overflow = HashMap::new();
        overflow.insert(
            "hookInfos".to_string(),
            serde_json::json!([{"name": "pre-commit"}]),
        );

        let record = SystemRecord {
            base: test_base("sys-extra", "sess-extra"),
            subtype: "stop_hook_summary".to_string(),
            level: Some("info".to_string()),
            duration_ms: None,
            hook_count: Some(3),
            content: None,
            overflow,
        };

        decompose_system(&record, &tx).unwrap();
        tx.commit().unwrap();

        let extra_json: String = conn
            .query_row(
                "SELECT extra_json FROM system_events WHERE uuid = 'sys-extra'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&extra_json).unwrap();
        assert_eq!(parsed["hookCount"], 3);
        assert!(parsed["hookInfos"].is_array());
    }
}
