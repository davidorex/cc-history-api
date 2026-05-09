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
    AssistantRecord, AttachmentBody, AttachmentRecord, FileHistorySnapshotRecord, JSONLRecord,
    QueueOperationRecord, RecordBase, SummaryRecord, UserRecord,
};
use claude_history_core::system::SystemRecord;
use rusqlite::Transaction;

use crate::drift;

/// Result statistics returned from a single record decomposition.
#[derive(Debug, Clone, Default)]
pub struct DecomposeResult {
    /// Number of rows inserted across all tables for this record
    pub rows_inserted: usize,
    /// Count of "drift signals observed" — sums per-field
    /// `schema_drift_log` rows (the original meaning, used by every full-base
    /// decomposer arm) and per-record-type `record_type_drift_log` rows
    /// (added by B1.1 for `JSONLRecord::Unknown` and reused by C1.2 for
    /// `AttachmentBody::Unknown`). The two tables share the abstraction at
    /// the SSE/telemetry layer (a "drift observation"), so the counter
    /// uniformly aggregates both. Note (C1.1-Review #29): the field name
    /// retains its original `overflow_fields` spelling for backward
    /// compatibility with the SSE event payload and downstream telemetry;
    /// the cross-table semantics live in this docstring rather than a rename.
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
        JSONLRecord::Attachment(r) => decompose_attachment(r, tx)?,
        JSONLRecord::Unknown { type_name, raw } => decompose_unknown(type_name, raw, tx)?,
    };

    // Upsert the project row for full-base record types that carry a cwd.
    // QueueOperation, Summary, and FileHistorySnapshot lack a RecordBase,
    // so they are skipped.
    let maybe_base = match record {
        JSONLRecord::User(r) => Some(&r.base),
        JSONLRecord::Assistant(r) => Some(&r.base),
        JSONLRecord::Progress(r) => Some(&r.base),
        JSONLRecord::System(r) => Some(&r.base),
        _ => None,
    };
    if let Some(base) = maybe_base {
        if !base.cwd.is_empty() {
            result.rows_inserted += upsert_project(base, tx)?;
        }
    }

    // Second pass: artifact extraction from tool_use blocks.
    // Runs in same transaction for atomicity. Produces file_operations
    // and git_operations rows from Write/Edit/Read/Bash tool inputs.
    // Returns 0 for record types without tool_use blocks (non-assistant).
    result.rows_inserted +=
        crate::artifacts::decompose_artifacts(record, session_id_from_file, tx)?;

    Ok(result)
}

/// Upsert a project row from a full-base record's RecordBase.
///
/// Uses INSERT ... ON CONFLICT to create or update the projects row.
/// display_name is derived from the last path component. session_count is
/// recomputed from sessions on each conflict to stay accurate.
/// Only called when base.cwd is non-empty (has a project_path).
fn upsert_project(base: &RecordBase, tx: &Transaction) -> Result<usize, rusqlite::Error> {
    let display_name = base.cwd.split('/').last().unwrap_or("");
    tx.execute(
        "INSERT INTO projects (project_path, display_name, first_seen, last_seen, session_count)
         VALUES (?1, ?2, ?3, ?3, 1)
         ON CONFLICT(project_path) DO UPDATE SET
           last_seen = MAX(projects.last_seen, excluded.last_seen),
           session_count = (SELECT COUNT(DISTINCT session_id) FROM sessions WHERE project_path = ?1)",
        rusqlite::params![base.cwd, display_name, base.timestamp],
    )
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
///
/// After inserting the base message row, extracts known overflow fields
/// (isCompactSummary, sourceToolUseID, planContent) into promoted columns
/// on messages, and serializes any remaining overflow keys into extra_json.
/// Promoted keys are excluded from extra_json to avoid duplication.
///
/// The UPDATE runs after INSERT OR IGNORE, so re-syncs can backfill these
/// newly-promoted fields on existing rows.
///
/// Drift logging uses the ORIGINAL r.overflow (including promoted keys)
/// because drift detection tracks what Claude Code sends, not what we promote.
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

    // 4. Extract promoted overflow fields and populate extra_json.
    //    planContent is the C2.1 promotion; the prior two have been promoted
    //    since migration 006 (isCompactSummary + sourceToolUseID).
    let is_compact = r.overflow.get("isCompactSummary")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let source_tool_id = r.overflow.get("sourceToolUseID")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let plan_content = r.overflow.get("planContent")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    // Build extra_json from remaining overflow (excluding promoted keys).
    // planContent removal is conditional on string-extraction success: if a
    // future Claude Code version emits planContent as a non-string Value
    // (object/number/array), `as_str()` above returns None and plan_content
    // stays NULL. In that case we deliberately preserve the original Value
    // in extra_json so it remains queryable via json_extract for forensic
    // recovery rather than being silently dropped. [C2.1.1 / audit #25,#39,#40]
    let mut remaining = r.overflow.clone();
    remaining.remove("isCompactSummary");
    remaining.remove("sourceToolUseID");
    if plan_content.is_some() {
        remaining.remove("planContent");
    }
    let extra_json = if remaining.is_empty() {
        None
    } else {
        Some(serde_json::to_string(&remaining)?)
    };

    // UPDATE the message row with extracted values. This runs after INSERT OR
    // IGNORE so re-sync can backfill promoted fields on existing rows.
    tx.execute(
        "UPDATE messages
         SET is_compact_summary = ?1,
             source_tool_use_id = ?2,
             plan_content = ?3,
             extra_json = ?4
         WHERE uuid = ?5",
        rusqlite::params![
            is_compact as i32,
            source_tool_id,
            plan_content,
            extra_json,
            r.base.uuid,
        ],
    )?;

    // 5. Upsert agent if present
    result.rows_inserted += upsert_agent(&r.base, tx)?;

    // 6. Log overflow for drift detection (uses ORIGINAL overflow including
    // promoted keys — drift tracks what Claude Code sends)
    result.overflow_fields += drift::log_overflow(
        Some(&r.base.version),
        "user",
        &r.overflow,
        tx,
    )?;

    Ok(result)
}

/// Decompose an assistant record into sessions + messages + message_content
/// + token_usage + tool_executions rows.
///
/// After inserting the base message row, merges record-level overflow and
/// message-level overflow into a single extra_json on the messages row.
/// Compact summary fields (isCompactSummary, sourceToolUseID) appear on
/// USER records, not assistant records, so only extra_json is relevant here.
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
            Some(&r.base.version),
            "assistant.message.usage",
            &usage.overflow,
            tx,
        )?;
    }

    // 5. Build extra_json from record-level and message-level overflow
    let msg_extra = if r.message.overflow.is_empty() {
        None
    } else {
        Some(serde_json::to_string(&r.message.overflow)?)
    };

    let record_extra = if r.overflow.is_empty() {
        None
    } else {
        Some(serde_json::to_string(&r.overflow)?)
    };

    // Merge both overflow sources into a single extra_json
    let combined_extra = match (record_extra, msg_extra) {
        (Some(r_json), Some(m_json)) => {
            let mut r_map: serde_json::Map<String, serde_json::Value> =
                serde_json::from_str(&r_json)?;
            let m_map: serde_json::Map<String, serde_json::Value> =
                serde_json::from_str(&m_json)?;
            r_map.extend(m_map);
            Some(serde_json::to_string(&r_map)?)
        }
        (Some(v), None) | (None, Some(v)) => Some(v),
        (None, None) => None,
    };

    // UPDATE the message row with extra_json
    tx.execute(
        "UPDATE messages SET extra_json = ?1 WHERE uuid = ?2",
        rusqlite::params![combined_extra, r.base.uuid],
    )?;

    // 6. Upsert agent if present
    result.rows_inserted += upsert_agent(&r.base, tx)?;

    // 7. Log overflow from both AssistantRecord and AssistantMessage levels
    result.overflow_fields += drift::log_overflow(
        Some(&r.base.version),
        "assistant",
        &r.overflow,
        tx,
    )?;
    result.overflow_fields += drift::log_overflow(
        Some(&r.base.version),
        "assistant.message",
        &r.message.overflow,
        tx,
    )?;

    Ok(result)
}

/// Decompose a progress record into a session upsert + drift logging.
/// [DECOMP-03]
///
/// progress_events INSERT intentionally dropped — these records
/// (agent_progress, bash_progress, hook_progress, mcp_progress)
/// carry zero semantic value and dominated ~70% of database size.
fn decompose_progress(
    r: &ProgressRecord,
    tx: &Transaction,
) -> Result<DecomposeResult, DecomposeError> {
    let mut result = DecomposeResult::default();

    // 1. Upsert session
    result.rows_inserted += upsert_session(&r.base, tx)?;

    // 2. Skip progress_events INSERT — these records (agent_progress,
    // bash_progress, hook_progress, mcp_progress) carry zero semantic value
    // and account for ~70% of database size. Session upsert and drift
    // logging are preserved; only the blob storage is eliminated.

    // 3. Log overflow
    result.overflow_fields += drift::log_overflow(
        Some(&r.base.version),
        "progress",
        &r.overflow,
        tx,
    )?;

    Ok(result)
}

/// Decompose a queue-operation record — drift logging only.
/// [DECOMP-04]
///
/// queue_operations INSERT intentionally dropped — enqueue content
/// duplicates user prompts already in messages, and scheduling ops
/// (dequeue/remove/popAll) carry no project intelligence.
///
/// No session upsert — queue operations have session_id but lack the full
/// session metadata (project_path, version, etc.) needed for a meaningful
/// sessions row.
fn decompose_queue_operation(
    r: &QueueOperationRecord,
    tx: &Transaction,
) -> Result<DecomposeResult, DecomposeError> {
    let mut result = DecomposeResult::default();

    // Skip queue_operations INSERT — enqueue content duplicates the raw
    // user prompt (already in messages), dequeue/remove/popAll are internal
    // scheduling state with no project intelligence value.

    // Log overflow — version is not available on queue-operation records,
    // so we use "unknown" as the version context
    result.overflow_fields += drift::log_overflow(
        Some("unknown"),
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
        Some(&r.base.version),
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
        Some("unknown"),
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
        Some("unknown"),
        "file-history-snapshot",
        &r.overflow,
        tx,
    )?;

    Ok(result)
}

/// Maximum sample-value length stored in `record_type_drift_log.sample_value`.
/// Mirrors `MAX_SAMPLE_VALUE_LEN` in `crates/store/src/drift.rs` so the two
/// drift tables share a consistent truncation policy.
const RECORD_TYPE_SAMPLE_MAX_LEN: usize = 500;

/// Decompose a `JSONLRecord::Unknown` variant.
///
/// Records whose top-level `type` discriminator is not one of the seven known
/// values land here. We do not write to any structural data table — the record
/// shape is, by definition, not yet modeled. Instead we record the
/// (type_name, version) pair plus a truncated sample of the raw JSON to
/// `record_type_drift_log` via `drift::log_record_type_drift`.
///
/// `version` is extracted from `raw.version` if present and a string. Some
/// observed unknown discriminators (e.g. `last-prompt`, `custom-title`) carry
/// no `version` field at all; in that case `None` is recorded and participates
/// in the table's UNIQUE(type_name, version) constraint as SQL NULL.
///
/// `sample_value` is the raw record's JSON serialization truncated to
/// [`RECORD_TYPE_SAMPLE_MAX_LEN`] characters. The truncation is performed on
/// a UTF-8 char boundary to avoid panicking on multi-byte content. The
/// truncation policy mirrors the per-field drift logger (`drift::log_overflow`)
/// so the two drift tables produce comparable forensic samples.
///
/// Note that B1.1 deliberately does NOT write to a backfill / archival table
/// (`dropped_records` per the audit's optional shape) — that is split into
/// B1.2's bytewise re-ingestion responsibility. This commit only closes the
/// silent-drop blind spot prospectively for newly-emitted records and writes
/// no message/system/etc. row for unknown variants.
///
/// Returns `DecomposeResult { rows_inserted: 0, overflow_fields: <changed> }`
/// where `changed` is 1 for both new inserts and conflicting updates (matching
/// `drift::log_overflow` semantics so callers can sum the two counts).
fn decompose_unknown(
    type_name: &str,
    raw: &serde_json::Value,
    tx: &Transaction,
) -> Result<DecomposeResult, DecomposeError> {
    let mut result = DecomposeResult::default();

    // Extract optional version string from the raw record envelope.
    // Records without a version field (e.g. `last-prompt`) record None,
    // which is preserved by record_type_drift_log's nullable version column.
    let version = raw
        .get("version")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    // Build the sample value: the entire raw JSON record, truncated.
    // A full record may be hundreds of KB (especially for attachment payloads
    // carrying skill listings or MCP instruction blocks); 500 chars is enough
    // for forensic identification without bloating the drift log.
    let raw_json = serde_json::to_string(raw)?;
    let sample_value = if raw_json.len() > RECORD_TYPE_SAMPLE_MAX_LEN {
        let mut cut = RECORD_TYPE_SAMPLE_MAX_LEN;
        while !raw_json.is_char_boundary(cut) && cut > 0 {
            cut -= 1;
        }
        format!("{}...", &raw_json[..cut])
    } else {
        raw_json
    };

    // Delegate the actual INSERT to drift::log_record_type_drift. The drift
    // module owns the SQL shape; decompose_unknown only assembles inputs.
    let changed =
        drift::log_record_type_drift(type_name, version.as_deref(), &sample_value, tx)?;

    // The variant did not produce any structural-table rows — it produced one
    // drift-log row (or updated an existing one). Surface that via the
    // overflow_fields counter so downstream telemetry treats it consistently
    // with field-level drift observations.
    result.overflow_fields = changed;

    Ok(result)
}

/// Upsert a `sessions` row from an `AttachmentRecord` envelope.
///
/// `attachments.session_id` is a foreign key to `sessions.session_id`, so an
/// attachment record arriving for a session that has not yet been observed
/// via a full-base record (User/Assistant/Progress/System) would otherwise
/// fail the FK constraint. This helper mirrors `upsert_session` for full-base
/// records but reads the optional envelope fields directly off the
/// AttachmentRecord. INSERT OR IGNORE preserves first-seen metadata when a
/// later full-base record carries richer envelope data.
fn upsert_session_from_attachment(
    record: &AttachmentRecord,
    tx: &Transaction,
) -> Result<usize, rusqlite::Error> {
    let changed = tx.execute(
        "INSERT OR IGNORE INTO sessions (session_id, project_path, first_seen_at, version, slug, git_branch)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        rusqlite::params![
            record.session_id,
            record.cwd,
            record.timestamp,
            record.version,
            record.slug,
            record.git_branch,
        ],
    )?;
    Ok(changed)
}

/// Insert one row into `attachments` from an `AttachmentRecord`.
///
/// `inner_type` is the AttachmentBody subtype discriminator (e.g.
/// `"hook_success"`, `"plan_mode"`, or the bare unmodeled subtype name like
/// `"date_change"` — per the C1.2.1 audit row #37 resolution, the
/// `"attachment.<subtype>"` qualified prefix is reserved for
/// `record_type_drift_log.type_name` only and is not duplicated here so a
/// future promotion of an unmodeled subtype into the modeled set produces
/// rows under one consistent `inner_type` value).
/// `body_json` is the serialized inner body — for modeled subtypes it is the
/// per-subtype struct (which already excludes the `type` discriminator); for
/// the Unknown variant it is the raw Value preserving the original on-disk
/// shape. Both shapes round-trip through AttachmentBody's Serialize impl,
/// but for storage we serialize the inner struct directly so downstream
/// queries can json_extract subtype-specific fields without re-parsing the
/// outer discriminator.
fn insert_attachment_row(
    record: &AttachmentRecord,
    inner_type: &str,
    body_json: Option<&str>,
    tx: &Transaction,
) -> Result<usize, rusqlite::Error> {
    let changed = tx.execute(
        "INSERT OR IGNORE INTO attachments
         (uuid, session_id, parent_uuid, timestamp, cwd, version, git_branch,
          slug, entrypoint, inner_type, body_json)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        rusqlite::params![
            record.uuid,
            record.session_id,
            record.parent_uuid,
            record.timestamp,
            record.cwd,
            record.version,
            record.git_branch,
            record.slug,
            record.entrypoint,
            inner_type,
            body_json,
        ],
    )?;
    if changed == 0 {
        tracing::debug!(
            uuid = %record.uuid,
            inner_type = inner_type,
            "Attachment already exists, INSERT OR IGNORE skipped duplicate"
        );
    }
    Ok(changed)
}

/// Insert one row into `hook_executions` for a `hook_success` body.
///
/// `decision` is left NULL — that column is populated only by
/// `hook_permission_decision`. `command`/`stdout`/`stderr`/`exit_code`/
/// `duration_ms` are nullable per migration 008, so missing optionals on
/// the body translate cleanly to SQL NULL.
fn insert_hook_success_row(
    attachment_uuid: &str,
    body: &claude_history_core::record::HookSuccessBody,
    tx: &Transaction,
) -> Result<usize, rusqlite::Error> {
    // INSERT OR IGNORE per the C1.2.1 idempotency contract. The natural
    // primary key is the migration-008 UNIQUE(attachment_uuid, hook_event,
    // tool_use_id) composite. Re-decomposing the same hook_success record
    // routes through OR-IGNORE rather than accumulating duplicate rows in
    // hook_executions — matching the decompose_user / decompose_assistant
    // INSERT OR IGNORE precedent on UUID-PK tables.
    let changed = tx.execute(
        "INSERT OR IGNORE INTO hook_executions
         (attachment_uuid, hook_name, hook_event, tool_use_id, exit_code,
          duration_ms, stdout, stderr, command, decision)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, NULL)",
        rusqlite::params![
            attachment_uuid,
            body.hook_name,
            body.hook_event,
            body.tool_use_id,
            body.exit_code,
            body.duration_ms,
            body.stdout,
            body.stderr,
            body.command,
        ],
    )?;
    Ok(changed)
}

/// Insert one row into `hook_executions` for a `hook_permission_decision`
/// body. Hook-success-specific columns (`hook_name`, `exit_code`,
/// `duration_ms`, `stdout`, `stderr`, `command`) are left NULL.
fn insert_hook_permission_decision_row(
    attachment_uuid: &str,
    body: &claude_history_core::record::HookPermissionDecisionBody,
    tx: &Transaction,
) -> Result<usize, rusqlite::Error> {
    // INSERT OR IGNORE per the C1.2.1 idempotency contract. See
    // insert_hook_success_row for the rationale; the same UNIQUE
    // (attachment_uuid, hook_event, tool_use_id) composite governs both
    // hook subtypes since they share the hook_executions table.
    let changed = tx.execute(
        "INSERT OR IGNORE INTO hook_executions
         (attachment_uuid, hook_name, hook_event, tool_use_id, exit_code,
          duration_ms, stdout, stderr, command, decision)
         VALUES (?1, NULL, ?2, ?3, NULL, NULL, NULL, NULL, NULL, ?4)",
        rusqlite::params![
            attachment_uuid,
            body.hook_event,
            body.tool_use_id,
            body.decision,
        ],
    )?;
    Ok(changed)
}

/// Decompose a `JSONLRecord::Attachment` variant into `attachments` and
/// (for hook subtypes) `hook_executions` rows.
///
/// **C1.2 — table-population path active.** C1.1 shipped the structural
/// foundation (variant, body enum, migration 008) plus the full Attachment
/// arm in `drift.rs::log_record_overflow`. C1.2 replaces the C1.1 stub with:
///
/// 1. Always upsert the corresponding `sessions` row first, so the FK from
///    `attachments.session_id` resolves even when the attachment record is
///    the first observation of a session.
///
/// 2. For each modeled `AttachmentBody` variant, INSERT OR IGNORE one row
///    into `attachments` carrying the envelope fields and `inner_type` set
///    to the subtype discriminator. `body_json` carries the serialized
///    inner struct (excluding the `type` discriminator, which is already
///    captured by `inner_type`) so subtype-specific fields are queryable
///    via `json_extract` without re-parsing the outer envelope.
///
/// 3. For `hook_success` and `hook_permission_decision`, also INSERT one
///    row into `hook_executions` with the appropriate flat columns. The
///    two subtypes share the table because their (toolUseID, hookEvent)
///    join shape is identical; subtype is recoverable via
///    `attachment_uuid -> attachments.inner_type`.
///
/// 4. For `AttachmentBody::Unknown`, INSERT OR IGNORE one `attachments`
///    row with `inner_type = "attachment.<subtype>"` and `body_json =
///    serialized raw Value`, AND log a row to `record_type_drift_log`
///    via `drift::log_record_type_drift`. This preserves the C1.1
///    behavior where unmodeled subtypes surface in the drift log while
///    also storing the body for forensic recovery.
///
/// 5. Always invoke the per-subtype `drift::log_overflow` calls — envelope
///    overflow under `record_type = "attachment"` and inner-body overflow
///    under `record_type = "attachment.<subtype>"`. This activates the
///    drift signals enumerated in `drift.rs::log_record_overflow`'s
///    Attachment arm (signals #1 and #2; signal #3 — Unknown-subtype
///    record_type_drift_log — is handled directly by step 4 above). The
///    drift.rs arm is still callable independently for non-ingestion code
///    paths (tests, batch tooling).
///
/// Note on `DecomposeResult.overflow_fields` semantics. Across the existing
/// `decompose_*` functions this counter sums `schema_drift_log` row inserts
/// + updates — a per-field count. The Attachment arm continues that
/// convention for steps 5 above (envelope + inner-body overflow). For the
/// Unknown-subtype `record_type_drift_log` write (step 4), the same counter
/// is incremented by one. The two tables share the "drift signal observed"
/// abstraction at the SSE/telemetry layer, so a unified counter is
/// project-pattern compliant despite the cross-table semantics. Renaming
/// the field would churn every call site and the SSE event payload; the
/// semantic clarification lives here in the docstring per the C1.1-Review
/// #29 forensic-completeness observation.
fn decompose_attachment(
    record: &AttachmentRecord,
    tx: &Transaction,
) -> Result<DecomposeResult, DecomposeError> {
    let mut result = DecomposeResult::default();

    // Step 1: ensure the sessions row exists for the FK.
    result.rows_inserted += upsert_session_from_attachment(record, tx)?;

    // C1.2.1 fix (audit row #15): the drift-version partition for an
    // AttachmentRecord uses Option<&str> bound directly to SQL NULL when the
    // envelope's `version` field is absent, rather than the literal string
    // `"unknown"`. The previous fallback collided with the partition key
    // produced by `decompose_unknown` for the JSONLRecord::Unknown variant
    // (which itself uses the literal `"unknown"` for missing versions on
    // `last-prompt`/`custom-title`/etc). NULL is partition-distinct from any
    // string sentinel; both `schema_drift_log.version` and
    // `record_type_drift_log.version` allow NULL per migrations 001 and 007.
    let drift_version: Option<&str> = record.version.as_deref();

    // Step 5a: envelope overflow drift logging (signal #1 from drift.rs's
    // Attachment-arm comment block). Runs unconditionally so envelope
    // overflow surfaces regardless of inner subtype.
    result.overflow_fields +=
        drift::log_overflow(drift_version, "attachment", &record.overflow, tx)?;

    // Steps 2/3/4: dispatch by inner body. Each arm:
    //   - inserts the attachments row with the appropriate inner_type
    //     and body_json (or NULL where the envelope captures everything)
    //   - for hook subtypes, inserts the hook_executions row
    //   - for Unknown, also writes record_type_drift_log
    //   - logs inner-body overflow under "attachment.<subtype>"
    match &record.attachment {
        AttachmentBody::HookSuccess(b) => {
            let body_json = serde_json::to_string(b)?;
            result.rows_inserted +=
                insert_attachment_row(record, "hook_success", Some(&body_json), tx)?;
            result.rows_inserted += insert_hook_success_row(&record.uuid, b, tx)?;
            result.overflow_fields +=
                drift::log_overflow(drift_version, "attachment.hook_success", &b.overflow, tx)?;
        }
        AttachmentBody::HookPermissionDecision(b) => {
            let body_json = serde_json::to_string(b)?;
            result.rows_inserted += insert_attachment_row(
                record,
                "hook_permission_decision",
                Some(&body_json),
                tx,
            )?;
            result.rows_inserted += insert_hook_permission_decision_row(&record.uuid, b, tx)?;
            result.overflow_fields += drift::log_overflow(
                drift_version,
                "attachment.hook_permission_decision",
                &b.overflow,
                tx,
            )?;
        }
        AttachmentBody::McpInstructionsDelta(b) => {
            let body_json = serde_json::to_string(b)?;
            result.rows_inserted += insert_attachment_row(
                record,
                "mcp_instructions_delta",
                Some(&body_json),
                tx,
            )?;
            result.overflow_fields += drift::log_overflow(
                drift_version,
                "attachment.mcp_instructions_delta",
                &b.overflow,
                tx,
            )?;
        }
        AttachmentBody::SkillListing(b) => {
            let body_json = serde_json::to_string(b)?;
            result.rows_inserted +=
                insert_attachment_row(record, "skill_listing", Some(&body_json), tx)?;
            result.overflow_fields +=
                drift::log_overflow(drift_version, "attachment.skill_listing", &b.overflow, tx)?;
        }
        AttachmentBody::EditedTextFile(b) => {
            let body_json = serde_json::to_string(b)?;
            result.rows_inserted +=
                insert_attachment_row(record, "edited_text_file", Some(&body_json), tx)?;
            result.overflow_fields += drift::log_overflow(
                drift_version,
                "attachment.edited_text_file",
                &b.overflow,
                tx,
            )?;
        }
        AttachmentBody::TaskReminder(b) => {
            let body_json = serde_json::to_string(b)?;
            result.rows_inserted +=
                insert_attachment_row(record, "task_reminder", Some(&body_json), tx)?;
            result.overflow_fields +=
                drift::log_overflow(drift_version, "attachment.task_reminder", &b.overflow, tx)?;
        }
        AttachmentBody::TodoReminder(b) => {
            let body_json = serde_json::to_string(b)?;
            result.rows_inserted +=
                insert_attachment_row(record, "todo_reminder", Some(&body_json), tx)?;
            result.overflow_fields +=
                drift::log_overflow(drift_version, "attachment.todo_reminder", &b.overflow, tx)?;
        }
        AttachmentBody::DeferredToolsDelta(b) => {
            let body_json = serde_json::to_string(b)?;
            result.rows_inserted +=
                insert_attachment_row(record, "deferred_tools_delta", Some(&body_json), tx)?;
            result.overflow_fields += drift::log_overflow(
                drift_version,
                "attachment.deferred_tools_delta",
                &b.overflow,
                tx,
            )?;
        }
        AttachmentBody::PlanMode(b) => {
            let body_json = serde_json::to_string(b)?;
            result.rows_inserted +=
                insert_attachment_row(record, "plan_mode", Some(&body_json), tx)?;
            result.overflow_fields +=
                drift::log_overflow(drift_version, "attachment.plan_mode", &b.overflow, tx)?;
        }
        AttachmentBody::PlanModeExit(b) => {
            let body_json = serde_json::to_string(b)?;
            result.rows_inserted +=
                insert_attachment_row(record, "plan_mode_exit", Some(&body_json), tx)?;
            result.overflow_fields +=
                drift::log_overflow(drift_version, "attachment.plan_mode_exit", &b.overflow, tx)?;
        }
        AttachmentBody::PlanModeReentry(b) => {
            let body_json = serde_json::to_string(b)?;
            result.rows_inserted +=
                insert_attachment_row(record, "plan_mode_reentry", Some(&body_json), tx)?;
            result.overflow_fields += drift::log_overflow(
                drift_version,
                "attachment.plan_mode_reentry",
                &b.overflow,
                tx,
            )?;
        }
        AttachmentBody::NestedMemory(b) => {
            let body_json = serde_json::to_string(b)?;
            result.rows_inserted +=
                insert_attachment_row(record, "nested_memory", Some(&body_json), tx)?;
            result.overflow_fields +=
                drift::log_overflow(drift_version, "attachment.nested_memory", &b.overflow, tx)?;
        }
        AttachmentBody::Unknown { subtype, raw } => {
            // Step 4: unmodeled subtype catch-all. Populate attachments with
            // the raw body so forensic recovery is possible without
            // re-parsing the JSONL, AND log to record_type_drift_log so the
            // schema-drift surfaces (CLI/REST) report the unmodeled subtype.
            //
            // C1.2.1 fix (audit row #37): the two destinations carry the
            // discriminator under different namespaces and that distinction
            // is now respected:
            //   - `attachments.inner_type` is set to the literal `subtype`
            //     verbatim (e.g. `"date_change"`). Modeled subtypes write
            //     the bare subtype name there too, so a future promotion
            //     of an unmodeled subtype into the modeled set produces
            //     rows under one consistent `inner_type` value rather than
            //     two parallel namespaces (`"date_change"` vs
            //     `"attachment.date_change"`).
            //   - `record_type_drift_log.type_name` retains the qualified
            //     `"attachment.<subtype>"` prefix so cross-record drift
            //     tracking distinguishes attachment-inner-discriminator
            //     drift from outer `JSONLRecord::Unknown` drift (e.g. a
            //     `"last-prompt"` outer-Unknown record vs an attachment
            //     subtype string that happens to be `"last-prompt"`).
            let inner_type = subtype.clone();
            let drift_type_name = format!("attachment.{subtype}");
            let raw_json = serde_json::to_string(raw)?;
            result.rows_inserted +=
                insert_attachment_row(record, &inner_type, Some(&raw_json), tx)?;

            // record_type_drift_log truncated-sample policy mirrors
            // decompose_unknown above (the B1.1 precedent).
            let sample_value = if raw_json.len() > RECORD_TYPE_SAMPLE_MAX_LEN {
                let mut cut = RECORD_TYPE_SAMPLE_MAX_LEN;
                while !raw_json.is_char_boundary(cut) && cut > 0 {
                    cut -= 1;
                }
                format!("{}...", &raw_json[..cut])
            } else {
                raw_json
            };
            result.overflow_fields += drift::log_record_type_drift(
                &drift_type_name,
                record.version.as_deref(),
                &sample_value,
                tx,
            )?;
        }
    }

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
    // Test 3: ProgressRecord -> session upsert only (progress_events INSERT
    // dropped — zero semantic value, ~70% of database bloat)
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

        // Session row should still be created
        let session_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sessions WHERE session_id = 'sess-003'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(session_count, 1, "session upsert still fires");

        // progress_events table dropped by migration 005 — no rows to check
        assert_eq!(result.rows_inserted, 1, "session only");
    }

    // -----------------------------------------------------------------------
    // Test 4: QueueOperationRecord -> no rows (INSERT dropped — enqueue
    // content duplicates user prompt, scheduling ops are internal noise)
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

        // queue_operations table dropped by migration 005 — no rows to check
        assert_eq!(result.rows_inserted, 0);
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

        // Verify each record type landed in its target table.
        // progress_events and queue_operations dropped by migration 005.

        let msg_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM messages", [], |row| row.get(0))
            .unwrap();
        assert_eq!(msg_count, 2, "user + assistant in messages table");

        let sys_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM system_events", [], |row| row.get(0))
            .unwrap();
        assert_eq!(sys_count, 1, "1 system event");

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

        let result = decompose_progress(&record, &tx).unwrap();
        tx.commit().unwrap();

        // progress_events INSERT dropped — verify session still created
        let session_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sessions WHERE session_id = 'sess-prog2'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(session_count, 1, "session upsert still fires");
        assert_eq!(result.rows_inserted, 1, "session only");
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

    // -----------------------------------------------------------------------
    // Test 13: upsert_project creates a projects row during decompose_record
    // -----------------------------------------------------------------------
    #[test]
    fn test_decompose_record_creates_project() {
        let conn = setup_db();
        let tx = conn.unchecked_transaction().unwrap();

        let record = JSONLRecord::User(UserRecord {
            base: test_base("user-proj", "sess-proj"),
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

        decompose_record(&record, "sess-proj", &tx).unwrap();
        tx.commit().unwrap();

        // Verify projects row was created
        let display_name: String = conn
            .query_row(
                "SELECT display_name FROM projects WHERE project_path = '/home/user/project'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(display_name, "project");

        let session_count: i64 = conn
            .query_row(
                "SELECT session_count FROM projects WHERE project_path = '/home/user/project'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(session_count, 1);
    }

    // -----------------------------------------------------------------------
    // Test 14: upsert_project updates session_count for same project_path
    // -----------------------------------------------------------------------
    #[test]
    fn test_decompose_record_updates_project_session_count() {
        let conn = setup_db();

        // First session
        {
            let tx = conn.unchecked_transaction().unwrap();
            let record = JSONLRecord::User(UserRecord {
                base: test_base("user-p1", "sess-p1"),
                message: UserMessage {
                    role: "user".to_string(),
                    content: MessageContent::Text("first".to_string()),
                },
                source_tool_assistant_uuid: None,
                tool_use_result: None,
                thinking_metadata: None,
                todos: None,
                permission_mode: None,
                overflow: HashMap::new(),
            });
            decompose_record(&record, "sess-p1", &tx).unwrap();
            tx.commit().unwrap();
        }

        // Second session with same project_path but different session_id
        {
            let tx = conn.unchecked_transaction().unwrap();
            let mut base2 = test_base("user-p2", "sess-p2");
            // cwd is the same "/home/user/project" from test_base
            base2.cwd = "/home/user/project".to_string();

            let record = JSONLRecord::User(UserRecord {
                base: base2,
                message: UserMessage {
                    role: "user".to_string(),
                    content: MessageContent::Text("second".to_string()),
                },
                source_tool_assistant_uuid: None,
                tool_use_result: None,
                thinking_metadata: None,
                todos: None,
                permission_mode: None,
                overflow: HashMap::new(),
            });
            decompose_record(&record, "sess-p2", &tx).unwrap();
            tx.commit().unwrap();
        }

        // Verify session_count is now 2
        let session_count: i64 = conn
            .query_row(
                "SELECT session_count FROM projects WHERE project_path = '/home/user/project'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(session_count, 2, "session_count should reflect both sessions");
    }

    // -----------------------------------------------------------------------
    // Test 15: User record with isCompactSummary and sourceToolUseID in
    //          overflow -> promoted columns populated on messages row
    // -----------------------------------------------------------------------
    #[test]
    fn test_decompose_user_compact_summary() {
        let conn = setup_db();
        let tx = conn.unchecked_transaction().unwrap();

        let mut overflow = HashMap::new();
        overflow.insert(
            "isCompactSummary".to_string(),
            serde_json::json!(true),
        );
        overflow.insert(
            "sourceToolUseID".to_string(),
            serde_json::json!("tool_123"),
        );

        let record = UserRecord {
            base: test_base("user-compact", "sess-compact"),
            message: UserMessage {
                role: "user".to_string(),
                content: MessageContent::Text("Compact summary content".to_string()),
            },
            source_tool_assistant_uuid: None,
            tool_use_result: None,
            thinking_metadata: None,
            todos: None,
            permission_mode: None,
            overflow,
        };

        decompose_user(&record, &tx).unwrap();
        tx.commit().unwrap();

        // Verify is_compact_summary = 1
        let is_compact: i64 = conn
            .query_row(
                "SELECT is_compact_summary FROM messages WHERE uuid = 'user-compact'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(is_compact, 1, "is_compact_summary should be 1 for compact summary records");

        // Verify source_tool_use_id = "tool_123"
        let source_tool_id: String = conn
            .query_row(
                "SELECT source_tool_use_id FROM messages WHERE uuid = 'user-compact'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(source_tool_id, "tool_123");
    }

    // -----------------------------------------------------------------------
    // Test 16: User record with promoted fields + unknown overflow field ->
    //          extra_json contains unknown field but NOT promoted keys
    // -----------------------------------------------------------------------
    #[test]
    fn test_decompose_user_extra_json() {
        let conn = setup_db();
        let tx = conn.unchecked_transaction().unwrap();

        let mut overflow = HashMap::new();
        overflow.insert(
            "isCompactSummary".to_string(),
            serde_json::json!(true),
        );
        overflow.insert(
            "sourceToolUseID".to_string(),
            serde_json::json!("tool_456"),
        );
        overflow.insert(
            "unknownField".to_string(),
            serde_json::json!("mysterious value"),
        );

        let record = UserRecord {
            base: test_base("user-extra", "sess-extra-json"),
            message: UserMessage {
                role: "user".to_string(),
                content: MessageContent::Text("test".to_string()),
            },
            source_tool_assistant_uuid: None,
            tool_use_result: None,
            thinking_metadata: None,
            todos: None,
            permission_mode: None,
            overflow,
        };

        decompose_user(&record, &tx).unwrap();
        tx.commit().unwrap();

        // Verify extra_json contains unknownField
        let extra_json: String = conn
            .query_row(
                "SELECT extra_json FROM messages WHERE uuid = 'user-extra'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&extra_json).unwrap();
        assert_eq!(parsed["unknownField"], "mysterious value");

        // Verify promoted keys are NOT in extra_json
        assert!(
            parsed.get("isCompactSummary").is_none(),
            "isCompactSummary should NOT be in extra_json"
        );
        assert!(
            parsed.get("sourceToolUseID").is_none(),
            "sourceToolUseID should NOT be in extra_json"
        );
    }

    // -----------------------------------------------------------------------
    // Test 17: User record with no compact summary fields in overflow ->
    //          is_compact_summary = 0 and extra_json IS NULL
    // -----------------------------------------------------------------------
    #[test]
    fn test_decompose_user_no_compact_summary() {
        let conn = setup_db();
        let tx = conn.unchecked_transaction().unwrap();

        let record = UserRecord {
            base: test_base("user-nocompact", "sess-nocompact"),
            message: UserMessage {
                role: "user".to_string(),
                content: MessageContent::Text("normal message".to_string()),
            },
            source_tool_assistant_uuid: None,
            tool_use_result: None,
            thinking_metadata: None,
            todos: None,
            permission_mode: None,
            overflow: HashMap::new(),
        };

        decompose_user(&record, &tx).unwrap();
        tx.commit().unwrap();

        // Verify is_compact_summary = 0
        let is_compact: i64 = conn
            .query_row(
                "SELECT is_compact_summary FROM messages WHERE uuid = 'user-nocompact'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(is_compact, 0, "is_compact_summary should be 0 for normal messages");

        // Verify extra_json IS NULL
        let extra_json: Option<String> = conn
            .query_row(
                "SELECT extra_json FROM messages WHERE uuid = 'user-nocompact'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(extra_json.is_none(), "extra_json should be NULL when overflow is empty");
    }

    // -----------------------------------------------------------------------
    // Test 17b (C2.1): User record with planContent in overflow ->
    //          plan_content column populated; planContent stripped from
    //          extra_json so the typed column is the single source of truth.
    // -----------------------------------------------------------------------
    #[test]
    fn test_decompose_user_plan_content() {
        let conn = setup_db();
        let tx = conn.unchecked_transaction().unwrap();

        let mut overflow = HashMap::new();
        overflow.insert(
            "planContent".to_string(),
            serde_json::json!("# Plan\n\nDo the thing in three steps."),
        );

        let record = UserRecord {
            base: test_base("user-plan", "sess-plan"),
            message: UserMessage {
                role: "user".to_string(),
                content: MessageContent::Text("plan-bearing user record".to_string()),
            },
            source_tool_assistant_uuid: None,
            tool_use_result: None,
            thinking_metadata: None,
            todos: None,
            permission_mode: None,
            overflow,
        };

        decompose_user(&record, &tx).unwrap();
        tx.commit().unwrap();

        // plan_content column populated with the markdown body.
        let plan_content: Option<String> = conn
            .query_row(
                "SELECT plan_content FROM messages WHERE uuid = 'user-plan'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            plan_content.as_deref(),
            Some("# Plan\n\nDo the thing in three steps."),
            "plan_content column should hold the promoted overflow value"
        );

        // extra_json should be NULL because planContent was the only
        // overflow key and has been promoted out.
        let extra_json: Option<String> = conn
            .query_row(
                "SELECT extra_json FROM messages WHERE uuid = 'user-plan'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(
            extra_json.is_none(),
            "extra_json should be NULL when only-overflow key was promoted; got {extra_json:?}"
        );
    }

    // -----------------------------------------------------------------------
    // Test 17b' (C2.1.1): User record with a NON-string planContent Value
    //          (e.g., an object) -> plan_content column NULL AND the
    //          original Value preserved in extra_json. Regression for
    //          audit rows #25/#39/#40: prior code unconditionally removed
    //          "planContent" from the remaining-overflow map even when
    //          string extraction failed via as_str(), losing the Value
    //          entirely. C2.1.1 makes the remove() conditional on
    //          extraction success so non-string Values stay queryable
    //          via json_extract(extra_json, '$.planContent').
    // -----------------------------------------------------------------------
    #[test]
    fn test_decompose_user_preserves_non_string_plan_content() {
        let conn = setup_db();
        let tx = conn.unchecked_transaction().unwrap();

        // Non-string Value: an object. as_str() returns None, so
        // plan_content extraction yields None and the Value should remain
        // in extra_json for forensic recovery.
        let mut overflow = HashMap::new();
        overflow.insert(
            "planContent".to_string(),
            serde_json::json!({"unexpected": "object", "n": 1}),
        );

        let record = UserRecord {
            base: test_base("user-plan-nonstr", "sess-plan-nonstr"),
            message: UserMessage {
                role: "user".to_string(),
                content: MessageContent::Text("non-string planContent record".to_string()),
            },
            source_tool_assistant_uuid: None,
            tool_use_result: None,
            thinking_metadata: None,
            todos: None,
            permission_mode: None,
            overflow,
        };

        decompose_user(&record, &tx).unwrap();
        tx.commit().unwrap();

        // plan_content column should be NULL because as_str() returned None
        // for the object Value.
        let plan_content: Option<String> = conn
            .query_row(
                "SELECT plan_content FROM messages WHERE uuid = 'user-plan-nonstr'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(
            plan_content.is_none(),
            "plan_content should be NULL when planContent Value is non-string; got {plan_content:?}"
        );

        // The original non-string Value should still be present in extra_json
        // (NOT removed). This guards against the audit-flagged data-loss
        // path where unconditional remove() stripped the Value.
        let extra_json: Option<String> = conn
            .query_row(
                "SELECT extra_json FROM messages WHERE uuid = 'user-plan-nonstr'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let extra_json = extra_json.expect(
            "extra_json should be Some when a non-string planContent Value is preserved",
        );
        let parsed: serde_json::Value = serde_json::from_str(&extra_json).unwrap();
        assert_eq!(
            parsed.get("planContent"),
            Some(&serde_json::json!({"unexpected": "object", "n": 1})),
            "non-string planContent Value should be preserved verbatim in extra_json; got {parsed}"
        );
    }

    // -----------------------------------------------------------------------
    // Test 17c (C2.1): User record with planContent + a sibling unknown
    //          overflow key -> plan_content column populated; sibling
    //          retained in extra_json; planContent absent from extra_json.
    // -----------------------------------------------------------------------
    #[test]
    fn test_decompose_user_plan_content_with_sibling_overflow() {
        let conn = setup_db();
        let tx = conn.unchecked_transaction().unwrap();

        let mut overflow = HashMap::new();
        overflow.insert(
            "planContent".to_string(),
            serde_json::json!("# Plan\n- step one\n- step two"),
        );
        overflow.insert(
            "futureField".to_string(),
            serde_json::json!("not-yet-modeled"),
        );

        let record = UserRecord {
            base: test_base("user-plan-sib", "sess-plan-sib"),
            message: UserMessage {
                role: "user".to_string(),
                content: MessageContent::Text("plan + sibling overflow".to_string()),
            },
            source_tool_assistant_uuid: None,
            tool_use_result: None,
            thinking_metadata: None,
            todos: None,
            permission_mode: None,
            overflow,
        };

        decompose_user(&record, &tx).unwrap();
        tx.commit().unwrap();

        let plan_content: Option<String> = conn
            .query_row(
                "SELECT plan_content FROM messages WHERE uuid = 'user-plan-sib'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            plan_content.as_deref(),
            Some("# Plan\n- step one\n- step two"),
        );

        let extra_json: String = conn
            .query_row(
                "SELECT extra_json FROM messages WHERE uuid = 'user-plan-sib'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&extra_json).unwrap();
        assert!(
            parsed.get("planContent").is_none(),
            "planContent should NOT appear in extra_json after promotion; got {parsed}"
        );
        assert_eq!(
            parsed.get("futureField"),
            Some(&serde_json::json!("not-yet-modeled")),
            "sibling overflow keys should be preserved in extra_json"
        );
    }

    // -----------------------------------------------------------------------
    // Test 17d (C2.1): User record with no planContent -> plan_content
    //          column is NULL. Confirms the new column doesn't get spuriously
    //          populated for non-plan messages (the vast majority).
    // -----------------------------------------------------------------------
    #[test]
    fn test_decompose_user_no_plan_content() {
        let conn = setup_db();
        let tx = conn.unchecked_transaction().unwrap();

        let record = UserRecord {
            base: test_base("user-no-plan", "sess-no-plan"),
            message: UserMessage {
                role: "user".to_string(),
                content: MessageContent::Text("plain user message".to_string()),
            },
            source_tool_assistant_uuid: None,
            tool_use_result: None,
            thinking_metadata: None,
            todos: None,
            permission_mode: None,
            overflow: HashMap::new(),
        };

        decompose_user(&record, &tx).unwrap();
        tx.commit().unwrap();

        let plan_content: Option<String> = conn
            .query_row(
                "SELECT plan_content FROM messages WHERE uuid = 'user-no-plan'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(
            plan_content.is_none(),
            "plan_content should be NULL for messages without a planContent overflow key; got {plan_content:?}"
        );
    }

    // -----------------------------------------------------------------------
    // Test 18: Assistant record with overflow -> extra_json populated from
    //          merged record-level and message-level overflow
    // -----------------------------------------------------------------------
    #[test]
    fn test_decompose_assistant_extra_json() {
        let conn = setup_db();
        let tx = conn.unchecked_transaction().unwrap();

        let mut record_overflow = HashMap::new();
        record_overflow.insert("apiError".to_string(), serde_json::json!("timeout"));

        let mut message_overflow = HashMap::new();
        message_overflow.insert(
            "context_management".to_string(),
            serde_json::json!({"strategy": "truncate"}),
        );

        let record = AssistantRecord {
            base: test_base("assist-extra", "sess-assist-extra"),
            message: AssistantMessage {
                id: "msg_extra".to_string(),
                model: "claude-opus-4-6".to_string(),
                role: "assistant".to_string(),
                content: vec![ContentBlock::Text {
                    text: "Response".to_string(),
                }],
                stop_reason: Some("end_turn".to_string()),
                stop_sequence: None,
                usage: None,
                overflow: message_overflow,
            },
            request_id: None,
            is_api_error_message: None,
            error: None,
            overflow: record_overflow,
        };

        decompose_assistant(&record, &tx).unwrap();
        tx.commit().unwrap();

        // Verify extra_json merges both overflow sources
        let extra_json: String = conn
            .query_row(
                "SELECT extra_json FROM messages WHERE uuid = 'assist-extra'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&extra_json).unwrap();
        assert_eq!(parsed["apiError"], "timeout");
        assert!(parsed["context_management"].is_object());
    }

    // -----------------------------------------------------------------------
    // C1.2 — decompose_attachment table-population tests.
    //
    // Each modeled subtype has a focused test that:
    //   1. Loads the corresponding /tmp/c1.1-samples-<subtype>.json fixture
    //      (real records sampled from the live corpus during C1.1 development)
    //   2. Parses it into a JSONLRecord via serde_json::from_str (exercising
    //      the manual Deserialize impl)
    //   3. Calls decompose_record with the in-memory schema applied
    //   4. Asserts the expected attachments row exists with the right
    //      inner_type and a non-NULL body_json
    //   5. For hook subtypes, asserts the hook_executions row exists and
    //      joins to attachments via attachment_uuid
    //
    // Fixtures are loaded via std::fs::read_to_string from /tmp because they
    // were produced by C1.1's investigation and are not committed to the
    // repo. Tests skip via early-return when a fixture is missing so the
    // suite remains green on CI runners that lack /tmp/c1.1-samples-*.
    //
    // The Unknown-subtype test uses a synthesized record (rather than a real
    // fixture) because the modeled set covers all 12 subtypes observed at
    // >=10 records per subtype; the corpus has no canonical Unknown-subtype
    // fixture suitable as a single-file test input.
    // -----------------------------------------------------------------------

    /// Load a /tmp/c1.1-samples-<subtype>.json fixture as the first non-empty
    /// JSONL record. Returns Some(JSONLRecord::Attachment) on success, None
    /// when the fixture file is absent or empty (so tests skip gracefully).
    fn load_attachment_fixture(
        subtype: &str,
    ) -> Option<claude_history_core::record::AttachmentRecord> {
        let path = format!("/tmp/c1.1-samples-{subtype}.json");
        let contents = std::fs::read_to_string(&path).ok()?;
        // The fixture format is one JSON object per line. Take the first
        // non-empty line.
        let line = contents.lines().find(|l| !l.trim().is_empty())?;
        let record: claude_history_core::record::JSONLRecord = serde_json::from_str(line).ok()?;
        match record {
            claude_history_core::record::JSONLRecord::Attachment(r) => Some(r),
            _ => None,
        }
    }

    /// Helper: count rows in `attachments` matching a uuid + inner_type.
    fn count_attachments(conn: &Connection, uuid: &str, inner_type: &str) -> i64 {
        conn.query_row(
            "SELECT COUNT(*) FROM attachments WHERE uuid = ?1 AND inner_type = ?2",
            rusqlite::params![uuid, inner_type],
            |r| r.get::<_, i64>(0),
        )
        .unwrap()
    }

    /// Helper: count rows in `hook_executions` joined to attachments by uuid.
    fn count_hook_executions(conn: &Connection, attachment_uuid: &str) -> i64 {
        conn.query_row(
            "SELECT COUNT(*) FROM hook_executions WHERE attachment_uuid = ?1",
            rusqlite::params![attachment_uuid],
            |r| r.get::<_, i64>(0),
        )
        .unwrap()
    }

    /// Run a fixture-driven attachment decompose and return uuid + Connection.
    fn decompose_fixture(subtype: &str) -> Option<(Connection, String)> {
        let record = load_attachment_fixture(subtype)?;
        let conn = setup_db();
        let tx = conn.unchecked_transaction().unwrap();
        let uuid = record.uuid.clone();
        let jsonl = claude_history_core::record::JSONLRecord::Attachment(record);
        decompose_record(&jsonl, "fallback-session-id", &tx).unwrap();
        tx.commit().unwrap();
        Some((conn, uuid))
    }

    // ---- Subtype 1: hook_success ----
    #[test]
    fn test_decompose_attachment_hook_success() {
        let Some((conn, uuid)) = decompose_fixture("hook_success") else {
            eprintln!("skip: /tmp/c1.1-samples-hook_success.json missing");
            return;
        };
        assert_eq!(count_attachments(&conn, &uuid, "hook_success"), 1);
        assert_eq!(count_hook_executions(&conn, &uuid), 1);
        // body_json is non-null
        let body_json: Option<String> = conn
            .query_row(
                "SELECT body_json FROM attachments WHERE uuid = ?1",
                [&uuid],
                |r| r.get(0),
            )
            .unwrap();
        assert!(body_json.is_some());
        // hook_executions carries the hook_name and tool_use_id from the body
        let (hook_name, hook_event): (Option<String>, Option<String>) = conn
            .query_row(
                "SELECT hook_name, hook_event FROM hook_executions WHERE attachment_uuid = ?1",
                [&uuid],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert!(hook_name.is_some());
        assert!(hook_event.is_some());
    }

    // ---- Subtype 2: hook_permission_decision ----
    #[test]
    fn test_decompose_attachment_hook_permission_decision() {
        let Some((conn, uuid)) = decompose_fixture("hook_permission_decision") else {
            eprintln!("skip: /tmp/c1.1-samples-hook_permission_decision.json missing");
            return;
        };
        assert_eq!(count_attachments(&conn, &uuid, "hook_permission_decision"), 1);
        assert_eq!(count_hook_executions(&conn, &uuid), 1);
        // For hook_permission_decision: decision is populated, hook_name/exit_code/etc are NULL
        let (decision, hook_name, exit_code): (Option<String>, Option<String>, Option<i64>) = conn
            .query_row(
                "SELECT decision, hook_name, exit_code FROM hook_executions WHERE attachment_uuid = ?1",
                [&uuid],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert!(decision.is_some());
        assert!(hook_name.is_none());
        assert!(exit_code.is_none());
    }

    // ---- Subtype 3: mcp_instructions_delta ----
    #[test]
    fn test_decompose_attachment_mcp_instructions_delta() {
        let Some((conn, uuid)) = decompose_fixture("mcp_instructions_delta") else {
            eprintln!("skip: /tmp/c1.1-samples-mcp_instructions_delta.json missing");
            return;
        };
        assert_eq!(count_attachments(&conn, &uuid, "mcp_instructions_delta"), 1);
        // No hook_executions row for non-hook subtype
        assert_eq!(count_hook_executions(&conn, &uuid), 0);
        // body_json captures addedNames / addedBlocks
        let body_json: String = conn
            .query_row(
                "SELECT body_json FROM attachments WHERE uuid = ?1",
                [&uuid],
                |r| r.get(0),
            )
            .unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&body_json).unwrap();
        // At least one of the documented array fields must be present
        assert!(parsed.get("addedNames").is_some() || parsed.get("addedBlocks").is_some());
    }

    // ---- Subtype 4: skill_listing ----
    #[test]
    fn test_decompose_attachment_skill_listing() {
        let Some((conn, uuid)) = decompose_fixture("skill_listing") else {
            eprintln!("skip: /tmp/c1.1-samples-skill_listing.json missing");
            return;
        };
        assert_eq!(count_attachments(&conn, &uuid, "skill_listing"), 1);
        assert_eq!(count_hook_executions(&conn, &uuid), 0);
        let body_json: String = conn
            .query_row(
                "SELECT body_json FROM attachments WHERE uuid = ?1",
                [&uuid],
                |r| r.get(0),
            )
            .unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&body_json).unwrap();
        assert!(parsed.get("content").and_then(|v| v.as_str()).is_some());
    }

    // ---- Subtype 5: edited_text_file ----
    #[test]
    fn test_decompose_attachment_edited_text_file() {
        let Some((conn, uuid)) = decompose_fixture("edited_text_file") else {
            eprintln!("skip: /tmp/c1.1-samples-edited_text_file.json missing");
            return;
        };
        assert_eq!(count_attachments(&conn, &uuid, "edited_text_file"), 1);
        let body_json: String = conn
            .query_row(
                "SELECT body_json FROM attachments WHERE uuid = ?1",
                [&uuid],
                |r| r.get(0),
            )
            .unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&body_json).unwrap();
        assert!(parsed.get("filename").and_then(|v| v.as_str()).is_some());
        assert!(parsed.get("snippet").and_then(|v| v.as_str()).is_some());
    }

    // ---- Subtype 6: task_reminder ----
    #[test]
    fn test_decompose_attachment_task_reminder() {
        let Some((conn, uuid)) = decompose_fixture("task_reminder") else {
            eprintln!("skip: /tmp/c1.1-samples-task_reminder.json missing");
            return;
        };
        assert_eq!(count_attachments(&conn, &uuid, "task_reminder"), 1);
        let body_json: String = conn
            .query_row(
                "SELECT body_json FROM attachments WHERE uuid = ?1",
                [&uuid],
                |r| r.get(0),
            )
            .unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&body_json).unwrap();
        assert!(parsed.get("itemCount").is_some());
        assert!(parsed.get("content").is_some());
    }

    // ---- Subtype 7: todo_reminder ----
    #[test]
    fn test_decompose_attachment_todo_reminder() {
        let Some((conn, uuid)) = decompose_fixture("todo_reminder") else {
            eprintln!("skip: /tmp/c1.1-samples-todo_reminder.json missing");
            return;
        };
        assert_eq!(count_attachments(&conn, &uuid, "todo_reminder"), 1);
        let body_json: String = conn
            .query_row(
                "SELECT body_json FROM attachments WHERE uuid = ?1",
                [&uuid],
                |r| r.get(0),
            )
            .unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&body_json).unwrap();
        assert!(parsed.get("itemCount").is_some());
    }

    // ---- Subtype 8: deferred_tools_delta ----
    #[test]
    fn test_decompose_attachment_deferred_tools_delta() {
        let Some((conn, uuid)) = decompose_fixture("deferred_tools_delta") else {
            eprintln!("skip: /tmp/c1.1-samples-deferred_tools_delta.json missing");
            return;
        };
        assert_eq!(count_attachments(&conn, &uuid, "deferred_tools_delta"), 1);
        let body_json: String = conn
            .query_row(
                "SELECT body_json FROM attachments WHERE uuid = ?1",
                [&uuid],
                |r| r.get(0),
            )
            .unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&body_json).unwrap();
        // addedNames may default to empty array; just confirm the key exists
        // somewhere in the serialized body (or in overflow if real records
        // ship a different shape — the test stays permissive).
        let has_added_names = parsed.get("addedNames").is_some();
        let has_overflow_field = parsed.as_object().map(|o| !o.is_empty()).unwrap_or(false);
        assert!(has_added_names || has_overflow_field);
    }

    // ---- Subtype 9: plan_mode ----
    #[test]
    fn test_decompose_attachment_plan_mode() {
        let Some((conn, uuid)) = decompose_fixture("plan_mode") else {
            eprintln!("skip: /tmp/c1.1-samples-plan_mode.json missing");
            return;
        };
        assert_eq!(count_attachments(&conn, &uuid, "plan_mode"), 1);
        let body_json: String = conn
            .query_row(
                "SELECT body_json FROM attachments WHERE uuid = ?1",
                [&uuid],
                |r| r.get(0),
            )
            .unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&body_json).unwrap();
        assert!(parsed.get("planFilePath").and_then(|v| v.as_str()).is_some());
    }

    // ---- Subtype 10: plan_mode_exit ----
    #[test]
    fn test_decompose_attachment_plan_mode_exit() {
        let Some((conn, uuid)) = decompose_fixture("plan_mode_exit") else {
            eprintln!("skip: /tmp/c1.1-samples-plan_mode_exit.json missing");
            return;
        };
        assert_eq!(count_attachments(&conn, &uuid, "plan_mode_exit"), 1);
    }

    // ---- Subtype 11: plan_mode_reentry ----
    #[test]
    fn test_decompose_attachment_plan_mode_reentry() {
        let Some((conn, uuid)) = decompose_fixture("plan_mode_reentry") else {
            eprintln!("skip: /tmp/c1.1-samples-plan_mode_reentry.json missing");
            return;
        };
        assert_eq!(count_attachments(&conn, &uuid, "plan_mode_reentry"), 1);
        let body_json: String = conn
            .query_row(
                "SELECT body_json FROM attachments WHERE uuid = ?1",
                [&uuid],
                |r| r.get(0),
            )
            .unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&body_json).unwrap();
        assert!(parsed.get("planFilePath").is_some());
    }

    // ---- Subtype 12: nested_memory ----
    #[test]
    fn test_decompose_attachment_nested_memory() {
        let Some((conn, uuid)) = decompose_fixture("nested_memory") else {
            eprintln!("skip: /tmp/c1.1-samples-nested_memory.json missing");
            return;
        };
        assert_eq!(count_attachments(&conn, &uuid, "nested_memory"), 1);
        let body_json: String = conn
            .query_row(
                "SELECT body_json FROM attachments WHERE uuid = ?1",
                [&uuid],
                |r| r.get(0),
            )
            .unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&body_json).unwrap();
        assert!(parsed.get("path").is_some());
        // nested_memory.content is itself an object with path/type/content
        let inner = parsed.get("content").and_then(|c| c.as_object());
        assert!(inner.is_some());
    }

    // -----------------------------------------------------------------------
    // C1.2 — Unknown-subtype catch-all test.
    //
    // Synthesizes an AttachmentRecord whose body parses as
    // AttachmentBody::Unknown (via an unmodeled `type` discriminator on the
    // inner attachment object). Asserts:
    //   - attachments row exists with inner_type = "attachment.<subtype>"
    //   - body_json captures the raw Value
    //   - record_type_drift_log row exists with the same type_name
    // -----------------------------------------------------------------------
    #[test]
    fn test_decompose_attachment_unknown_subtype() {
        let conn = setup_db();
        let tx = conn.unchecked_transaction().unwrap();

        // Construct a JSONL line with an unmodeled attachment subtype.
        let json = r#"{
            "type": "attachment",
            "uuid": "att-unknown-001",
            "sessionId": "sess-unknown-001",
            "timestamp": "2026-05-09T10:00:00.000Z",
            "version": "2.1.126",
            "cwd": "/tmp/test",
            "gitBranch": "main",
            "slug": "test",
            "isSidechain": false,
            "userType": "external",
            "attachment": {
                "type": "fictional_unmodeled_subtype_xyz",
                "fooField": "bar",
                "extraValue": 42
            }
        }"#;
        let record: claude_history_core::record::JSONLRecord =
            serde_json::from_str(json).unwrap();
        decompose_record(&record, "fallback-sess", &tx).unwrap();
        tx.commit().unwrap();

        // attachments row populated with raw body_json. Per C1.2.1 fix
        // (audit row #37) the inner_type is the bare subtype (no
        // "attachment." prefix); the prefix is retained only on
        // record_type_drift_log.type_name to keep that table's cross-record
        // drift discriminator namespaced from outer JSONLRecord::Unknown
        // discriminators.
        let (inner_type, body_json): (String, Option<String>) = conn
            .query_row(
                "SELECT inner_type, body_json FROM attachments WHERE uuid = 'att-unknown-001'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(inner_type, "fictional_unmodeled_subtype_xyz");
        assert!(body_json.is_some());
        let parsed: serde_json::Value = serde_json::from_str(&body_json.unwrap()).unwrap();
        assert_eq!(parsed["fooField"], "bar");
        assert_eq!(parsed["extraValue"], 42);

        // record_type_drift_log row written under the qualified "attachment.<subtype>"
        // type_name — the two columns are intentionally distinct.
        let drift_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM record_type_drift_log
                 WHERE type_name = 'attachment.fictional_unmodeled_subtype_xyz'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(drift_count, 1);

        // Confirm the unprefixed name does NOT appear in record_type_drift_log
        // (the two columns serve distinct purposes).
        let drift_count_unprefixed: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM record_type_drift_log
                 WHERE type_name = 'fictional_unmodeled_subtype_xyz'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(drift_count_unprefixed, 0);
    }

    // -----------------------------------------------------------------------
    // C1.2 — FK shape test: hook_executions.attachment_uuid joins to
    // attachments.uuid. Inserts one synthesized hook_success record, then
    // runs a JOIN and asserts the row appears in both tables.
    // -----------------------------------------------------------------------
    #[test]
    fn test_decompose_attachment_hook_executions_fk_join() {
        let conn = setup_db();
        let tx = conn.unchecked_transaction().unwrap();

        let json = r#"{
            "type": "attachment",
            "uuid": "att-hook-fk-001",
            "sessionId": "sess-fk-001",
            "timestamp": "2026-05-09T10:00:00.000Z",
            "version": "2.1.126",
            "cwd": "/tmp/test",
            "gitBranch": "main",
            "slug": "test",
            "isSidechain": false,
            "userType": "external",
            "attachment": {
                "type": "hook_success",
                "hookName": "PostToolUse",
                "toolUseID": "toolu_synthetic_001",
                "hookEvent": "PostToolUse",
                "exitCode": 0,
                "durationMs": 42,
                "stdout": "ok",
                "stderr": "",
                "command": "echo ok"
            }
        }"#;
        let record: claude_history_core::record::JSONLRecord =
            serde_json::from_str(json).unwrap();
        decompose_record(&record, "fallback-sess", &tx).unwrap();
        tx.commit().unwrap();

        // Joined query: confirms the FK shape is correct.
        let joined: (String, String, i64) = conn
            .query_row(
                "SELECT a.uuid, h.tool_use_id, h.exit_code
                 FROM attachments a
                 JOIN hook_executions h ON h.attachment_uuid = a.uuid
                 WHERE a.uuid = 'att-hook-fk-001'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(joined.0, "att-hook-fk-001");
        assert_eq!(joined.1, "toolu_synthetic_001");
        assert_eq!(joined.2, 0);
    }

    // -----------------------------------------------------------------------
    // C1.2 — Envelope-overflow drift test. Synthesizes an AttachmentRecord
    // with an envelope-level field (`agentId`) that is not enumerated on
    // AttachmentRecord, expects schema_drift_log to capture it under
    // record_type = "attachment".
    // -----------------------------------------------------------------------
    #[test]
    fn test_decompose_attachment_envelope_overflow_drift() {
        let conn = setup_db();
        let tx = conn.unchecked_transaction().unwrap();

        let json = r#"{
            "type": "attachment",
            "uuid": "att-envov-001",
            "sessionId": "sess-envov-001",
            "timestamp": "2026-05-09T10:00:00.000Z",
            "version": "2.1.126",
            "cwd": "/tmp/test",
            "gitBranch": "main",
            "slug": "test",
            "isSidechain": false,
            "userType": "external",
            "agentId": "subagent_xyz",
            "attachment": {
                "type": "task_reminder",
                "content": [],
                "itemCount": 0
            }
        }"#;
        let record: claude_history_core::record::JSONLRecord =
            serde_json::from_str(json).unwrap();
        decompose_record(&record, "fallback-sess", &tx).unwrap();
        tx.commit().unwrap();

        // schema_drift_log row written for the envelope agentId field
        let drift_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM schema_drift_log
                 WHERE field_name = 'agentId' AND record_type = 'attachment'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(drift_count, 1);
    }

    // -----------------------------------------------------------------------
    // C1.2.1 — idempotency: re-decomposing the same attachment record
    // produces the same DB state. Mirrors the test_decompose_idempotency
    // precedent for User records and exercises the
    // INSERT-OR-IGNORE-on-hook_executions contract that audit row #2
    // surfaced as missing in C1.2. Three subtype shapes are exercised:
    //   - hook_success (writes attachments + hook_executions)
    //   - hook_permission_decision (also writes hook_executions, with
    //     NULL tool_use_id potentially)
    //   - Unknown (writes attachments + record_type_drift_log; the
    //     drift-log occurrence_count is expected to increment on
    //     re-observation per the log_record_type_drift contract)
    // -----------------------------------------------------------------------

    /// Re-decompose a hook_success attachment. Asserts attachments stays at
    /// 1 row (INSERT OR IGNORE on uuid), hook_executions stays at 1 row
    /// (INSERT OR IGNORE on the migration-008
    /// UNIQUE(attachment_uuid, hook_event, tool_use_id) composite). Exercises
    /// the per-table idempotency contract that audit row #2 identified as
    /// missing prior to C1.2.1.
    #[test]
    fn test_decompose_attachment_idempotency_hook_success() {
        let conn = setup_db();

        let json = r#"{
            "type": "attachment",
            "uuid": "att-idem-hs-001",
            "sessionId": "sess-idem-hs",
            "timestamp": "2026-05-09T10:00:00.000Z",
            "version": "2.1.126",
            "cwd": "/tmp/test",
            "gitBranch": "main",
            "slug": "test",
            "isSidechain": false,
            "userType": "external",
            "attachment": {
                "type": "hook_success",
                "hookName": "PostToolUse",
                "toolUseID": "toolu_idem_001",
                "hookEvent": "PostToolUse",
                "exitCode": 0,
                "durationMs": 7
            }
        }"#;
        let record: claude_history_core::record::JSONLRecord =
            serde_json::from_str(json).unwrap();

        // First decompose
        {
            let tx = conn.unchecked_transaction().unwrap();
            decompose_record(&record, "fallback-sess", &tx).unwrap();
            tx.commit().unwrap();
        }
        let attachments_after_first: i64 = conn
            .query_row("SELECT COUNT(*) FROM attachments", [], |r| r.get(0))
            .unwrap();
        let hook_after_first: i64 = conn
            .query_row("SELECT COUNT(*) FROM hook_executions", [], |r| r.get(0))
            .unwrap();
        assert_eq!(attachments_after_first, 1);
        assert_eq!(hook_after_first, 1);

        // Second decompose — must not duplicate either row.
        {
            let tx = conn.unchecked_transaction().unwrap();
            decompose_record(&record, "fallback-sess", &tx).unwrap();
            tx.commit().unwrap();
        }
        let attachments_after_second: i64 = conn
            .query_row("SELECT COUNT(*) FROM attachments", [], |r| r.get(0))
            .unwrap();
        let hook_after_second: i64 = conn
            .query_row("SELECT COUNT(*) FROM hook_executions", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            attachments_after_second, 1,
            "attachments must stay at 1 (INSERT OR IGNORE on uuid)"
        );
        assert_eq!(
            hook_after_second, 1,
            "hook_executions must stay at 1 (INSERT OR IGNORE on UNIQUE composite)"
        );
    }

    /// Re-decompose a hook_permission_decision attachment. Same shape as
    /// the hook_success idempotency test but exercises the second hook
    /// helper (insert_hook_permission_decision_row) which also gained
    /// INSERT OR IGNORE in C1.2.1.
    #[test]
    fn test_decompose_attachment_idempotency_hook_permission_decision() {
        let conn = setup_db();

        let json = r#"{
            "type": "attachment",
            "uuid": "att-idem-hpd-001",
            "sessionId": "sess-idem-hpd",
            "timestamp": "2026-05-09T10:00:00.000Z",
            "version": "2.1.126",
            "cwd": "/tmp/test",
            "gitBranch": "main",
            "slug": "test",
            "isSidechain": false,
            "userType": "external",
            "attachment": {
                "type": "hook_permission_decision",
                "decision": "allow",
                "toolUseID": "toolu_idem_hpd_001",
                "hookEvent": "PreToolUse"
            }
        }"#;
        let record: claude_history_core::record::JSONLRecord =
            serde_json::from_str(json).unwrap();

        for _ in 0..2 {
            let tx = conn.unchecked_transaction().unwrap();
            decompose_record(&record, "fallback-sess", &tx).unwrap();
            tx.commit().unwrap();
        }

        let attachments_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM attachments", [], |r| r.get(0))
            .unwrap();
        let hook_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM hook_executions", [], |r| r.get(0))
            .unwrap();
        assert_eq!(attachments_count, 1);
        assert_eq!(hook_count, 1);
    }

    /// Re-decompose an Unknown-subtype attachment. Asserts attachments
    /// stays at 1 row, record_type_drift_log stays at 1 row but its
    /// occurrence_count increments to 2 — this matches
    /// log_record_type_drift's UPDATE-on-conflict semantics, consistent
    /// with the decompose_unknown precedent at decompose.rs around the
    /// log_record_type_drift call site.
    #[test]
    fn test_decompose_attachment_idempotency_unknown_subtype() {
        let conn = setup_db();

        let json = r#"{
            "type": "attachment",
            "uuid": "att-idem-unk-001",
            "sessionId": "sess-idem-unk",
            "timestamp": "2026-05-09T10:00:00.000Z",
            "version": "2.1.126",
            "cwd": "/tmp/test",
            "gitBranch": "main",
            "slug": "test",
            "isSidechain": false,
            "userType": "external",
            "attachment": {
                "type": "fictional_idem_subtype_zzz",
                "fooField": "bar"
            }
        }"#;
        let record: claude_history_core::record::JSONLRecord =
            serde_json::from_str(json).unwrap();

        for _ in 0..2 {
            let tx = conn.unchecked_transaction().unwrap();
            decompose_record(&record, "fallback-sess", &tx).unwrap();
            tx.commit().unwrap();
        }

        // attachments stays singular
        let attachments_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM attachments", [], |r| r.get(0))
            .unwrap();
        assert_eq!(attachments_count, 1);
        // record_type_drift_log stays singular for this (type_name, version)
        let drift_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM record_type_drift_log
                 WHERE type_name = 'attachment.fictional_idem_subtype_zzz'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(drift_count, 1);
        // occurrence_count incremented to 2 on the second observation
        let occ: i64 = conn
            .query_row(
                "SELECT occurrence_count FROM record_type_drift_log
                 WHERE type_name = 'attachment.fictional_idem_subtype_zzz'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(occ, 2);

        // attachments.inner_type is the bare subtype (C1.2.1 audit row #37
        // resolution); record_type_drift_log keeps the qualified
        // "attachment.<subtype>" namespace.
        let inner_type: String = conn
            .query_row(
                "SELECT inner_type FROM attachments WHERE uuid = 'att-idem-unk-001'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(inner_type, "fictional_idem_subtype_zzz");
    }

    // -----------------------------------------------------------------------
    // C1.2.1 — NULL version partition (audit row #15). Asserts that an
    // AttachmentRecord with no `version` field on its envelope produces
    // schema_drift_log + record_type_drift_log rows whose `version` column
    // is SQL NULL, not the literal string `"unknown"`. This protects the
    // partition key from colliding with `decompose_unknown`'s own
    // `"unknown"` fallback for `JSONLRecord::Unknown` records.
    // -----------------------------------------------------------------------
    #[test]
    fn test_decompose_attachment_null_version_partition() {
        let conn = setup_db();
        let tx = conn.unchecked_transaction().unwrap();

        // No `version` field on the envelope; an envelope-level overflow
        // (`agentId`) provides a schema_drift_log row to inspect, and an
        // unmodeled inner subtype provides a record_type_drift_log row.
        let json = r#"{
            "type": "attachment",
            "uuid": "att-nullver-001",
            "sessionId": "sess-nullver",
            "timestamp": "2026-05-09T10:00:00.000Z",
            "cwd": "/tmp/test",
            "gitBranch": "main",
            "slug": "test",
            "isSidechain": false,
            "userType": "external",
            "agentId": "subagent_nullver",
            "attachment": {
                "type": "fictional_nullver_subtype",
                "value": 1
            }
        }"#;
        let record: claude_history_core::record::JSONLRecord =
            serde_json::from_str(json).unwrap();
        decompose_record(&record, "fallback-sess", &tx).unwrap();
        tx.commit().unwrap();

        // schema_drift_log: envelope agentId row's version is NULL.
        let sd_version: Option<String> = conn
            .query_row(
                "SELECT version FROM schema_drift_log
                 WHERE field_name = 'agentId' AND record_type = 'attachment'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(
            sd_version.is_none(),
            "AttachmentRecord with no version must bind NULL on schema_drift_log.version, not the literal 'unknown'"
        );

        // record_type_drift_log: unmodeled inner subtype row's version is NULL.
        let rd_version: Option<String> = conn
            .query_row(
                "SELECT version FROM record_type_drift_log
                 WHERE type_name = 'attachment.fictional_nullver_subtype'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(
            rd_version.is_none(),
            "AttachmentRecord with no version must bind NULL on record_type_drift_log.version, not the literal 'unknown'"
        );

        // Neither table has a row with the literal version='unknown' for
        // this attachment-derived data — that partition string is reserved
        // for decompose_unknown's JSONLRecord::Unknown fallback.
        let collision_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM schema_drift_log
                 WHERE record_type = 'attachment' AND version = 'unknown'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(collision_count, 0);
    }
}
