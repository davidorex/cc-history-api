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

use std::collections::{BTreeMap, HashSet};

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

/// Record-type drift log entry — one row per (type_name, version) observed.
///
/// Mirrors the shape of the `record_type_drift_log` table introduced in
/// migration 007. Where `DriftEntry` represents an unknown *field* on a known
/// record type, this represents an unknown top-level *record type*
/// (the JSONLRecord::Unknown variant). The two are disjoint sources of
/// schema-evolution telemetry and live in separate tables.
#[derive(Debug, Serialize, Deserialize)]
pub struct RecordTypeDriftEntry {
    pub type_name: String,
    pub version: Option<String>,
    pub sample_value: Option<String>,
    pub first_seen_at: String,
    pub last_seen_at: String,
    pub occurrence_count: i64,
}

/// One row from the `attachments` table (migration 008) projected for the
/// CLI / REST / MCP list and show surfaces added in C1.4.
///
/// Mirrors the `attachments` envelope shape verbatim; the inner subtype body
/// remains opaque-JSON in `body_json` (raw text — the typed `AttachmentBody`
/// enum lives in the core crate and is not pulled into the store query layer
/// to keep this surface dependency-light).
///
/// The `body_json` field is `Option<String>` because some modeled subtypes
/// (e.g. those whose entire payload is captured in the envelope) may store
/// `NULL` per migration 008's schema. Callers that need typed inspection
/// should round-trip through `core::record::AttachmentRecord` separately.
#[derive(Debug, Serialize, Deserialize)]
pub struct AttachmentRow {
    pub uuid: String,
    pub session_id: String,
    pub parent_uuid: Option<String>,
    pub timestamp: String,
    pub cwd: Option<String>,
    pub version: Option<String>,
    pub git_branch: Option<String>,
    pub slug: Option<String>,
    pub entrypoint: Option<String>,
    pub inner_type: String,
    pub body_json: Option<String>,
}

/// One row from the `hook_executions` table (migration 008) projected for
/// the CLI / REST / MCP list surface added in C1.4.
///
/// All decomposed columns from migration 008 are surfaced. Nullable columns
/// are `Option<...>` to faithfully represent the table's NULL-allowed shape
/// — `tool_use_id` in particular is naturally NULL for
/// hook_permission_decision rows where no tool_use is attached.
#[derive(Debug, Serialize, Deserialize)]
pub struct HookExecutionRow {
    pub id: i64,
    pub attachment_uuid: String,
    pub hook_name: Option<String>,
    pub hook_event: Option<String>,
    pub tool_use_id: Option<String>,
    pub exit_code: Option<i64>,
    pub duration_ms: Option<i64>,
    pub stdout: Option<String>,
    pub stderr: Option<String>,
    pub command: Option<String>,
    pub decision: Option<String>,
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

/// Enhanced version history entry from the version_history table.
/// Includes session_count and new_fields_count for richer timeline display.
#[derive(Debug, Serialize, Deserialize)]
pub struct VersionHistoryEntry {
    pub version: String,
    pub first_seen_at: String,
    pub last_seen_at: String,
    pub session_id: Option<String>,
    pub session_count: i64,
    pub new_fields_count: i64,
}

/// A drift field with occurrence count and promotion status.
/// Promotion status is computed dynamically from schema introspection.
#[derive(Debug, Serialize, Deserialize)]
pub struct DriftFieldEntry {
    pub field_name: String,
    pub record_type: String,
    pub sample_value: Option<String>,
    pub occurrence_count: i64,
    pub first_seen_at: String,
    pub promotion_status: String,
}

/// Drift entries grouped by version, then by record type within each version.
#[derive(Debug, Serialize, Deserialize)]
pub struct VersionDriftGroup {
    pub version: String,
    pub record_types: Vec<RecordTypeDriftGroup>,
}

/// Drift entries for a specific record type within a version.
#[derive(Debug, Serialize, Deserialize)]
pub struct RecordTypeDriftGroup {
    pub record_type: String,
    pub fields: Vec<DriftFieldEntry>,
}

/// Version diff entry showing fields introduced or disappeared in a version.
#[derive(Debug, Serialize, Deserialize)]
pub struct VersionDiffEntry {
    pub version: String,
    pub first_seen_at: String,
    pub last_seen_at: String,
    pub session_id: Option<String>,
    pub session_count: i64,
    pub new_fields_count: i64,
    pub new_fields: Vec<String>,
    pub disappeared_fields: Vec<String>,
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

/// List record-type drift log entries with optional filters.
///
/// Returns rows from `record_type_drift_log` ordered by `last_seen_at`
/// descending (most recent first). Optional filters narrow the result set:
/// - `type_name`: substring match against `type_name` (e.g. `attachment`)
/// - `version`: exact match against `version`
/// - `since`: lower bound (inclusive) on `last_seen_at` as ISO-8601 text;
///   matches the SQLite `datetime()` text format used in the table
/// - `limit`: cap on rows returned (None = no cap)
///
/// All filters bind via parameterized SQL; no user-provided values are
/// interpolated. The substring match for `type_name` uses LIKE with
/// runtime-escaped `%` wildcards.
pub fn record_type_drift_list(
    conn: &Connection,
    type_name: Option<&str>,
    version: Option<&str>,
    since: Option<&str>,
    limit: Option<usize>,
) -> Result<Vec<RecordTypeDriftEntry>, rusqlite::Error> {
    let mut sql = String::from(
        "SELECT type_name, version, sample_value, first_seen_at, last_seen_at, occurrence_count
         FROM record_type_drift_log
         WHERE 1=1",
    );
    let mut params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();

    if let Some(tn) = type_name {
        sql.push_str(" AND type_name LIKE ?");
        // Wrap with %...% for substring match; LIKE wildcards in user input are
        // escaped to literal % and _ via the ESCAPE clause.
        let escaped = tn.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_");
        sql.push_str(" ESCAPE '\\'");
        params.push(Box::new(format!("%{}%", escaped)));
    }

    if let Some(v) = version {
        sql.push_str(" AND version = ?");
        params.push(Box::new(v.to_string()));
    }

    if let Some(s) = since {
        sql.push_str(" AND last_seen_at >= ?");
        params.push(Box::new(s.to_string()));
    }

    sql.push_str(" ORDER BY last_seen_at DESC");

    if let Some(l) = limit {
        sql.push_str(" LIMIT ?");
        params.push(Box::new(l as i64));
    }

    let mut stmt = conn.prepare(&sql)?;
    let param_refs: Vec<&dyn rusqlite::ToSql> =
        params.iter().map(|b| b.as_ref()).collect();
    let results = stmt.query_map(param_refs.as_slice(), |row| {
        Ok(RecordTypeDriftEntry {
            type_name: row.get(0)?,
            version: row.get(1)?,
            sample_value: row.get(2)?,
            first_seen_at: row.get(3)?,
            last_seen_at: row.get(4)?,
            occurrence_count: row.get(5)?,
        })
    })?;

    results.collect()
}

/// List attachment rows with optional filters.
///
/// Returns rows from the `attachments` table (migration 008) ordered by
/// `timestamp` descending (most recent first). Filters narrow the result set:
/// - `project`: substring match against `sessions.project_path` (joined via
///   `attachments.session_id`). Sessions with NULL project_path do not match
///   the substring filter (LIKE on NULL yields NULL → row omitted).
/// - `inner_type`: exact match against `attachments.inner_type` (e.g.
///   `hook_success`, `mcp_instructions_delta`).
/// - `since`: lower bound (inclusive) on `attachments.timestamp` (ISO-8601
///   text matching the format the decomposer writes).
/// - `limit`: cap on rows returned.
///
/// All filters bind via parameterized SQL; user-provided values are not
/// interpolated.
pub fn attachments_list(
    conn: &Connection,
    project: Option<&str>,
    inner_type: Option<&str>,
    since: Option<&str>,
    limit: usize,
) -> Result<Vec<AttachmentRow>, rusqlite::Error> {
    let mut sql = String::from(
        "SELECT a.uuid, a.session_id, a.parent_uuid, a.timestamp, a.cwd,
                a.version, a.git_branch, a.slug, a.entrypoint, a.inner_type,
                a.body_json
         FROM attachments a
         LEFT JOIN sessions s ON s.session_id = a.session_id
         WHERE 1=1",
    );
    let mut params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();

    if let Some(p) = project {
        // Substring match on project_path. LIKE-vs-NULL evaluates to NULL,
        // so sessions with no project_path naturally drop out — that matches
        // the existing `list_sessions` precedent where project filter is a
        // LIKE expression rather than an IS NULL OR LIKE compound.
        sql.push_str(" AND s.project_path LIKE ?");
        let escaped = p.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_");
        sql.push_str(" ESCAPE '\\'");
        params.push(Box::new(format!("%{}%", escaped)));
    }

    if let Some(it) = inner_type {
        sql.push_str(" AND a.inner_type = ?");
        params.push(Box::new(it.to_string()));
    }

    if let Some(s) = since {
        sql.push_str(" AND a.timestamp >= ?");
        params.push(Box::new(s.to_string()));
    }

    sql.push_str(" ORDER BY a.timestamp DESC LIMIT ?");
    params.push(Box::new(limit as i64));

    let mut stmt = conn.prepare(&sql)?;
    let param_refs: Vec<&dyn rusqlite::ToSql> =
        params.iter().map(|b| b.as_ref()).collect();
    let results = stmt.query_map(param_refs.as_slice(), |row| {
        Ok(AttachmentRow {
            uuid: row.get(0)?,
            session_id: row.get(1)?,
            parent_uuid: row.get(2)?,
            timestamp: row.get(3)?,
            cwd: row.get(4)?,
            version: row.get(5)?,
            git_branch: row.get(6)?,
            slug: row.get(7)?,
            entrypoint: row.get(8)?,
            inner_type: row.get(9)?,
            body_json: row.get(10)?,
        })
    })?;

    results.collect()
}

/// Fetch a single attachment row by uuid.
///
/// Returns `Ok(None)` if no row matches — distinguishing "not found" from
/// "query failed" so REST/CLI callers can return 404 vs 500 cleanly.
pub fn attachment_by_uuid(
    conn: &Connection,
    uuid: &str,
) -> Result<Option<AttachmentRow>, rusqlite::Error> {
    let mut stmt = conn.prepare(
        "SELECT uuid, session_id, parent_uuid, timestamp, cwd, version,
                git_branch, slug, entrypoint, inner_type, body_json
         FROM attachments
         WHERE uuid = ?",
    )?;

    stmt.query_row(rusqlite::params![uuid], |row| {
        Ok(AttachmentRow {
            uuid: row.get(0)?,
            session_id: row.get(1)?,
            parent_uuid: row.get(2)?,
            timestamp: row.get(3)?,
            cwd: row.get(4)?,
            version: row.get(5)?,
            git_branch: row.get(6)?,
            slug: row.get(7)?,
            entrypoint: row.get(8)?,
            inner_type: row.get(9)?,
            body_json: row.get(10)?,
        })
    })
    .optional()
}

/// List hook_executions rows with optional filters.
///
/// Returns rows from the `hook_executions` table (migration 008) ordered by
/// `id` descending — newer rows first, matching insertion order under the
/// AUTOINCREMENT column. Filters:
/// - `tool_use_id`: exact match against `hook_executions.tool_use_id`.
///   Useful for joining a single tool_executions row to its hook events.
/// - `hook_event`: exact match against `hook_executions.hook_event`
///   (e.g. `PreToolUse`, `PostToolUse`, `UserPromptSubmit`, `Stop`).
/// - `exit_code`: exact match against `hook_executions.exit_code`.
/// - `limit`: cap on rows returned.
///
/// All filters bind via parameterized SQL.
pub fn hook_executions_list(
    conn: &Connection,
    tool_use_id: Option<&str>,
    hook_event: Option<&str>,
    exit_code: Option<i64>,
    limit: usize,
) -> Result<Vec<HookExecutionRow>, rusqlite::Error> {
    let mut sql = String::from(
        "SELECT id, attachment_uuid, hook_name, hook_event, tool_use_id,
                exit_code, duration_ms, stdout, stderr, command, decision
         FROM hook_executions
         WHERE 1=1",
    );
    let mut params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();

    if let Some(tid) = tool_use_id {
        sql.push_str(" AND tool_use_id = ?");
        params.push(Box::new(tid.to_string()));
    }

    if let Some(he) = hook_event {
        sql.push_str(" AND hook_event = ?");
        params.push(Box::new(he.to_string()));
    }

    if let Some(ec) = exit_code {
        sql.push_str(" AND exit_code = ?");
        params.push(Box::new(ec));
    }

    sql.push_str(" ORDER BY id DESC LIMIT ?");
    params.push(Box::new(limit as i64));

    let mut stmt = conn.prepare(&sql)?;
    let param_refs: Vec<&dyn rusqlite::ToSql> =
        params.iter().map(|b| b.as_ref()).collect();
    let results = stmt.query_map(param_refs.as_slice(), |row| {
        Ok(HookExecutionRow {
            id: row.get(0)?,
            attachment_uuid: row.get(1)?,
            hook_name: row.get(2)?,
            hook_event: row.get(3)?,
            tool_use_id: row.get(4)?,
            exit_code: row.get(5)?,
            duration_ms: row.get(6)?,
            stdout: row.get(7)?,
            stderr: row.get(8)?,
            command: row.get(9)?,
            decision: row.get(10)?,
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

    // 2. For each message, load content blocks and token usage.
    //    The `block_type != 'plan_content'` filter excludes synthetic
    //    block_index = -1 rows introduced by migration 011 / decompose_user
    //    step 4b. Those rows live in message_content for FTS-indexability of
    //    plan markdown but are not user-facing content blocks; including
    //    them in export output produced a leading `*[plan_content]*` prefix
    //    line via `write_markdown_block`'s `_` fallthrough arm. Aim: keep
    //    the synthetic rows FTS-only by filtering them out of every export
    //    query that builds an ExportContentBlock list.
    let mut content_stmt = conn.prepare(
        "SELECT block_index, block_type, text_content, tool_use_id, tool_name, tool_input, is_error
         FROM message_content
         WHERE message_uuid = ?1 AND block_type != 'plan_content'
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

    // 2. For each message, load content blocks and token usage.
    //    Filter `block_type != 'plan_content'` to keep migration-011's
    //    synthetic FTS-only rows (block_index = -1) out of the conversation
    //    block list — same rationale as session_messages_for_export above.
    let mut content_stmt = conn.prepare(
        "SELECT block_index, block_type, text_content, tool_use_id, tool_name, tool_input, is_error
         FROM message_content
         WHERE message_uuid = ?1 AND block_type != 'plan_content'
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

    // Load content blocks. The `block_type != 'plan_content'` predicate
    // matches the export-side filter in session_messages_for_export and
    // session_conversation: synthetic block_index = -1 rows from migration
    // 011 / decompose_user step 4b are FTS-only and not user-facing message
    // blocks; excluding them here keeps single-message retrieval semantics
    // aligned with batched export retrieval.
    let mut content_stmt = conn.prepare(
        "SELECT block_index, block_type, text_content, tool_use_id, tool_name, tool_input, is_error
         FROM message_content
         WHERE message_uuid = ?1 AND block_type != 'plan_content'
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

// ---------------------------------------------------------------------------
// Version monitoring query functions (Plan 06-03)
// ---------------------------------------------------------------------------

/// Enhanced version history from the version_history table.
///
/// [VER-01] Returns all versions ordered by first_seen_at ascending,
/// including session_count and new_fields_count for timeline display.
/// This complements the older `version_history()` function which queries
/// messages directly — this one reads from the dedicated version_history table.
pub fn version_history_enhanced(
    conn: &Connection,
) -> Result<Vec<VersionHistoryEntry>, rusqlite::Error> {
    let mut stmt = conn.prepare(
        "SELECT version, first_seen_at, last_seen_at, session_id, session_count, new_fields_count
         FROM version_history
         ORDER BY first_seen_at ASC",
    )?;

    let results = stmt.query_map([], |row| {
        Ok(VersionHistoryEntry {
            version: row.get(0)?,
            first_seen_at: row.get(1)?,
            last_seen_at: row.get(2)?,
            session_id: row.get(3)?,
            session_count: row.get(4)?,
            new_fields_count: row.get(5)?,
        })
    })?;

    results.collect()
}

/// Version history with diff analysis: for each version, shows which drift
/// fields first appeared ("new_fields") and which disappeared compared to
/// the previous version.
///
/// [VER-01] new_fields are drift fields whose first_seen_at in schema_drift_log
/// is attributed to this version. disappeared_fields are drift fields present
/// in the immediately preceding version but absent in this one.
pub fn version_history_with_diff(
    conn: &Connection,
) -> Result<Vec<VersionDiffEntry>, rusqlite::Error> {
    // Load all versions in chronological order
    let versions = version_history_enhanced(conn)?;

    // Load all drift entries grouped by version
    let mut stmt = conn.prepare(
        "SELECT field_name, record_type, version
         FROM schema_drift_log
         WHERE version IS NOT NULL
         ORDER BY version, field_name",
    )?;

    // Build a map: version -> set of "field_name:record_type" keys
    let mut version_fields: BTreeMap<String, HashSet<String>> = BTreeMap::new();
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
        ))
    })?;

    for row in rows {
        let (field_name, record_type, version) = row?;
        version_fields
            .entry(version)
            .or_default()
            .insert(format!("{}:{}", field_name, record_type));
    }

    let mut result = Vec::with_capacity(versions.len());
    let mut prev_fields: Option<&HashSet<String>> = None;

    for v in &versions {
        let current_fields = version_fields.get(&v.version);
        let empty = HashSet::new();
        let current = current_fields.unwrap_or(&empty);

        // new_fields: in current but not in any earlier version's drift entries
        // (approximated by checking the immediately preceding version)
        let new_fields: Vec<String> = if let Some(prev) = prev_fields {
            current
                .iter()
                .filter(|f| !prev.contains(*f))
                .cloned()
                .collect()
        } else {
            // First version: all fields are "new"
            current.iter().cloned().collect()
        };

        // disappeared_fields: in previous version but not in current
        let disappeared_fields: Vec<String> = if let Some(prev) = prev_fields {
            prev.iter()
                .filter(|f| !current.contains(*f))
                .cloned()
                .collect()
        } else {
            Vec::new()
        };

        result.push(VersionDiffEntry {
            version: v.version.clone(),
            first_seen_at: v.first_seen_at.clone(),
            last_seen_at: v.last_seen_at.clone(),
            session_id: v.session_id.clone(),
            session_count: v.session_count,
            new_fields_count: v.new_fields_count,
            new_fields,
            disappeared_fields,
        });

        prev_fields = current_fields;
    }

    Ok(result)
}

/// Map a drift record_type to its target database table name.
///
/// Returns None for record types that have no corresponding table
/// (e.g., "file-history-snapshot").
fn record_type_to_table(record_type: &str) -> Option<&'static str> {
    match record_type {
        "user" | "assistant" | "assistant.message" | "progress" => Some("messages"),
        "assistant.message.usage" => Some("token_usage"),
        "system" => Some("system_events"),
        "summary" => Some("summaries"),
        "queue-operation" => None, // Dropped in migration 005
        "file-history-snapshot" => None, // No target table
        _ => None,
    }
}

/// Build a HashSet of column names for a given table using PRAGMA table_info.
///
/// Returns an empty set if the table does not exist or the pragma fails.
fn table_columns(conn: &Connection, table_name: &str) -> HashSet<String> {
    let mut columns = HashSet::new();
    // PRAGMA table_info cannot use parameterized queries, but table_name
    // comes from our own record_type_to_table mapping (not user input).
    let sql = format!("PRAGMA table_info({})", table_name);
    if let Ok(mut stmt) = conn.prepare(&sql) {
        if let Ok(rows) = stmt.query_map([], |row| row.get::<_, String>(1)) {
            for col in rows.flatten() {
                columns.insert(col);
            }
        }
    }
    columns
}

/// Drift entries grouped by version then record_type, with dynamic promotion status.
///
/// [VER-04] For each drift field, determines promotion_status by checking whether
/// the field_name matches an actual column on the target table:
/// - "promoted" if the field matches a real column
/// - "extra_json" if the target table has an extra_json column (overflow is captured)
/// - "unhandled" if neither condition applies or no target table exists
///
/// sample_value is truncated to 200 characters per CONTEXT decision.
pub fn drift_by_version(
    conn: &Connection,
) -> Result<Vec<VersionDriftGroup>, rusqlite::Error> {
    // Build column maps for all relevant tables (computed once, not per-field)
    let table_names = ["messages", "token_usage", "system_events", "summaries"];
    let mut table_column_cache: BTreeMap<&str, HashSet<String>> = BTreeMap::new();
    for table in &table_names {
        table_column_cache.insert(table, table_columns(conn, table));
    }

    // Query all drift entries with occurrence_count
    let mut stmt = conn.prepare(
        "SELECT field_name, record_type, version, sample_value,
                COALESCE(occurrence_count, 1) as occurrence_count,
                first_seen_at
         FROM schema_drift_log
         WHERE version IS NOT NULL
         ORDER BY version ASC, record_type ASC, field_name ASC",
    )?;

    // Intermediate: version -> record_type -> Vec<DriftFieldEntry>
    let mut groups: BTreeMap<String, BTreeMap<String, Vec<DriftFieldEntry>>> = BTreeMap::new();

    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, Option<String>>(3)?,
            row.get::<_, i64>(4)?,
            row.get::<_, String>(5)?,
        ))
    })?;

    for row in rows {
        let (field_name, record_type, version, sample_value, occurrence_count, first_seen_at) = row?;

        // Truncate sample_value to 200 chars
        let sample_value = sample_value.map(|s| {
            if s.len() > 200 {
                format!("{}...", &s[..200])
            } else {
                s
            }
        });

        // Compute promotion_status dynamically
        let promotion_status = match record_type_to_table(&record_type) {
            Some(table) => {
                let cols = table_column_cache.get(table).cloned().unwrap_or_default();
                if cols.contains(&field_name) {
                    "promoted".to_string()
                } else if cols.contains("extra_json") {
                    "extra_json".to_string()
                } else {
                    "unhandled".to_string()
                }
            }
            None => "unhandled".to_string(),
        };

        let entry = DriftFieldEntry {
            field_name,
            record_type: record_type.clone(),
            sample_value,
            occurrence_count,
            first_seen_at,
            promotion_status,
        };

        groups
            .entry(version)
            .or_default()
            .entry(record_type)
            .or_default()
            .push(entry);
    }

    // Convert BTreeMap structure to Vec<VersionDriftGroup>
    let result = groups
        .into_iter()
        .map(|(version, rt_map)| {
            let record_types = rt_map
                .into_iter()
                .map(|(record_type, fields)| RecordTypeDriftGroup {
                    record_type,
                    fields,
                })
                .collect();
            VersionDriftGroup {
                version,
                record_types,
            }
        })
        .collect();

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

    // -----------------------------------------------------------------------
    // Version monitoring query tests (Plan 06-03)
    // -----------------------------------------------------------------------

    #[test]
    fn test_version_history_enhanced() {
        let conn = setup_db();

        // Insert version_history entries directly
        conn.execute(
            "INSERT INTO version_history (version, first_seen_at, last_seen_at, session_id, session_count, new_fields_count)
             VALUES ('1.0.0', '2026-01-01T00:00:00Z', '2026-01-02T00:00:00Z', 'sess-1', 3, 2)",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO version_history (version, first_seen_at, last_seen_at, session_id, session_count, new_fields_count)
             VALUES ('1.1.0', '2026-01-03T00:00:00Z', '2026-01-04T00:00:00Z', 'sess-2', 5, 1)",
            [],
        ).unwrap();

        let entries = version_history_enhanced(&conn).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].version, "1.0.0");
        assert_eq!(entries[0].session_count, 3);
        assert_eq!(entries[0].new_fields_count, 2);
        assert_eq!(entries[1].version, "1.1.0");
        assert_eq!(entries[1].session_count, 5);
        assert_eq!(entries[1].new_fields_count, 1);
    }

    #[test]
    fn test_drift_by_version_grouping() {
        let conn = setup_db();

        // Insert drift entries for two versions
        conn.execute(
            "INSERT INTO schema_drift_log (field_name, record_type, version, sample_value, first_seen_at, occurrence_count, last_seen_at)
             VALUES ('new_field_a', 'user', '1.0.0', 'sample_a', '2026-01-01T00:00:00Z', 3, '2026-01-02T00:00:00Z')",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO schema_drift_log (field_name, record_type, version, sample_value, first_seen_at, occurrence_count, last_seen_at)
             VALUES ('new_field_b', 'assistant', '1.0.0', 'sample_b', '2026-01-01T00:00:00Z', 1, '2026-01-01T00:00:00Z')",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO schema_drift_log (field_name, record_type, version, sample_value, first_seen_at, occurrence_count, last_seen_at)
             VALUES ('new_field_c', 'user', '1.1.0', 'sample_c', '2026-01-03T00:00:00Z', 2, '2026-01-04T00:00:00Z')",
            [],
        ).unwrap();

        let groups = drift_by_version(&conn).unwrap();
        assert_eq!(groups.len(), 2, "Should have 2 version groups");

        // Version 1.0.0 should have 2 record_type groups: "assistant" and "user"
        let v1 = &groups[0];
        assert_eq!(v1.version, "1.0.0");
        assert_eq!(v1.record_types.len(), 2);
        // BTreeMap ordering: "assistant" before "user"
        assert_eq!(v1.record_types[0].record_type, "assistant");
        assert_eq!(v1.record_types[0].fields.len(), 1);
        assert_eq!(v1.record_types[1].record_type, "user");
        assert_eq!(v1.record_types[1].fields.len(), 1);

        // Version 1.1.0 should have 1 record_type group: "user"
        let v2 = &groups[1];
        assert_eq!(v2.version, "1.1.0");
        assert_eq!(v2.record_types.len(), 1);
        assert_eq!(v2.record_types[0].record_type, "user");
        assert_eq!(v2.record_types[0].fields[0].occurrence_count, 2);
    }

    #[test]
    fn test_drift_promotion_status() {
        let conn = setup_db();

        // Insert a drift field named "is_compact_summary" for record_type "user".
        // Since "user" maps to the "messages" table, and messages has a real column
        // named "is_compact_summary" (added in migration 006), this should get
        // promotion_status "promoted".
        conn.execute(
            "INSERT INTO schema_drift_log (field_name, record_type, version, sample_value, first_seen_at, occurrence_count, last_seen_at)
             VALUES ('is_compact_summary', 'user', '1.0.0', '1', '2026-01-01T00:00:00Z', 5, '2026-01-05T00:00:00Z')",
            [],
        ).unwrap();

        // Insert a drift field with a name that does NOT match any messages column.
        // Messages has extra_json, so this should get "extra_json" status.
        conn.execute(
            "INSERT INTO schema_drift_log (field_name, record_type, version, sample_value, first_seen_at, occurrence_count, last_seen_at)
             VALUES ('unknown_exotic_field', 'user', '1.0.0', 'exotic', '2026-01-01T00:00:00Z', 1, '2026-01-01T00:00:00Z')",
            [],
        ).unwrap();

        // Insert a drift field for a record_type with no target table.
        // Should get "unhandled" status.
        conn.execute(
            "INSERT INTO schema_drift_log (field_name, record_type, version, sample_value, first_seen_at, occurrence_count, last_seen_at)
             VALUES ('some_field', 'file-history-snapshot', '1.0.0', 'val', '2026-01-01T00:00:00Z', 1, '2026-01-01T00:00:00Z')",
            [],
        ).unwrap();

        let groups = drift_by_version(&conn).unwrap();
        assert_eq!(groups.len(), 1);

        let v = &groups[0];
        assert_eq!(v.version, "1.0.0");

        // Find the file-history-snapshot group
        let fhs_group = v.record_types.iter().find(|rt| rt.record_type == "file-history-snapshot").unwrap();
        assert_eq!(fhs_group.fields[0].promotion_status, "unhandled");

        // Find the user group
        let user_group = v.record_types.iter().find(|rt| rt.record_type == "user").unwrap();
        assert_eq!(user_group.fields.len(), 2);

        // Find is_compact_summary -> should be "promoted"
        let promoted = user_group.fields.iter().find(|f| f.field_name == "is_compact_summary").unwrap();
        assert_eq!(promoted.promotion_status, "promoted");

        // Find unknown_exotic_field -> should be "extra_json"
        let extra = user_group.fields.iter().find(|f| f.field_name == "unknown_exotic_field").unwrap();
        assert_eq!(extra.promotion_status, "extra_json");
    }

    // -----------------------------------------------------------------------
    // record_type_drift_list — B1.2
    //
    // Cover three filter combinations that mirror the CLI subcommand surface:
    // unfiltered listing, type_name substring filter, and version exact match.
    // These exercise the parameterized SQL path in record_type_drift_list and
    // confirm ordering by last_seen_at descending.
    // -----------------------------------------------------------------------

    fn seed_record_type_drift(
        conn: &Connection,
        type_name: &str,
        version: Option<&str>,
        sample: &str,
        last_seen: &str,
        occurrence_count: i64,
    ) {
        conn.execute(
            "INSERT INTO record_type_drift_log
             (type_name, version, sample_value, first_seen_at, last_seen_at, occurrence_count)
             VALUES (?1, ?2, ?3, ?4, ?4, ?5)",
            rusqlite::params![type_name, version, sample, last_seen, occurrence_count],
        )
        .unwrap();
    }

    #[test]
    fn test_record_type_drift_list_no_filters() {
        let conn = setup_db();
        seed_record_type_drift(&conn, "attachment", Some("2.1.126"), "{}", "2026-05-01T00:00:00", 100);
        seed_record_type_drift(&conn, "last-prompt", None, "{}", "2026-05-02T00:00:00", 50);
        seed_record_type_drift(&conn, "custom-title", Some("2.1.121"), "{}", "2026-05-03T00:00:00", 25);

        let entries = record_type_drift_list(&conn, None, None, None, None).unwrap();
        assert_eq!(entries.len(), 3);
        // Ordered by last_seen_at DESC.
        assert_eq!(entries[0].type_name, "custom-title");
        assert_eq!(entries[1].type_name, "last-prompt");
        assert_eq!(entries[2].type_name, "attachment");
    }

    #[test]
    fn test_record_type_drift_list_type_name_substring() {
        let conn = setup_db();
        seed_record_type_drift(&conn, "attachment", Some("2.1.126"), "{}", "2026-05-01T00:00:00", 100);
        seed_record_type_drift(&conn, "last-prompt", None, "{}", "2026-05-02T00:00:00", 50);
        seed_record_type_drift(&conn, "permission-mode", Some("2.1.126"), "{}", "2026-05-03T00:00:00", 12);

        let entries =
            record_type_drift_list(&conn, Some("prompt"), None, None, None).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].type_name, "last-prompt");
    }

    #[test]
    fn test_record_type_drift_list_version_and_limit() {
        let conn = setup_db();
        seed_record_type_drift(&conn, "attachment", Some("2.1.126"), "{}", "2026-05-01T00:00:00", 100);
        seed_record_type_drift(&conn, "permission-mode", Some("2.1.126"), "{}", "2026-05-02T00:00:00", 12);
        seed_record_type_drift(&conn, "agent-name", Some("2.1.121"), "{}", "2026-05-03T00:00:00", 8);

        let entries =
            record_type_drift_list(&conn, None, Some("2.1.126"), None, None).unwrap();
        assert_eq!(entries.len(), 2, "version filter narrows to two rows");
        assert!(entries.iter().all(|e| e.version.as_deref() == Some("2.1.126")));

        let limited = record_type_drift_list(&conn, None, None, None, Some(1)).unwrap();
        assert_eq!(limited.len(), 1);
    }

    #[test]
    fn test_record_type_drift_list_since_filter() {
        let conn = setup_db();
        seed_record_type_drift(&conn, "old-type", Some("2.0.0"), "{}", "2026-04-01T00:00:00", 1);
        seed_record_type_drift(&conn, "new-type", Some("2.1.126"), "{}", "2026-05-01T00:00:00", 1);

        let entries =
            record_type_drift_list(&conn, None, None, Some("2026-04-15T00:00:00"), None).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].type_name, "new-type");
    }

    // -----------------------------------------------------------------------
    // attachments_list / attachment_by_uuid / hook_executions_list — C1.4
    //
    // These exercise the parameterized SQL paths that the new CLI / REST /
    // MCP surfaces consume. Filters covered: project substring, inner_type
    // exact match, since lower bound, limit truncation, attachment_by_uuid
    // hit + miss, hook_executions tool_use_id / hook_event / exit_code
    // filters, and ordering.
    // -----------------------------------------------------------------------

    fn seed_session(conn: &Connection, session_id: &str, project_path: Option<&str>) {
        conn.execute(
            "INSERT OR IGNORE INTO sessions (session_id, project_path, first_seen_at)
             VALUES (?1, ?2, '2026-01-01T00:00:00Z')",
            rusqlite::params![session_id, project_path],
        )
        .unwrap();
    }

    fn seed_attachment(
        conn: &Connection,
        uuid: &str,
        session_id: &str,
        timestamp: &str,
        inner_type: &str,
        body_json: Option<&str>,
    ) {
        conn.execute(
            "INSERT INTO attachments
             (uuid, session_id, parent_uuid, timestamp, cwd, version, git_branch,
              slug, entrypoint, inner_type, body_json)
             VALUES (?1, ?2, NULL, ?3, NULL, NULL, NULL, NULL, NULL, ?4, ?5)",
            rusqlite::params![uuid, session_id, timestamp, inner_type, body_json],
        )
        .unwrap();
    }

    fn seed_hook_execution(
        conn: &Connection,
        attachment_uuid: &str,
        hook_event: Option<&str>,
        tool_use_id: Option<&str>,
        exit_code: Option<i64>,
    ) {
        conn.execute(
            "INSERT INTO hook_executions
             (attachment_uuid, hook_name, hook_event, tool_use_id, exit_code,
              duration_ms, stdout, stderr, command, decision)
             VALUES (?1, NULL, ?2, ?3, ?4, NULL, NULL, NULL, NULL, NULL)",
            rusqlite::params![attachment_uuid, hook_event, tool_use_id, exit_code],
        )
        .unwrap();
    }

    #[test]
    fn test_attachments_list_no_filters_orders_by_timestamp_desc() {
        let conn = setup_db();
        seed_session(&conn, "s-1", Some("/Users/dev/Projects/alpha"));
        seed_attachment(&conn, "att-a", "s-1", "2026-05-01T00:00:00Z", "hook_success", Some("{}"));
        seed_attachment(&conn, "att-b", "s-1", "2026-05-03T00:00:00Z", "skill_listing", None);
        seed_attachment(&conn, "att-c", "s-1", "2026-05-02T00:00:00Z", "hook_success", Some("{}"));

        let rows = attachments_list(&conn, None, None, None, 50).unwrap();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].uuid, "att-b", "newest first");
        assert_eq!(rows[1].uuid, "att-c");
        assert_eq!(rows[2].uuid, "att-a");
    }

    #[test]
    fn test_attachments_list_inner_type_filter() {
        let conn = setup_db();
        seed_session(&conn, "s-1", Some("/Users/dev/Projects/alpha"));
        seed_attachment(&conn, "att-a", "s-1", "2026-05-01T00:00:00Z", "hook_success", Some("{}"));
        seed_attachment(&conn, "att-b", "s-1", "2026-05-02T00:00:00Z", "skill_listing", None);

        let rows = attachments_list(&conn, None, Some("hook_success"), None, 50).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].uuid, "att-a");
    }

    #[test]
    fn test_attachments_list_project_substring_filter() {
        let conn = setup_db();
        seed_session(&conn, "s-alpha", Some("/Users/dev/Projects/alpha"));
        seed_session(&conn, "s-beta", Some("/Users/dev/Projects/beta"));
        seed_attachment(&conn, "att-a", "s-alpha", "2026-05-01T00:00:00Z", "hook_success", Some("{}"));
        seed_attachment(&conn, "att-b", "s-beta", "2026-05-02T00:00:00Z", "hook_success", Some("{}"));

        let rows = attachments_list(&conn, Some("alpha"), None, None, 50).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].uuid, "att-a");
    }

    #[test]
    fn test_attachments_list_since_and_limit() {
        let conn = setup_db();
        seed_session(&conn, "s-1", Some("/Users/dev/Projects/alpha"));
        seed_attachment(&conn, "att-a", "s-1", "2026-04-01T00:00:00Z", "hook_success", None);
        seed_attachment(&conn, "att-b", "s-1", "2026-05-01T00:00:00Z", "hook_success", None);
        seed_attachment(&conn, "att-c", "s-1", "2026-06-01T00:00:00Z", "hook_success", None);

        let rows =
            attachments_list(&conn, None, None, Some("2026-04-15T00:00:00Z"), 50).unwrap();
        assert_eq!(rows.len(), 2);
        // Newest first.
        assert_eq!(rows[0].uuid, "att-c");
        assert_eq!(rows[1].uuid, "att-b");

        // Limit caps the result.
        let limited = attachments_list(&conn, None, None, None, 1).unwrap();
        assert_eq!(limited.len(), 1);
        assert_eq!(limited[0].uuid, "att-c");
    }

    #[test]
    fn test_attachment_by_uuid_hit_and_miss() {
        let conn = setup_db();
        seed_session(&conn, "s-1", None);
        seed_attachment(
            &conn,
            "att-a",
            "s-1",
            "2026-05-01T00:00:00Z",
            "skill_listing",
            Some(r#"{"name":"x"}"#),
        );

        let hit = attachment_by_uuid(&conn, "att-a").unwrap();
        assert!(hit.is_some());
        let row = hit.unwrap();
        assert_eq!(row.inner_type, "skill_listing");
        assert_eq!(row.body_json.as_deref(), Some(r#"{"name":"x"}"#));

        let miss = attachment_by_uuid(&conn, "does-not-exist").unwrap();
        assert!(miss.is_none());
    }

    #[test]
    fn test_hook_executions_list_no_filters() {
        let conn = setup_db();
        seed_session(&conn, "s-1", None);
        seed_attachment(&conn, "att-a", "s-1", "2026-05-01T00:00:00Z", "hook_success", None);
        seed_hook_execution(&conn, "att-a", Some("PreToolUse"), Some("tu-1"), Some(0));
        seed_hook_execution(&conn, "att-a", Some("PostToolUse"), Some("tu-1"), Some(0));

        let rows = hook_executions_list(&conn, None, None, None, 50).unwrap();
        assert_eq!(rows.len(), 2);
    }

    #[test]
    fn test_hook_executions_list_filters_combine() {
        let conn = setup_db();
        seed_session(&conn, "s-1", None);
        seed_attachment(&conn, "att-a", "s-1", "2026-05-01T00:00:00Z", "hook_success", None);
        seed_hook_execution(&conn, "att-a", Some("PreToolUse"), Some("tu-1"), Some(0));
        seed_hook_execution(&conn, "att-a", Some("PostToolUse"), Some("tu-1"), Some(0));
        seed_hook_execution(&conn, "att-a", Some("PostToolUse"), Some("tu-2"), Some(2));

        let pre = hook_executions_list(&conn, None, Some("PreToolUse"), None, 50).unwrap();
        assert_eq!(pre.len(), 1);
        assert_eq!(pre[0].tool_use_id.as_deref(), Some("tu-1"));

        let by_tu = hook_executions_list(&conn, Some("tu-1"), None, None, 50).unwrap();
        assert_eq!(by_tu.len(), 2);

        let by_exit = hook_executions_list(&conn, None, None, Some(2), 50).unwrap();
        assert_eq!(by_exit.len(), 1);
        assert_eq!(by_exit[0].tool_use_id.as_deref(), Some("tu-2"));

        let limited = hook_executions_list(&conn, None, None, None, 2).unwrap();
        assert_eq!(limited.len(), 2);
    }
}
