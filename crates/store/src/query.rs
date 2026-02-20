//! Query builder functions for all CLI subcommands.
//!
//! Each function takes a `&rusqlite::Connection` and returns
//! `Result<Vec<T>, rusqlite::Error>` with Serialize+Debug result structs.
//! Results are collected inside the conn scope and returned to the caller
//! for formatting — no println or I/O inside DB operations.
//!
//! All queries use parameterized SQL (`?N` placeholders) with
//! `rusqlite::params_from_iter` for dynamic WHERE clauses. No user-provided
//! values are interpolated directly into SQL strings.
//!
//! Requirement IDs: CLI-03, CLI-04, CLI-06, CLI-07, CLI-08, CLI-09

use rusqlite::Connection;
use serde::Serialize;

// ---------------------------------------------------------------------------
// Result structs
// ---------------------------------------------------------------------------

/// Summary of a session for the `sessions` CLI subcommand.
#[derive(Debug, Serialize)]
pub struct SessionSummary {
    pub session_id: String,
    pub project_path: Option<String>,
    pub first_seen_at: Option<String>,
    pub version: Option<String>,
    pub message_count: i64,
    pub model: Option<String>,
}

/// A single message row for the `query` CLI subcommand.
#[derive(Debug, Serialize)]
pub struct MessageResult {
    pub uuid: String,
    pub session_id: String,
    pub message_type: String,
    pub timestamp: String,
    pub model: Option<String>,
    pub stop_reason: Option<String>,
}

/// Aggregated token statistics grouped by a key (model or session).
#[derive(Debug, Serialize)]
pub struct TokenStats {
    pub group_key: String,
    pub message_count: i64,
    pub total_input_tokens: i64,
    pub total_output_tokens: i64,
    pub total_cache_read: Option<i64>,
    pub total_cache_creation: Option<i64>,
}

/// Tool invocation statistics.
#[derive(Debug, Serialize)]
pub struct ToolStats {
    pub tool_name: String,
    pub invocations: i64,
    pub errors: i64,
}

/// Model usage breakdown with percentage.
#[derive(Debug, Serialize)]
pub struct ModelStats {
    pub model: String,
    pub message_count: i64,
    pub percentage: f64,
}

/// Version history entry.
#[derive(Debug, Serialize)]
pub struct VersionEntry {
    pub version: String,
    pub first_seen: String,
    pub last_seen: String,
}

/// Schema drift log entry.
#[derive(Debug, Serialize)]
pub struct DriftEntry {
    pub field_name: String,
    pub record_type: String,
    pub version: Option<String>,
    pub sample_value: Option<String>,
    pub first_seen_at: String,
    pub source_context: Option<String>,
}

/// A message with content blocks and token usage for export.
#[derive(Debug, Serialize)]
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
#[derive(Debug, Serialize)]
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
#[derive(Debug, Serialize)]
pub struct ExportTokenUsage {
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub cache_creation_input_tokens: Option<i64>,
    pub cache_read_input_tokens: Option<i64>,
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
