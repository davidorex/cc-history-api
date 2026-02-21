//! Query builder functions for CLI subcommands and HTTP API handlers.
//!
//! Each function takes a `&rusqlite::Connection` and returns
//! `Result<T, rusqlite::Error>` with Serialize+Debug result structs.
//! Results are collected inside the conn scope and returned to the caller
//! for formatting — no println or I/O inside DB operations.
//!
//! All queries use parameterized SQL (`?N` placeholders) with
//! `rusqlite::params_from_iter` for dynamic WHERE clauses. No user-provided
//! values are interpolated directly into SQL strings.
//!
//! Requirement IDs: CLI-03, CLI-04, CLI-06, CLI-07, CLI-08, CLI-09,
//!                  API-03, API-04, API-05, API-06, API-07, API-09

use rusqlite::Connection;
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Result structs
// ---------------------------------------------------------------------------

/// Summary of a session for the `sessions` CLI subcommand.
#[derive(Debug, Serialize, Deserialize)]
pub struct SessionSummary {
    pub session_id: String,
    pub project_path: Option<String>,
    pub first_seen_at: Option<String>,
    pub version: Option<String>,
    pub message_count: i64,
    pub model: Option<String>,
}

/// A single message row for the `query` CLI subcommand.
#[derive(Debug, Serialize, Deserialize)]
pub struct MessageResult {
    pub uuid: String,
    pub session_id: String,
    pub message_type: String,
    pub timestamp: String,
    pub model: Option<String>,
    pub stop_reason: Option<String>,
}

/// Aggregated token statistics grouped by a key (model or session).
#[derive(Debug, Serialize, Deserialize)]
pub struct TokenStats {
    pub group_key: String,
    pub message_count: i64,
    pub total_input_tokens: i64,
    pub total_output_tokens: i64,
    pub total_cache_read: Option<i64>,
    pub total_cache_creation: Option<i64>,
}

/// Tool invocation statistics.
#[derive(Debug, Serialize, Deserialize)]
pub struct ToolStats {
    pub tool_name: String,
    pub invocations: i64,
    pub errors: i64,
}

/// Model usage breakdown with percentage.
#[derive(Debug, Serialize, Deserialize)]
pub struct ModelStats {
    pub model: String,
    pub message_count: i64,
    pub percentage: f64,
}

/// Version history entry.
#[derive(Debug, Serialize, Deserialize)]
pub struct VersionEntry {
    pub version: String,
    pub first_seen: String,
    pub last_seen: String,
}

/// Schema drift log entry.
#[derive(Debug, Serialize, Deserialize)]
pub struct DriftEntry {
    pub field_name: String,
    pub record_type: String,
    pub version: Option<String>,
    pub sample_value: Option<String>,
    pub first_seen_at: String,
    pub source_context: Option<String>,
}

/// Detailed session information for the API `GET /sessions/:id` endpoint.
#[derive(Debug, Serialize, Deserialize)]
pub struct SessionDetail {
    pub session_id: String,
    pub project_path: Option<String>,
    pub first_seen_at: Option<String>,
    pub last_seen_at: Option<String>,
    pub version: Option<String>,
    pub slug: Option<String>,
    pub git_branch: Option<String>,
    pub message_count: i64,
    pub primary_model: Option<String>,
}

/// A message in a session conversation with content blocks and token usage.
/// Used by the API `GET /sessions/:id/conversation` endpoint.
#[derive(Debug, Serialize, Deserialize)]
pub struct ConversationMessage {
    pub uuid: String,
    pub session_id: String,
    pub message_type: String,
    pub timestamp: String,
    pub model: Option<String>,
    pub parent_uuid: Option<String>,
    pub is_sidechain: bool,
    pub content_blocks: Vec<ExportContentBlock>,
    pub token_usage: Option<ExportTokenUsage>,
}

/// A node in the session message tree showing parent-child relationships.
/// Used by the API `GET /sessions/:id/tree` endpoint.
#[derive(Debug, Serialize, Deserialize)]
pub struct TreeNode {
    pub uuid: String,
    pub parent_uuid: Option<String>,
    pub is_sidechain: bool,
    pub message_type: String,
    pub timestamp: String,
    pub model: Option<String>,
    pub children_count: i64,
}

/// An agent entry from the agents table.
/// Used by the API `GET /sessions/:id/agents` endpoint.
#[derive(Debug, Serialize, Deserialize)]
pub struct AgentEntry {
    pub agent_id: String,
    pub session_id: Option<String>,
    pub first_seen_at: Option<String>,
    pub last_seen_at: Option<String>,
}

/// Aggregated summary statistics for a session.
/// Used by the API `GET /sessions/:id/summary` endpoint.
#[derive(Debug, Serialize, Deserialize)]
pub struct SessionSummaryStats {
    pub session_id: String,
    pub message_count: i64,
    pub total_input_tokens: i64,
    pub total_output_tokens: i64,
    pub tool_use_count: i64,
    pub unique_tools: i64,
    pub first_timestamp: Option<String>,
    pub last_timestamp: Option<String>,
    pub duration_seconds: Option<i64>,
}

/// A message with content blocks and token usage for export.
#[derive(Debug, Serialize, Deserialize)]
pub struct ExportMessage {
    pub uuid: String,
    pub session_id: String,
    pub message_type: String,
    pub timestamp: String,
    pub model: Option<String>,
    pub stop_reason: Option<String>,
    pub content_blocks: Vec<ExportContentBlock>,
    pub token_usage: Option<ExportTokenUsage>,
}

/// A content block within an exported message.
#[derive(Debug, Serialize, Deserialize)]
pub struct ExportContentBlock {
    pub block_index: i64,
    pub block_type: String,
    pub text_content: Option<String>,
    pub tool_use_id: Option<String>,
    pub tool_name: Option<String>,
    pub tool_input: Option<String>,
    pub is_error: Option<bool>,
}

/// Token usage data for an exported message.
#[derive(Debug, Serialize, Deserialize)]
pub struct ExportTokenUsage {
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub cache_creation_input_tokens: Option<i64>,
    pub cache_read_input_tokens: Option<i64>,
}

/// A project row from the projects table.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectEntry {
    pub project_path: String,
    pub display_name: Option<String>,
    pub session_count: i64,
    pub first_seen: String,
    pub last_seen: String,
}

/// Detailed project information from the v_project_summary view.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectDetail {
    pub project_path: String,
    pub display_name: Option<String>,
    pub session_count: i64,
    pub message_count: i64,
    pub total_tokens: i64,
    pub file_operations: i64,
    pub git_operations: i64,
    pub first_activity: Option<String>,
    pub last_activity: Option<String>,
}

// ---------------------------------------------------------------------------
// Query functions
// ---------------------------------------------------------------------------

/// List sessions with optional filters for project path, date range, and limit.
///
/// [CLI-04] Returns sessions ordered by first_seen_at descending, with
/// message count and primary model (first non-null model in the session).
/// Project filter uses substring match (LIKE '%project%').
/// Date filters compare against `sessions.first_seen_at`.
pub fn list_sessions(
    conn: &Connection,
    project: Option<&str>,
    after: Option<&str>,
    before: Option<&str>,
    limit: usize,
) -> Result<Vec<SessionSummary>, rusqlite::Error> {
    let mut conditions = Vec::new();
    let mut param_values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

    if let Some(project) = project {
        conditions.push(format!(
            "s.project_path LIKE ?{}",
            param_values.len() + 1
        ));
        param_values.push(Box::new(format!("%{}%", project)));
    }
    if let Some(after) = after {
        conditions.push(format!("s.first_seen_at >= ?{}", param_values.len() + 1));
        param_values.push(Box::new(after.to_string()));
    }
    if let Some(before) = before {
        conditions.push(format!("s.first_seen_at <= ?{}", param_values.len() + 1));
        param_values.push(Box::new(before.to_string()));
    }

    let where_clause = if conditions.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", conditions.join(" AND "))
    };

    let sql = format!(
        "SELECT
            s.session_id,
            s.project_path,
            s.first_seen_at,
            s.version,
            COUNT(m.uuid) AS message_count,
            (SELECT model FROM messages
             WHERE session_id = s.session_id AND model IS NOT NULL
             LIMIT 1) AS primary_model
         FROM sessions s
         LEFT JOIN messages m ON m.session_id = s.session_id
         {}
         GROUP BY s.session_id
         ORDER BY s.first_seen_at DESC
         LIMIT ?{}",
        where_clause,
        param_values.len() + 1
    );
    param_values.push(Box::new(limit as i64));

    let params: Vec<&dyn rusqlite::types::ToSql> = param_values
        .iter()
        .map(|b| b.as_ref())
        .collect();

    let mut stmt = conn.prepare(&sql)?;
    let results = stmt.query_map(rusqlite::params_from_iter(params), |row| {
        Ok(SessionSummary {
            session_id: row.get(0)?,
            project_path: row.get(1)?,
            first_seen_at: row.get(2)?,
            version: row.get(3)?,
            message_count: row.get(4)?,
            model: row.get(5)?,
        })
    })?;

    results.collect()
}

/// Query messages with optional filters, returning JSON-serializable structs.
///
/// [CLI-03] Supports filtering by session_id, message_type, model, tool name,
/// and date range. The tool filter uses an EXISTS subquery against tool_executions.
/// Results are ordered by timestamp descending.
pub fn query_messages(
    conn: &Connection,
    session_id: Option<&str>,
    message_type: Option<&str>,
    model: Option<&str>,
    tool: Option<&str>,
    after: Option<&str>,
    before: Option<&str>,
    limit: usize,
) -> Result<Vec<MessageResult>, rusqlite::Error> {
    let mut conditions = Vec::new();
    let mut param_values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

    if let Some(sid) = session_id {
        conditions.push(format!("m.session_id = ?{}", param_values.len() + 1));
        param_values.push(Box::new(sid.to_string()));
    }
    if let Some(mt) = message_type {
        conditions.push(format!("m.type = ?{}", param_values.len() + 1));
        param_values.push(Box::new(mt.to_string()));
    }
    if let Some(mdl) = model {
        conditions.push(format!("m.model = ?{}", param_values.len() + 1));
        param_values.push(Box::new(mdl.to_string()));
    }
    if let Some(tool_name) = tool {
        conditions.push(format!(
            "EXISTS (SELECT 1 FROM tool_executions te WHERE te.message_uuid = m.uuid AND te.tool_name = ?{})",
            param_values.len() + 1
        ));
        param_values.push(Box::new(tool_name.to_string()));
    }
    if let Some(after) = after {
        conditions.push(format!("m.timestamp >= ?{}", param_values.len() + 1));
        param_values.push(Box::new(after.to_string()));
    }
    if let Some(before) = before {
        conditions.push(format!("m.timestamp <= ?{}", param_values.len() + 1));
        param_values.push(Box::new(before.to_string()));
    }

    let where_clause = if conditions.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", conditions.join(" AND "))
    };

    let sql = format!(
        "SELECT
            m.uuid,
            m.session_id,
            m.type,
            m.timestamp,
            m.model,
            m.stop_reason
         FROM messages m
         {}
         ORDER BY m.timestamp DESC
         LIMIT ?{}",
        where_clause,
        param_values.len() + 1
    );
    param_values.push(Box::new(limit as i64));

    let params: Vec<&dyn rusqlite::types::ToSql> = param_values
        .iter()
        .map(|b| b.as_ref())
        .collect();

    let mut stmt = conn.prepare(&sql)?;
    let results = stmt.query_map(rusqlite::params_from_iter(params), |row| {
        Ok(MessageResult {
            uuid: row.get(0)?,
            session_id: row.get(1)?,
            message_type: row.get(2)?,
            timestamp: row.get(3)?,
            model: row.get(4)?,
            stop_reason: row.get(5)?,
        })
    })?;

    results.collect()
}

/// Token statistics aggregated by model.
///
/// [CLI-06] Uses SQL aggregation (SUM, COUNT) — not in-memory aggregation.
/// Returns one row per model with total token counts.
pub fn token_stats_by_model(conn: &Connection) -> Result<Vec<TokenStats>, rusqlite::Error> {
    let mut stmt = conn.prepare(
        "SELECT
            COALESCE(m.model, 'unknown') AS model,
            COUNT(*) AS message_count,
            COALESCE(SUM(tu.input_tokens), 0) AS total_input,
            COALESCE(SUM(tu.output_tokens), 0) AS total_output,
            SUM(tu.cache_read_input_tokens) AS total_cache_read,
            SUM(tu.cache_creation_input_tokens) AS total_cache_creation
         FROM token_usage tu
         JOIN messages m ON m.uuid = tu.message_uuid
         GROUP BY m.model
         ORDER BY total_input DESC",
    )?;

    let results = stmt.query_map([], |row| {
        Ok(TokenStats {
            group_key: row.get(0)?,
            message_count: row.get(1)?,
            total_input_tokens: row.get(2)?,
            total_output_tokens: row.get(3)?,
            total_cache_read: row.get(4)?,
            total_cache_creation: row.get(5)?,
        })
    })?;

    results.collect()
}

/// Token statistics aggregated by session, optionally filtered to a single session.
///
/// [CLI-06] Uses SQL aggregation grouped by session_id.
pub fn token_stats_by_session(
    conn: &Connection,
    session_id: Option<&str>,
) -> Result<Vec<TokenStats>, rusqlite::Error> {
    let (sql, params_vec): (String, Vec<Box<dyn rusqlite::types::ToSql>>) = if let Some(sid) =
        session_id
    {
        (
            "SELECT
                m.session_id,
                COUNT(*) AS message_count,
                COALESCE(SUM(tu.input_tokens), 0) AS total_input,
                COALESCE(SUM(tu.output_tokens), 0) AS total_output,
                SUM(tu.cache_read_input_tokens) AS total_cache_read,
                SUM(tu.cache_creation_input_tokens) AS total_cache_creation
             FROM token_usage tu
             JOIN messages m ON m.uuid = tu.message_uuid
             WHERE m.session_id = ?1
             GROUP BY m.session_id
             ORDER BY total_input DESC"
                .to_string(),
            vec![Box::new(sid.to_string()) as Box<dyn rusqlite::types::ToSql>],
        )
    } else {
        (
            "SELECT
                m.session_id,
                COUNT(*) AS message_count,
                COALESCE(SUM(tu.input_tokens), 0) AS total_input,
                COALESCE(SUM(tu.output_tokens), 0) AS total_output,
                SUM(tu.cache_read_input_tokens) AS total_cache_read,
                SUM(tu.cache_creation_input_tokens) AS total_cache_creation
             FROM token_usage tu
             JOIN messages m ON m.uuid = tu.message_uuid
             GROUP BY m.session_id
             ORDER BY total_input DESC"
                .to_string(),
            vec![],
        )
    };

    let params: Vec<&dyn rusqlite::types::ToSql> =
        params_vec.iter().map(|b| b.as_ref()).collect();

    let mut stmt = conn.prepare(&sql)?;
    let results = stmt.query_map(rusqlite::params_from_iter(params), |row| {
        Ok(TokenStats {
            group_key: row.get(0)?,
            message_count: row.get(1)?,
            total_input_tokens: row.get(2)?,
            total_output_tokens: row.get(3)?,
            total_cache_read: row.get(4)?,
            total_cache_creation: row.get(5)?,
        })
    })?;

    results.collect()
}

/// Tool invocation frequency with error counts.
///
/// [CLI-06] Groups by tool_name, counts total invocations and errors.
/// Ordered by invocations descending.
pub fn tool_frequency(conn: &Connection) -> Result<Vec<ToolStats>, rusqlite::Error> {
    let mut stmt = conn.prepare(
        "SELECT
            te.tool_name,
            COUNT(*) AS invocations,
            SUM(CASE WHEN te.is_error = 1 THEN 1 ELSE 0 END) AS errors
         FROM tool_executions te
         GROUP BY te.tool_name
         ORDER BY invocations DESC",
    )?;

    let results = stmt.query_map([], |row| {
        Ok(ToolStats {
            tool_name: row.get(0)?,
            invocations: row.get(1)?,
            errors: row.get(2)?,
        })
    })?;

    results.collect()
}

/// Model usage breakdown with percentage of total messages.
///
/// [CLI-06] Counts messages per model and computes percentage.
/// Only counts messages that have a non-null model (assistant messages).
pub fn model_breakdown(conn: &Connection) -> Result<Vec<ModelStats>, rusqlite::Error> {
    let mut stmt = conn.prepare(
        "SELECT
            m.model,
            COUNT(*) AS message_count,
            ROUND(COUNT(*) * 100.0 / (SELECT COUNT(*) FROM messages WHERE model IS NOT NULL), 2) AS percentage
         FROM messages m
         WHERE m.model IS NOT NULL
         GROUP BY m.model
         ORDER BY message_count DESC",
    )?;

    let results = stmt.query_map([], |row| {
        Ok(ModelStats {
            model: row.get(0)?,
            message_count: row.get(1)?,
            percentage: row.get(2)?,
        })
    })?;

    results.collect()
}

/// Version history showing distinct Claude Code versions observed over time.
///
/// [CLI-08] Groups by version, showing first and last seen timestamps.
/// Ordered by first appearance.
pub fn version_history(conn: &Connection) -> Result<Vec<VersionEntry>, rusqlite::Error> {
    let mut stmt = conn.prepare(
        "SELECT DISTINCT
            version,
            MIN(timestamp) AS first_seen,
            MAX(timestamp) AS last_seen
         FROM messages
         WHERE version IS NOT NULL
         GROUP BY version
         ORDER BY MIN(timestamp)",
    )?;

    let results = stmt.query_map([], |row| {
        Ok(VersionEntry {
            version: row.get(0)?,
            first_seen: row.get(1)?,
            last_seen: row.get(2)?,
        })
    })?;

    results.collect()
}

/// List all schema drift log entries.
///
/// [CLI-09] Returns all entries from schema_drift_log ordered by first_seen_at
/// descending (most recent first).
pub fn schema_drift_list(conn: &Connection) -> Result<Vec<DriftEntry>, rusqlite::Error> {
    let mut stmt = conn.prepare(
        "SELECT
            field_name,
            record_type,
            version,
            sample_value,
            first_seen_at,
            source_context
         FROM schema_drift_log
         ORDER BY first_seen_at DESC",
    )?;

    let results = stmt.query_map([], |row| {
        Ok(DriftEntry {
            field_name: row.get(0)?,
            record_type: row.get(1)?,
            version: row.get(2)?,
            sample_value: row.get(3)?,
            first_seen_at: row.get(4)?,
            source_context: row.get(5)?,
        })
    })?;

    results.collect()
}

/// Retrieve messages for export in batches, with content blocks and token usage.
///
/// [CLI-07] Loads messages in batches to avoid OOM on large sessions.
/// For each message, loads its content blocks and optionally token usage.
/// Returns ExportMessage structs suitable for JSON serialization.
pub fn session_messages_for_export(
    conn: &Connection,
    session_id: &str,
    batch_size: usize,
    offset: usize,
) -> Result<Vec<ExportMessage>, rusqlite::Error> {
    // 1. Load a batch of messages for this session
    let mut msg_stmt = conn.prepare(
        "SELECT uuid, session_id, type, timestamp, model, stop_reason
         FROM messages
         WHERE session_id = ?1
         ORDER BY timestamp ASC
         LIMIT ?2 OFFSET ?3",
    )?;

    let messages: Vec<(String, String, String, String, Option<String>, Option<String>)> = msg_stmt
        .query_map(
            rusqlite::params![session_id, batch_size as i64, offset as i64],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                ))
            },
        )?
        .collect::<Result<Vec<_>, _>>()?;

    // 2. For each message, load content blocks and token usage
    let mut content_stmt = conn.prepare(
        "SELECT block_index, block_type, text_content, tool_use_id, tool_name, tool_input, is_error
         FROM message_content
         WHERE message_uuid = ?1
         ORDER BY block_index ASC",
    )?;

    let mut usage_stmt = conn.prepare(
        "SELECT input_tokens, output_tokens, cache_creation_input_tokens, cache_read_input_tokens
         FROM token_usage
         WHERE message_uuid = ?1",
    )?;

    let mut result = Vec::with_capacity(messages.len());

    for (uuid, sid, mtype, ts, model, stop_reason) in messages {
        // Load content blocks
        let blocks: Vec<ExportContentBlock> = content_stmt
            .query_map(rusqlite::params![&uuid], |row| {
                Ok(ExportContentBlock {
                    block_index: row.get(0)?,
                    block_type: row.get(1)?,
                    text_content: row.get(2)?,
                    tool_use_id: row.get(3)?,
                    tool_name: row.get(4)?,
                    tool_input: row.get(5)?,
                    is_error: row.get::<_, Option<i32>>(6)?.map(|v| v != 0),
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        // Load token usage (may not exist for non-assistant messages)
        let usage: Option<ExportTokenUsage> = usage_stmt
            .query_row(rusqlite::params![&uuid], |row| {
                Ok(ExportTokenUsage {
                    input_tokens: row.get(0)?,
                    output_tokens: row.get(1)?,
                    cache_creation_input_tokens: row.get(2)?,
                    cache_read_input_tokens: row.get(3)?,
                })
            })
            .ok();

        result.push(ExportMessage {
            uuid,
            session_id: sid,
            message_type: mtype,
            timestamp: ts,
            model,
            stop_reason,
            content_blocks: blocks,
            token_usage: usage,
        });
    }

    Ok(result)
}

// ---------------------------------------------------------------------------
// API query functions (Plan 03-01)
// ---------------------------------------------------------------------------

/// Get detailed information about a single session.
///
/// [API-03] Returns session metadata with message count and primary model.
/// Returns None if the session_id does not exist.
pub fn get_session(
    conn: &Connection,
    session_id: &str,
) -> Result<Option<SessionDetail>, rusqlite::Error> {
    let mut stmt = conn.prepare(
        "SELECT
            s.session_id,
            s.project_path,
            s.first_seen_at,
            s.last_seen_at,
            s.version,
            s.slug,
            s.git_branch,
            COUNT(m.uuid) AS message_count,
            (SELECT model FROM messages
             WHERE session_id = s.session_id AND model IS NOT NULL
             LIMIT 1) AS primary_model
         FROM sessions s
         LEFT JOIN messages m ON m.session_id = s.session_id
         WHERE s.session_id = ?1
         GROUP BY s.session_id",
    )?;

    let result = stmt
        .query_row(rusqlite::params![session_id], |row| {
            Ok(SessionDetail {
                session_id: row.get(0)?,
                project_path: row.get(1)?,
                first_seen_at: row.get(2)?,
                last_seen_at: row.get(3)?,
                version: row.get(4)?,
                slug: row.get(5)?,
                git_branch: row.get(6)?,
                message_count: row.get(7)?,
                primary_model: row.get(8)?,
            })
        })
        .optional()?;

    Ok(result)
}

/// Get messages in a session as a conversation, with content blocks and token usage.
///
/// [API-04] Returns messages ordered by timestamp ascending with LIMIT/OFFSET
/// pagination. Content blocks can be filtered: if `include_thinking` is false,
/// blocks with block_type "thinking" are excluded; if `include_tool_io` is false,
/// blocks with block_type "tool_use" or "tool_result" are excluded.
pub fn session_conversation(
    conn: &Connection,
    session_id: &str,
    include_thinking: bool,
    include_tool_io: bool,
    limit: usize,
    offset: usize,
) -> Result<Vec<ConversationMessage>, rusqlite::Error> {
    // 1. Load messages for this session
    let mut msg_stmt = conn.prepare(
        "SELECT uuid, session_id, type, timestamp, model, parent_uuid, is_sidechain
         FROM messages
         WHERE session_id = ?1
         ORDER BY timestamp ASC
         LIMIT ?2 OFFSET ?3",
    )?;

    let messages: Vec<(String, String, String, String, Option<String>, Option<String>, i32)> =
        msg_stmt
            .query_map(
                rusqlite::params![session_id, limit as i64, offset as i64],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                        row.get::<_, i32>(6)?,
                    ))
                },
            )?
            .collect::<Result<Vec<_>, _>>()?;

    // 2. For each message, load content blocks and token usage
    let mut content_stmt = conn.prepare(
        "SELECT block_index, block_type, text_content, tool_use_id, tool_name, tool_input, is_error
         FROM message_content
         WHERE message_uuid = ?1
         ORDER BY block_index ASC",
    )?;

    let mut usage_stmt = conn.prepare(
        "SELECT input_tokens, output_tokens, cache_creation_input_tokens, cache_read_input_tokens
         FROM token_usage
         WHERE message_uuid = ?1",
    )?;

    let mut result = Vec::with_capacity(messages.len());

    for (uuid, sid, mtype, ts, model, parent_uuid, is_sidechain_int) in messages {
        // Load content blocks with optional filtering
        let all_blocks: Vec<ExportContentBlock> = content_stmt
            .query_map(rusqlite::params![&uuid], |row| {
                Ok(ExportContentBlock {
                    block_index: row.get(0)?,
                    block_type: row.get(1)?,
                    text_content: row.get(2)?,
                    tool_use_id: row.get(3)?,
                    tool_name: row.get(4)?,
                    tool_input: row.get(5)?,
                    is_error: row.get::<_, Option<i32>>(6)?.map(|v| v != 0),
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        let blocks: Vec<ExportContentBlock> = all_blocks
            .into_iter()
            .filter(|b| {
                if !include_thinking && b.block_type == "thinking" {
                    return false;
                }
                if !include_tool_io
                    && (b.block_type == "tool_use" || b.block_type == "tool_result")
                {
                    return false;
                }
                true
            })
            .collect();

        // Load token usage (may not exist for non-assistant messages)
        let usage: Option<ExportTokenUsage> = usage_stmt
            .query_row(rusqlite::params![&uuid], |row| {
                Ok(ExportTokenUsage {
                    input_tokens: row.get(0)?,
                    output_tokens: row.get(1)?,
                    cache_creation_input_tokens: row.get(2)?,
                    cache_read_input_tokens: row.get(3)?,
                })
            })
            .ok();

        result.push(ConversationMessage {
            uuid,
            session_id: sid,
            message_type: mtype,
            timestamp: ts,
            model,
            parent_uuid,
            is_sidechain: is_sidechain_int != 0,
            content_blocks: blocks,
            token_usage: usage,
        });
    }

    Ok(result)
}

/// Get the message tree structure for a session.
///
/// [API-06] Returns all messages in the session with parent_uuid, is_sidechain,
/// and a count of direct children. This supports rendering conversation trees
/// without loading full content blocks.
pub fn session_tree(
    conn: &Connection,
    session_id: &str,
) -> Result<Vec<TreeNode>, rusqlite::Error> {
    let mut stmt = conn.prepare(
        "SELECT
            m.uuid,
            m.parent_uuid,
            m.is_sidechain,
            m.type,
            m.timestamp,
            m.model,
            (SELECT COUNT(*) FROM messages c WHERE c.parent_uuid = m.uuid) AS children_count
         FROM messages m
         WHERE m.session_id = ?1
         ORDER BY m.timestamp ASC",
    )?;

    let results = stmt.query_map(rusqlite::params![session_id], |row| {
        Ok(TreeNode {
            uuid: row.get(0)?,
            parent_uuid: row.get(1)?,
            is_sidechain: row.get::<_, i32>(2)? != 0,
            message_type: row.get(3)?,
            timestamp: row.get(4)?,
            model: row.get(5)?,
            children_count: row.get(6)?,
        })
    })?;

    results.collect()
}

/// Get agents associated with a session.
///
/// [API-07] Returns all agent entries for the given session_id, ordered by
/// first_seen_at ascending.
pub fn session_agents(
    conn: &Connection,
    session_id: &str,
) -> Result<Vec<AgentEntry>, rusqlite::Error> {
    let mut stmt = conn.prepare(
        "SELECT agent_id, session_id, first_seen_at, last_seen_at
         FROM agents
         WHERE session_id = ?1
         ORDER BY first_seen_at ASC",
    )?;

    let results = stmt.query_map(rusqlite::params![session_id], |row| {
        Ok(AgentEntry {
            agent_id: row.get(0)?,
            session_id: row.get(1)?,
            first_seen_at: row.get(2)?,
            last_seen_at: row.get(3)?,
        })
    })?;

    results.collect()
}

/// Get aggregated summary statistics for a session.
///
/// [API-05] Returns message count, total tokens, tool use statistics,
/// timestamp range, and computed duration. Returns None if the session
/// has no messages.
pub fn session_summary(
    conn: &Connection,
    session_id: &str,
) -> Result<Option<SessionSummaryStats>, rusqlite::Error> {
    let mut stmt = conn.prepare(
        "SELECT
            m.session_id,
            COUNT(m.uuid) AS message_count,
            COALESCE(SUM(tu.input_tokens), 0) AS total_input_tokens,
            COALESCE(SUM(tu.output_tokens), 0) AS total_output_tokens,
            (SELECT COUNT(*) FROM tool_executions te
             JOIN messages m2 ON m2.uuid = te.message_uuid
             WHERE m2.session_id = ?1) AS tool_use_count,
            (SELECT COUNT(DISTINCT te2.tool_name) FROM tool_executions te2
             JOIN messages m3 ON m3.uuid = te2.message_uuid
             WHERE m3.session_id = ?1) AS unique_tools,
            MIN(m.timestamp) AS first_timestamp,
            MAX(m.timestamp) AS last_timestamp,
            CASE
                WHEN MIN(m.timestamp) IS NOT NULL AND MAX(m.timestamp) IS NOT NULL
                THEN CAST(
                    (julianday(MAX(m.timestamp)) - julianday(MIN(m.timestamp))) * 86400
                    AS INTEGER
                )
                ELSE NULL
            END AS duration_seconds
         FROM messages m
         LEFT JOIN token_usage tu ON tu.message_uuid = m.uuid
         WHERE m.session_id = ?1
         GROUP BY m.session_id",
    )?;

    let result = stmt
        .query_row(rusqlite::params![session_id], |row| {
            Ok(SessionSummaryStats {
                session_id: row.get(0)?,
                message_count: row.get(1)?,
                total_input_tokens: row.get(2)?,
                total_output_tokens: row.get(3)?,
                tool_use_count: row.get(4)?,
                unique_tools: row.get(5)?,
                first_timestamp: row.get(6)?,
                last_timestamp: row.get(7)?,
                duration_seconds: row.get(8)?,
            })
        })
        .optional()?;

    Ok(result)
}

/// Get a single message by UUID with its content blocks and token usage.
///
/// [API-09] Reuses the ExportMessage struct. Returns None if the UUID
/// does not exist. Loads content blocks and token usage using the same
/// pattern as session_messages_for_export but for a single message.
pub fn get_message(
    conn: &Connection,
    uuid: &str,
) -> Result<Option<ExportMessage>, rusqlite::Error> {
    // Load the message row
    let msg = conn
        .query_row(
            "SELECT uuid, session_id, type, timestamp, model, stop_reason
             FROM messages
             WHERE uuid = ?1",
            rusqlite::params![uuid],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, Option<String>>(5)?,
                ))
            },
        )
        .optional()?;

    let (msg_uuid, session_id, message_type, timestamp, model, stop_reason) = match msg {
        Some(m) => m,
        None => return Ok(None),
    };

    // Load content blocks
    let mut content_stmt = conn.prepare(
        "SELECT block_index, block_type, text_content, tool_use_id, tool_name, tool_input, is_error
         FROM message_content
         WHERE message_uuid = ?1
         ORDER BY block_index ASC",
    )?;

    let blocks: Vec<ExportContentBlock> = content_stmt
        .query_map(rusqlite::params![&msg_uuid], |row| {
            Ok(ExportContentBlock {
                block_index: row.get(0)?,
                block_type: row.get(1)?,
                text_content: row.get(2)?,
                tool_use_id: row.get(3)?,
                tool_name: row.get(4)?,
                tool_input: row.get(5)?,
                is_error: row.get::<_, Option<i32>>(6)?.map(|v| v != 0),
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    // Load token usage
    let usage: Option<ExportTokenUsage> = conn
        .query_row(
            "SELECT input_tokens, output_tokens, cache_creation_input_tokens, cache_read_input_tokens
             FROM token_usage
             WHERE message_uuid = ?1",
            rusqlite::params![&msg_uuid],
            |row| {
                Ok(ExportTokenUsage {
                    input_tokens: row.get(0)?,
                    output_tokens: row.get(1)?,
                    cache_creation_input_tokens: row.get(2)?,
                    cache_read_input_tokens: row.get(3)?,
                })
            },
        )
        .ok();

    Ok(Some(ExportMessage {
        uuid: msg_uuid,
        session_id,
        message_type,
        timestamp,
        model,
        stop_reason,
        content_blocks: blocks,
        token_usage: usage,
    }))
}

/// Token statistics aggregated by day (DATE of message timestamp).
///
/// [API-09] Reuses the TokenStats struct with group_key set to the date
/// string (YYYY-MM-DD). Returns one row per day, ordered by date descending.
pub fn token_stats_by_day(conn: &Connection) -> Result<Vec<TokenStats>, rusqlite::Error> {
    let mut stmt = conn.prepare(
        "SELECT
            DATE(m.timestamp) AS day,
            COUNT(*) AS message_count,
            COALESCE(SUM(tu.input_tokens), 0) AS total_input,
            COALESCE(SUM(tu.output_tokens), 0) AS total_output,
            SUM(tu.cache_read_input_tokens) AS total_cache_read,
            SUM(tu.cache_creation_input_tokens) AS total_cache_creation
         FROM token_usage tu
         JOIN messages m ON m.uuid = tu.message_uuid
         GROUP BY DATE(m.timestamp)
         ORDER BY day DESC",
    )?;

    let results = stmt.query_map([], |row| {
        Ok(TokenStats {
            group_key: row.get(0)?,
            message_count: row.get(1)?,
            total_input_tokens: row.get(2)?,
            total_output_tokens: row.get(3)?,
            total_cache_read: row.get(4)?,
            total_cache_creation: row.get(5)?,
        })
    })?;

    results.collect()
}

/// List all projects, ordered by session_count descending.
///
/// [M2-P4] Reads from the projects table populated by migration 004.
pub fn list_projects(conn: &Connection, limit: usize) -> Result<Vec<ProjectEntry>, rusqlite::Error> {
    let mut stmt = conn.prepare(
        "SELECT project_path, display_name, session_count, first_seen, last_seen
         FROM projects
         ORDER BY session_count DESC
         LIMIT ?1",
    )?;

    let results = stmt.query_map(rusqlite::params![limit as i64], |row| {
        Ok(ProjectEntry {
            project_path: row.get(0)?,
            display_name: row.get(1)?,
            session_count: row.get(2)?,
            first_seen: row.get(3)?,
            last_seen: row.get(4)?,
        })
    })?;

    results.collect()
}

/// Get detailed project information from the v_project_summary view.
///
/// [M2-P4] Joins with the projects table for display_name. Returns None
/// if the project_path does not exist in v_project_summary.
pub fn get_project(
    conn: &Connection,
    project_path: &str,
) -> Result<Option<ProjectDetail>, rusqlite::Error> {
    let mut stmt = conn.prepare(
        "SELECT
            vs.project_path,
            p.display_name,
            vs.session_count,
            vs.message_count,
            vs.total_tokens,
            vs.file_operations,
            vs.git_operations,
            vs.first_activity,
            vs.last_activity
         FROM v_project_summary vs
         LEFT JOIN projects p ON p.project_path = vs.project_path
         WHERE vs.project_path = ?1",
    )?;

    let result = stmt
        .query_row(rusqlite::params![project_path], |row| {
            Ok(ProjectDetail {
                project_path: row.get(0)?,
                display_name: row.get(1)?,
                session_count: row.get(2)?,
                message_count: row.get(3)?,
                total_tokens: row.get(4)?,
                file_operations: row.get(5)?,
                git_operations: row.get(6)?,
                first_activity: row.get(7)?,
                last_activity: row.get(8)?,
            })
        })
        .optional()?;

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema;

    fn setup_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        schema::run_migrations(&conn).unwrap();
        conn
    }

    /// Insert a test session and corresponding projects row.
    fn seed_project(conn: &Connection, session_id: &str, project_path: &str) {
        conn.execute(
            "INSERT OR IGNORE INTO sessions (session_id, project_path, first_seen_at)
             VALUES (?1, ?2, '2026-01-01T00:00:00Z')",
            rusqlite::params![session_id, project_path],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO projects (project_path, display_name, first_seen, last_seen, session_count)
             VALUES (?1, ?2, '2026-01-01T00:00:00Z', '2026-01-02T00:00:00Z',
                     (SELECT COUNT(*) FROM sessions WHERE project_path = ?1))
             ON CONFLICT(project_path) DO UPDATE SET
               session_count = (SELECT COUNT(*) FROM sessions WHERE project_path = ?1)",
            rusqlite::params![project_path, project_path.split('/').last().unwrap_or("")],
        )
        .unwrap();
    }

    #[test]
    fn test_list_projects_returns_entries() {
        let conn = setup_db();
        seed_project(&conn, "sess-a", "/Users/dev/Projects/alpha");
        seed_project(&conn, "sess-b", "/Users/dev/Projects/beta");

        let projects = list_projects(&conn, 10).unwrap();
        assert_eq!(projects.len(), 2);
        let paths: Vec<&str> = projects.iter().map(|p| p.project_path.as_str()).collect();
        assert!(paths.contains(&"/Users/dev/Projects/alpha"));
        assert!(paths.contains(&"/Users/dev/Projects/beta"));
    }

    #[test]
    fn test_list_projects_respects_limit() {
        let conn = setup_db();
        seed_project(&conn, "sess-1", "/Users/dev/Projects/one");
        seed_project(&conn, "sess-2", "/Users/dev/Projects/two");
        seed_project(&conn, "sess-3", "/Users/dev/Projects/three");

        let projects = list_projects(&conn, 2).unwrap();
        assert_eq!(projects.len(), 2);
    }

    #[test]
    fn test_get_project_returns_detail() {
        let conn = setup_db();
        let path = "/Users/dev/Projects/myapp";
        seed_project(&conn, "sess-detail", path);

        // Insert a message so v_project_summary has data
        conn.execute(
            "INSERT INTO messages (uuid, session_id, type, timestamp)
             VALUES ('msg-1', 'sess-detail', 'user', '2026-01-01T01:00:00Z')",
            [],
        )
        .unwrap();

        let detail = get_project(&conn, path).unwrap();
        assert!(detail.is_some(), "get_project should return Some for an existing project");
        let d = detail.unwrap();
        assert_eq!(d.project_path, path);
        assert_eq!(d.session_count, 1);
        assert_eq!(d.message_count, 1);
    }

    #[test]
    fn test_get_project_returns_none_for_missing() {
        let conn = setup_db();
        let detail = get_project(&conn, "/nonexistent/path").unwrap();
        assert!(detail.is_none());
    }
}
