//! Query functions for the artifact layer: files, file operations, git operations,
//! content reconstruction, unified diff generation, and session timelines.
//!
//! Each function takes a `&rusqlite::Connection` and returns
//! `Result<T, rusqlite::Error>` with Serialize+Debug result structs.
//! Follows the same pattern as `query.rs` (PAT-022): dynamic WHERE clauses use
//! `Vec<Box<dyn rusqlite::types::ToSql>>` with `params_from_iter`.
//!
//! The reconstruction algorithm replays Write and Edit operations in timestamp
//! order to reproduce file content at any point in a session. The diff generator
//! uses the `similar` crate for unified diff output.
//!
//! Requirement IDs: ART-10, ART-11, API-17, API-18, API-19, API-20,
//!                  API-23, API-24, API-25, API-26, API-27, CLI-10,
//!                  CLI-11, CLI-12, CLI-13, CLI-14

use rusqlite::Connection;
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Result structs
// ---------------------------------------------------------------------------

/// A tracked file entry from the `files` table.
/// Represents a unique file path observed within a session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileEntry {
    pub id: i64,
    pub session_id: String,
    pub file_path: String,
    pub first_seen: String,
    pub last_modified: String,
    pub operation_count: i64,
}

/// A single file operation from the `file_operations` table.
/// Represents a write, edit, read, or bash_* operation on a file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileOperation {
    pub id: i64,
    pub session_id: String,
    pub file_path: String,
    pub operation_type: String,
    pub content: Option<String>,
    pub old_content: Option<String>,
    pub command: Option<String>,
    pub tool_use_id: Option<String>,
    pub message_uuid: Option<String>,
    pub timestamp: String,
}

/// A git operation extracted from Bash commands.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitOperation {
    pub id: i64,
    pub session_id: String,
    pub operation_type: String,
    pub command: String,
    pub commit_message: Option<String>,
    pub branch: Option<String>,
    pub tool_use_id: Option<String>,
    pub message_uuid: Option<String>,
    pub timestamp: String,
}

/// A chronologically ordered entry in a session timeline.
/// Unifies file operations, git operations, and tool executions into
/// a single stream ordered by timestamp.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimelineEntry {
    /// One of: "file_operation", "git_operation", "tool_execution"
    pub entry_type: String,
    /// write, edit, read, commit, push, tool_name, etc.
    pub operation_type: String,
    pub file_path: Option<String>,
    pub commit_message: Option<String>,
    pub branch: Option<String>,
    /// Populated for tool_execution entries
    pub tool_name: Option<String>,
    /// Truncated result_content (first 500 chars) for tool_execution entries
    pub result_summary: Option<String>,
    pub message_uuid: Option<String>,
    pub timestamp: String,
}

/// A tool execution entry for session artifact summaries.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolExecutionEntry {
    pub tool_use_id: String,
    pub tool_name: String,
    pub input_json: Option<String>,
    pub result_summary: Option<String>,
    pub is_error: Option<bool>,
    pub message_uuid: String,
    pub timestamp: String,
}

/// Combined artifacts for a session: files, git operations, and tool executions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionArtifacts {
    pub files: Vec<FileEntry>,
    pub git_operations: Vec<GitOperation>,
    pub tool_executions: Vec<ToolExecutionEntry>,
}

// ---------------------------------------------------------------------------
// Query functions
// ---------------------------------------------------------------------------

/// List files tracked across sessions with optional filters.
///
/// [API-17, CLI-10] Returns files ordered by last_modified descending.
/// Supports optional session_id and path substring filters.
pub fn list_files(
    conn: &Connection,
    session_id: Option<&str>,
    path_contains: Option<&str>,
    limit: usize,
) -> Result<Vec<FileEntry>, rusqlite::Error> {
    let mut conditions = Vec::new();
    let mut param_values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

    if let Some(sid) = session_id {
        conditions.push(format!("f.session_id = ?{}", param_values.len() + 1));
        param_values.push(Box::new(sid.to_string()));
    }
    if let Some(path) = path_contains {
        conditions.push(format!("f.file_path LIKE ?{}", param_values.len() + 1));
        param_values.push(Box::new(format!("%{}%", path)));
    }

    let where_clause = if conditions.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", conditions.join(" AND "))
    };

    let sql = format!(
        "SELECT id, session_id, file_path, first_seen, last_modified, operation_count
         FROM files f
         {}
         ORDER BY f.last_modified DESC
         LIMIT ?{}",
        where_clause,
        param_values.len() + 1
    );
    param_values.push(Box::new(limit as i64));

    let params: Vec<&dyn rusqlite::types::ToSql> =
        param_values.iter().map(|b| b.as_ref()).collect();

    let mut stmt = conn.prepare(&sql)?;
    let results = stmt.query_map(rusqlite::params_from_iter(params), |row| {
        Ok(FileEntry {
            id: row.get(0)?,
            session_id: row.get(1)?,
            file_path: row.get(2)?,
            first_seen: row.get(3)?,
            last_modified: row.get(4)?,
            operation_count: row.get(5)?,
        })
    })?;

    results.collect()
}

/// Get a single file entry by ID.
///
/// [API-18] Returns None if the file ID does not exist.
pub fn get_file(
    conn: &Connection,
    file_id: i64,
) -> Result<Option<FileEntry>, rusqlite::Error> {
    conn.query_row(
        "SELECT id, session_id, file_path, first_seen, last_modified, operation_count
         FROM files WHERE id = ?1",
        rusqlite::params![file_id],
        |row| {
            Ok(FileEntry {
                id: row.get(0)?,
                session_id: row.get(1)?,
                file_path: row.get(2)?,
                first_seen: row.get(3)?,
                last_modified: row.get(4)?,
                operation_count: row.get(5)?,
            })
        },
    )
    .optional()
}

/// Query file operations for a given file path.
///
/// [API-18, CLI-11] Returns operations ordered by timestamp ascending.
/// Supports optional session_id filter.
pub fn query_file_operations(
    conn: &Connection,
    file_path: &str,
    session_id: Option<&str>,
    limit: usize,
) -> Result<Vec<FileOperation>, rusqlite::Error> {
    let mut conditions = vec!["fo.file_path = ?1".to_string()];
    let mut param_values: Vec<Box<dyn rusqlite::types::ToSql>> =
        vec![Box::new(file_path.to_string())];

    if let Some(sid) = session_id {
        conditions.push(format!("fo.session_id = ?{}", param_values.len() + 1));
        param_values.push(Box::new(sid.to_string()));
    }

    let where_clause = format!("WHERE {}", conditions.join(" AND "));

    let sql = format!(
        "SELECT id, session_id, file_path, operation_type, content, old_content,
                command, tool_use_id, message_uuid, timestamp
         FROM file_operations fo
         {}
         ORDER BY fo.timestamp ASC
         LIMIT ?{}",
        where_clause,
        param_values.len() + 1
    );
    param_values.push(Box::new(limit as i64));

    let params: Vec<&dyn rusqlite::types::ToSql> =
        param_values.iter().map(|b| b.as_ref()).collect();

    let mut stmt = conn.prepare(&sql)?;
    let results = stmt.query_map(rusqlite::params_from_iter(params), |row| {
        Ok(FileOperation {
            id: row.get(0)?,
            session_id: row.get(1)?,
            file_path: row.get(2)?,
            operation_type: row.get(3)?,
            content: row.get(4)?,
            old_content: row.get(5)?,
            command: row.get(6)?,
            tool_use_id: row.get(7)?,
            message_uuid: row.get(8)?,
            timestamp: row.get(9)?,
        })
    })?;

    results.collect()
}

/// Query all file operations for a file in a session, ordered for replay.
///
/// Internal function used by reconstruction and diff generation.
/// Returns all operations in timestamp order with optional cutoff.
/// No LIMIT -- reconstruction needs the complete operation sequence.
pub fn query_file_operations_ordered(
    conn: &Connection,
    file_path: &str,
    session_id: &str,
    cutoff_timestamp: Option<&str>,
) -> Result<Vec<FileOperation>, rusqlite::Error> {
    let mut conditions = vec![
        "fo.file_path = ?1".to_string(),
        "fo.session_id = ?2".to_string(),
    ];
    let mut param_values: Vec<Box<dyn rusqlite::types::ToSql>> = vec![
        Box::new(file_path.to_string()),
        Box::new(session_id.to_string()),
    ];

    if let Some(cutoff) = cutoff_timestamp {
        conditions.push(format!("fo.timestamp <= ?{}", param_values.len() + 1));
        param_values.push(Box::new(cutoff.to_string()));
    }

    let where_clause = format!("WHERE {}", conditions.join(" AND "));

    let sql = format!(
        "SELECT id, session_id, file_path, operation_type, content, old_content,
                command, tool_use_id, message_uuid, timestamp
         FROM file_operations fo
         {}
         ORDER BY fo.timestamp ASC",
        where_clause
    );

    let params: Vec<&dyn rusqlite::types::ToSql> =
        param_values.iter().map(|b| b.as_ref()).collect();

    let mut stmt = conn.prepare(&sql)?;
    let results = stmt.query_map(rusqlite::params_from_iter(params), |row| {
        Ok(FileOperation {
            id: row.get(0)?,
            session_id: row.get(1)?,
            file_path: row.get(2)?,
            operation_type: row.get(3)?,
            content: row.get(4)?,
            old_content: row.get(5)?,
            command: row.get(6)?,
            tool_use_id: row.get(7)?,
            message_uuid: row.get(8)?,
            timestamp: row.get(9)?,
        })
    })?;

    results.collect()
}

/// Reconstruct file content by replaying Write and Edit operations.
///
/// [ART-10, API-19, CLI-12] Replays operations in timestamp order:
/// - "write" => replaces content entirely (full file write)
/// - "edit" => applies string replacement (old_content -> content)
/// - Other types (read, bash_*) => no mutation
///
/// If `at_message_uuid` is provided, looks up its timestamp from the
/// messages table and only replays operations up to that point.
/// Returns None if no write operations establish initial content.
pub fn reconstruct_file_content(
    conn: &Connection,
    file_path: &str,
    session_id: &str,
    at_message_uuid: Option<&str>,
) -> Result<Option<String>, rusqlite::Error> {
    // If at_message_uuid provided, look up its timestamp for the cutoff
    let cutoff_timestamp: Option<String> = if let Some(uuid) = at_message_uuid {
        conn.query_row(
            "SELECT timestamp FROM messages WHERE uuid = ?1",
            rusqlite::params![uuid],
            |row| row.get(0),
        )
        .optional()?
    } else {
        None
    };

    let ops = query_file_operations_ordered(
        conn,
        file_path,
        session_id,
        cutoff_timestamp.as_deref(),
    )?;

    let mut content: Option<String> = None;

    for op in &ops {
        match op.operation_type.as_str() {
            "write" => {
                // Write replaces entire file content
                if let Some(ref c) = op.content {
                    content = Some(c.clone());
                }
            }
            "edit" => {
                // Edit applies string replacement within existing content
                if let (Some(ref current), Some(ref old_str), Some(ref new_str)) =
                    (&content, &op.old_content, &op.content)
                {
                    content = Some(current.replace(old_str, new_str));
                }
            }
            _ => {
                // read, bash_* operations do not mutate content
            }
        }
    }

    Ok(content)
}

/// Generate unified diff output for all mutations to a file in a session.
///
/// [ART-11, API-20] Replays writes and edits, producing a unified diff at
/// each mutation step using `similar::TextDiff::from_lines()`.
/// Returns the accumulated diff string.
pub fn generate_file_diff(
    conn: &Connection,
    file_path: &str,
    session_id: &str,
) -> Result<String, rusqlite::Error> {
    let ops = query_file_operations_ordered(conn, file_path, session_id, None)?;

    let mut diffs = Vec::new();
    let mut content: Option<String> = None;

    for op in &ops {
        match op.operation_type.as_str() {
            "write" => {
                if let Some(ref new_content) = op.content {
                    let old = content.as_deref().unwrap_or("");
                    let diff = similar::TextDiff::from_lines(old, new_content);
                    let unified = diff
                        .unified_diff()
                        .context_radius(3)
                        .header(
                            &format!("a/{}", file_path),
                            &format!("b/{}", file_path),
                        )
                        .to_string();
                    if !unified.is_empty() {
                        diffs.push(unified);
                    }
                    content = Some(new_content.clone());
                }
            }
            "edit" => {
                if let (Some(ref current), Some(ref old_str), Some(ref new_str)) =
                    (&content, &op.old_content, &op.content)
                {
                    let new_content = current.replace(old_str, new_str);
                    let diff = similar::TextDiff::from_lines(current.as_str(), &new_content);
                    let unified = diff
                        .unified_diff()
                        .context_radius(3)
                        .header(
                            &format!("a/{}", file_path),
                            &format!("b/{}", file_path),
                        )
                        .to_string();
                    if !unified.is_empty() {
                        diffs.push(unified);
                    }
                    content = Some(new_content);
                }
            }
            _ => {}
        }
    }

    Ok(diffs.join("\n"))
}

/// List git operations with optional filters.
///
/// [API-23, CLI-13] Returns git operations ordered by timestamp descending.
/// Supports optional session_id and operation_type filters.
pub fn list_git_operations(
    conn: &Connection,
    session_id: Option<&str>,
    operation_type: Option<&str>,
    limit: usize,
) -> Result<Vec<GitOperation>, rusqlite::Error> {
    let mut conditions = Vec::new();
    let mut param_values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

    if let Some(sid) = session_id {
        conditions.push(format!("go.session_id = ?{}", param_values.len() + 1));
        param_values.push(Box::new(sid.to_string()));
    }
    if let Some(ot) = operation_type {
        conditions.push(format!("go.operation_type = ?{}", param_values.len() + 1));
        param_values.push(Box::new(ot.to_string()));
    }

    let where_clause = if conditions.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", conditions.join(" AND "))
    };

    let sql = format!(
        "SELECT id, session_id, operation_type, command, commit_message, branch,
                tool_use_id, message_uuid, timestamp
         FROM git_operations go
         {}
         ORDER BY go.timestamp DESC
         LIMIT ?{}",
        where_clause,
        param_values.len() + 1
    );
    param_values.push(Box::new(limit as i64));

    let params: Vec<&dyn rusqlite::types::ToSql> =
        param_values.iter().map(|b| b.as_ref()).collect();

    let mut stmt = conn.prepare(&sql)?;
    let results = stmt.query_map(rusqlite::params_from_iter(params), |row| {
        Ok(GitOperation {
            id: row.get(0)?,
            session_id: row.get(1)?,
            operation_type: row.get(2)?,
            command: row.get(3)?,
            commit_message: row.get(4)?,
            branch: row.get(5)?,
            tool_use_id: row.get(6)?,
            message_uuid: row.get(7)?,
            timestamp: row.get(8)?,
        })
    })?;

    results.collect()
}

/// List git commit operations specifically.
///
/// [API-24, API-25] Filters to operation_type = 'commit' only.
/// Optional session_id filter, ordered by timestamp descending.
pub fn list_git_commits(
    conn: &Connection,
    session_id: Option<&str>,
    limit: usize,
) -> Result<Vec<GitOperation>, rusqlite::Error> {
    let mut param_values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

    let session_filter = if let Some(sid) = session_id {
        param_values.push(Box::new(sid.to_string()));
        format!("AND go.session_id = ?{}", param_values.len())
    } else {
        String::new()
    };

    let sql = format!(
        "SELECT id, session_id, operation_type, command, commit_message, branch,
                tool_use_id, message_uuid, timestamp
         FROM git_operations go
         WHERE go.operation_type = 'commit' {}
         ORDER BY go.timestamp DESC
         LIMIT ?{}",
        session_filter,
        param_values.len() + 1
    );
    param_values.push(Box::new(limit as i64));

    let params: Vec<&dyn rusqlite::types::ToSql> =
        param_values.iter().map(|b| b.as_ref()).collect();

    let mut stmt = conn.prepare(&sql)?;
    let results = stmt.query_map(rusqlite::params_from_iter(params), |row| {
        Ok(GitOperation {
            id: row.get(0)?,
            session_id: row.get(1)?,
            operation_type: row.get(2)?,
            command: row.get(3)?,
            commit_message: row.get(4)?,
            branch: row.get(5)?,
            tool_use_id: row.get(6)?,
            message_uuid: row.get(7)?,
            timestamp: row.get(8)?,
        })
    })?;

    results.collect()
}

/// Query combined session artifacts: files, git operations, and tool executions.
///
/// [API-26, CLI-14] Returns a composite struct with all artifact types.
/// Tool execution result_summary is truncated to 500 characters.
pub fn query_session_artifacts(
    conn: &Connection,
    session_id: &str,
) -> Result<SessionArtifacts, rusqlite::Error> {
    // 1. Files for this session
    let files = list_files(conn, Some(session_id), None, 10000)?;

    // 2. Git operations for this session
    let git_operations = list_git_operations(conn, Some(session_id), None, 10000)?;

    // 3. Tool executions for this session, joined with messages for timestamp
    let mut te_stmt = conn.prepare(
        "SELECT te.tool_use_id, te.tool_name, te.input_json, te.result_content,
                te.is_error, te.message_uuid, m.timestamp
         FROM tool_executions te
         JOIN messages m ON m.uuid = te.message_uuid
         WHERE m.session_id = ?1
         ORDER BY m.timestamp ASC",
    )?;

    let tool_executions: Vec<ToolExecutionEntry> = te_stmt
        .query_map(rusqlite::params![session_id], |row| {
            let result_content: Option<String> = row.get(3)?;
            let result_summary = result_content.map(|rc| {
                if rc.len() > 500 {
                    // Find a char boundary at or before byte 500 to avoid
                    // panicking on multi-byte UTF-8 sequences (e.g. '→')
                    let mut end = 500;
                    while !rc.is_char_boundary(end) {
                        end -= 1;
                    }
                    format!("{}...", &rc[..end])
                } else {
                    rc
                }
            });
            let is_error_int: Option<i32> = row.get(4)?;

            Ok(ToolExecutionEntry {
                tool_use_id: row.get(0)?,
                tool_name: row.get(1)?,
                input_json: row.get(2)?,
                result_summary,
                is_error: is_error_int.map(|v| v != 0),
                message_uuid: row.get(5)?,
                timestamp: row.get(6)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(SessionArtifacts {
        files,
        git_operations,
        tool_executions,
    })
}

/// Query a chronological timeline of all session activity.
///
/// [API-27] Returns a union of file operations, git operations, and tool
/// executions ordered by timestamp ascending. Uses UNION ALL for the three
/// sources, with each mapped to the TimelineEntry format.
pub fn query_session_timeline(
    conn: &Connection,
    session_id: &str,
    limit: usize,
) -> Result<Vec<TimelineEntry>, rusqlite::Error> {
    let mut stmt = conn.prepare(
        "SELECT entry_type, operation_type, file_path, commit_message, branch,
                tool_name, result_summary, message_uuid, timestamp
         FROM (
             SELECT
                 'file_operation' AS entry_type,
                 fo.operation_type AS operation_type,
                 fo.file_path AS file_path,
                 NULL AS commit_message,
                 NULL AS branch,
                 NULL AS tool_name,
                 NULL AS result_summary,
                 fo.message_uuid AS message_uuid,
                 fo.timestamp AS timestamp
             FROM file_operations fo
             WHERE fo.session_id = ?1

             UNION ALL

             SELECT
                 'git_operation' AS entry_type,
                 go.operation_type AS operation_type,
                 NULL AS file_path,
                 go.commit_message AS commit_message,
                 go.branch AS branch,
                 NULL AS tool_name,
                 NULL AS result_summary,
                 go.message_uuid AS message_uuid,
                 go.timestamp AS timestamp
             FROM git_operations go
             WHERE go.session_id = ?1

             UNION ALL

             SELECT
                 'tool_execution' AS entry_type,
                 te.tool_name AS operation_type,
                 NULL AS file_path,
                 NULL AS commit_message,
                 NULL AS branch,
                 te.tool_name AS tool_name,
                 CASE
                     WHEN LENGTH(te.result_content) > 500
                     THEN SUBSTR(te.result_content, 1, 500) || '...'
                     ELSE te.result_content
                 END AS result_summary,
                 te.message_uuid AS message_uuid,
                 m.timestamp AS timestamp
             FROM tool_executions te
             JOIN messages m ON m.uuid = te.message_uuid
             WHERE m.session_id = ?1
         )
         ORDER BY timestamp ASC
         LIMIT ?2",
    )?;

    let results = stmt.query_map(rusqlite::params![session_id, limit as i64], |row| {
        Ok(TimelineEntry {
            entry_type: row.get(0)?,
            operation_type: row.get(1)?,
            file_path: row.get(2)?,
            commit_message: row.get(3)?,
            branch: row.get(4)?,
            tool_name: row.get(5)?,
            result_summary: row.get(6)?,
            message_uuid: row.get(7)?,
            timestamp: row.get(8)?,
        })
    })?;

    results.collect()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema;
    use rusqlite::Connection;

    /// Create an in-memory SQLite database with all migrations applied.
    fn setup_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        schema::run_migrations(&conn).unwrap();
        conn
    }

    /// Insert a test session row to satisfy foreign key constraints.
    fn insert_test_session(conn: &Connection, session_id: &str) {
        conn.execute(
            "INSERT OR IGNORE INTO sessions (session_id, project_path, first_seen_at, version, slug, git_branch)
             VALUES (?1, '/test/project', '2026-02-20T00:00:00Z', '2.1.49', 'test', 'main')",
            rusqlite::params![session_id],
        )
        .unwrap();
    }

    /// Insert a test message row to satisfy foreign key constraints.
    fn insert_test_message(conn: &Connection, uuid: &str, session_id: &str, timestamp: &str) {
        conn.execute(
            "INSERT OR IGNORE INTO messages (uuid, session_id, type, timestamp)
             VALUES (?1, ?2, 'assistant', ?3)",
            rusqlite::params![uuid, session_id, timestamp],
        )
        .unwrap();
    }

    /// Insert a file row directly for testing.
    fn insert_file(conn: &Connection, session_id: &str, file_path: &str, op_count: i64) {
        conn.execute(
            "INSERT INTO files (session_id, file_path, first_seen, last_modified, operation_count)
             VALUES (?1, ?2, '2026-02-20T01:00:00Z', '2026-02-20T02:00:00Z', ?3)",
            rusqlite::params![session_id, file_path, op_count],
        )
        .unwrap();
    }

    /// Insert a file operation row directly for testing.
    fn insert_file_op(
        conn: &Connection,
        session_id: &str,
        file_path: &str,
        op_type: &str,
        content: Option<&str>,
        old_content: Option<&str>,
        tool_use_id: &str,
        message_uuid: &str,
        timestamp: &str,
    ) {
        conn.execute(
            "INSERT INTO file_operations
             (session_id, file_path, operation_type, content, old_content, command,
              tool_use_id, message_uuid, timestamp)
             VALUES (?1, ?2, ?3, ?4, ?5, NULL, ?6, ?7, ?8)",
            rusqlite::params![
                session_id, file_path, op_type, content, old_content,
                tool_use_id, message_uuid, timestamp
            ],
        )
        .unwrap();
    }

    /// Insert a git operation row directly for testing.
    fn insert_git_op(
        conn: &Connection,
        session_id: &str,
        op_type: &str,
        command: &str,
        commit_msg: Option<&str>,
        branch: Option<&str>,
        tool_use_id: &str,
        message_uuid: &str,
        timestamp: &str,
    ) {
        conn.execute(
            "INSERT INTO git_operations
             (session_id, operation_type, command, commit_message, branch,
              tool_use_id, message_uuid, timestamp)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            rusqlite::params![
                session_id, op_type, command, commit_msg, branch,
                tool_use_id, message_uuid, timestamp
            ],
        )
        .unwrap();
    }

    /// Insert a tool execution row directly for testing.
    fn insert_tool_execution(
        conn: &Connection,
        message_uuid: &str,
        tool_use_id: &str,
        tool_name: &str,
        result_content: Option<&str>,
        is_error: Option<i32>,
    ) {
        conn.execute(
            "INSERT INTO tool_executions
             (message_uuid, tool_use_id, tool_name, input_json, result_content, is_error)
             VALUES (?1, ?2, ?3, NULL, ?4, ?5)",
            rusqlite::params![message_uuid, tool_use_id, tool_name, result_content, is_error],
        )
        .unwrap();
    }

    // -------------------------------------------------------------------
    // Test 1: list_files returns inserted file entries
    // -------------------------------------------------------------------
    #[test]
    fn test_list_files_returns_entries() {
        let conn = setup_db();
        insert_test_session(&conn, "sess-lf");

        insert_file(&conn, "sess-lf", "/src/main.rs", 5);
        insert_file(&conn, "sess-lf", "/src/lib.rs", 3);

        let files = list_files(&conn, Some("sess-lf"), None, 100).unwrap();
        assert_eq!(files.len(), 2, "Should return 2 file entries");

        // Verify path_contains filter
        let filtered = list_files(&conn, Some("sess-lf"), Some("main"), 100).unwrap();
        assert_eq!(filtered.len(), 1, "Path filter should match 1 file");
        assert_eq!(filtered[0].file_path, "/src/main.rs");
    }

    // -------------------------------------------------------------------
    // Test 2: reconstruct_file_content with Write then Edit
    // -------------------------------------------------------------------
    #[test]
    fn test_reconstruct_file_content_write_then_edit() {
        let conn = setup_db();
        insert_test_session(&conn, "sess-rc");
        insert_test_message(&conn, "msg-rc-1", "sess-rc", "2026-02-20T01:00:00Z");
        insert_test_message(&conn, "msg-rc-2", "sess-rc", "2026-02-20T01:01:00Z");

        // First operation: Write entire file
        insert_file_op(
            &conn, "sess-rc", "/src/app.rs", "write",
            Some("fn main() {\n    println!(\"hello\");\n}\n"),
            None,
            "tool-rc-001", "msg-rc-1", "2026-02-20T01:00:00Z",
        );

        // Second operation: Edit -- replace "hello" with "world"
        insert_file_op(
            &conn, "sess-rc", "/src/app.rs", "edit",
            Some("world"),  // new_string stored in content
            Some("hello"),  // old_string stored in old_content
            "tool-rc-002", "msg-rc-2", "2026-02-20T01:01:00Z",
        );

        let result = reconstruct_file_content(&conn, "/src/app.rs", "sess-rc", None).unwrap();
        assert!(result.is_some(), "Should reconstruct content");

        let content = result.unwrap();
        assert!(
            content.contains("world"),
            "Edit should have replaced 'hello' with 'world', got: {}",
            content
        );
        assert!(
            !content.contains("hello"),
            "Original 'hello' should be gone, got: {}",
            content
        );
        assert_eq!(
            content,
            "fn main() {\n    println!(\"world\");\n}\n",
            "Full content should match expected"
        );
    }

    // -------------------------------------------------------------------
    // Test 3: reconstruct_file_content with at_message_uuid cutoff
    // -------------------------------------------------------------------
    #[test]
    fn test_reconstruct_file_content_with_cutoff() {
        let conn = setup_db();
        insert_test_session(&conn, "sess-cut");
        insert_test_message(&conn, "msg-cut-1", "sess-cut", "2026-02-20T01:00:00Z");
        insert_test_message(&conn, "msg-cut-2", "sess-cut", "2026-02-20T01:01:00Z");
        insert_test_message(&conn, "msg-cut-3", "sess-cut", "2026-02-20T01:02:00Z");

        // Write initial content
        insert_file_op(
            &conn, "sess-cut", "/src/lib.rs", "write",
            Some("// version 1\n"),
            None,
            "tool-cut-001", "msg-cut-1", "2026-02-20T01:00:00Z",
        );

        // Edit at T+1 min
        insert_file_op(
            &conn, "sess-cut", "/src/lib.rs", "edit",
            Some("// version 2\n"),
            Some("// version 1\n"),
            "tool-cut-002", "msg-cut-2", "2026-02-20T01:01:00Z",
        );

        // Edit at T+2 min -- this should be excluded by cutoff
        insert_file_op(
            &conn, "sess-cut", "/src/lib.rs", "edit",
            Some("// version 3\n"),
            Some("// version 2\n"),
            "tool-cut-003", "msg-cut-3", "2026-02-20T01:02:00Z",
        );

        // Reconstruct at msg-cut-2 (should see only Write + first Edit)
        let result = reconstruct_file_content(
            &conn, "/src/lib.rs", "sess-cut", Some("msg-cut-2"),
        )
        .unwrap();

        let content = result.unwrap();
        assert_eq!(
            content, "// version 2\n",
            "Should stop at version 2 (cutoff at msg-cut-2 timestamp)"
        );
    }

    // -------------------------------------------------------------------
    // Test 4: generate_file_diff produces valid unified diff output
    // -------------------------------------------------------------------
    #[test]
    fn test_generate_file_diff_produces_unified_diff() {
        let conn = setup_db();
        insert_test_session(&conn, "sess-diff");
        insert_test_message(&conn, "msg-diff-1", "sess-diff", "2026-02-20T01:00:00Z");
        insert_test_message(&conn, "msg-diff-2", "sess-diff", "2026-02-20T01:01:00Z");

        // Write initial content
        insert_file_op(
            &conn, "sess-diff", "/src/app.rs", "write",
            Some("line1\nline2\nline3\n"),
            None,
            "tool-diff-001", "msg-diff-1", "2026-02-20T01:00:00Z",
        );

        // Edit: replace line2 with line2-modified
        insert_file_op(
            &conn, "sess-diff", "/src/app.rs", "edit",
            Some("line2-modified"),
            Some("line2"),
            "tool-diff-002", "msg-diff-2", "2026-02-20T01:01:00Z",
        );

        let diff_output = generate_file_diff(&conn, "/src/app.rs", "sess-diff").unwrap();

        assert!(
            diff_output.contains("---"),
            "Unified diff should contain --- header, got: {}",
            diff_output
        );
        assert!(
            diff_output.contains("+++"),
            "Unified diff should contain +++ header, got: {}",
            diff_output
        );
        assert!(
            diff_output.contains("a//src/app.rs"),
            "Diff should reference the file path, got: {}",
            diff_output
        );
    }

    // -------------------------------------------------------------------
    // Test 5: list_git_operations returns inserted git entries
    // -------------------------------------------------------------------
    #[test]
    fn test_list_git_operations_returns_entries() {
        let conn = setup_db();
        insert_test_session(&conn, "sess-git");
        insert_test_message(&conn, "msg-git-1", "sess-git", "2026-02-20T01:00:00Z");

        insert_git_op(
            &conn, "sess-git", "commit",
            "git commit -m \"feat: add feature\"",
            Some("feat: add feature"),
            Some("main"),
            "tool-git-001", "msg-git-1", "2026-02-20T01:00:00Z",
        );
        insert_git_op(
            &conn, "sess-git", "push",
            "git push origin main",
            None,
            Some("main"),
            "tool-git-002", "msg-git-1", "2026-02-20T01:01:00Z",
        );

        let ops = list_git_operations(&conn, Some("sess-git"), None, 100).unwrap();
        assert_eq!(ops.len(), 2, "Should return 2 git operations");

        // Verify commit filter
        let commits = list_git_commits(&conn, Some("sess-git"), 100).unwrap();
        assert_eq!(commits.len(), 1, "Should return 1 commit");
        assert_eq!(
            commits[0].commit_message.as_deref(),
            Some("feat: add feature")
        );
    }

    // -------------------------------------------------------------------
    // Test 6: query_session_timeline returns chronologically ordered mixed entries
    // -------------------------------------------------------------------
    #[test]
    fn test_query_session_timeline_ordered() {
        let conn = setup_db();
        insert_test_session(&conn, "sess-tl");
        insert_test_message(&conn, "msg-tl-1", "sess-tl", "2026-02-20T01:00:00Z");
        insert_test_message(&conn, "msg-tl-2", "sess-tl", "2026-02-20T01:01:00Z");
        insert_test_message(&conn, "msg-tl-3", "sess-tl", "2026-02-20T01:02:00Z");

        // File operation at T+0
        insert_file_op(
            &conn, "sess-tl", "/src/main.rs", "write",
            Some("content"), None,
            "tool-tl-001", "msg-tl-1", "2026-02-20T01:00:00Z",
        );

        // Git operation at T+1
        insert_git_op(
            &conn, "sess-tl", "commit",
            "git commit -m \"msg\"",
            Some("msg"), None,
            "tool-tl-002", "msg-tl-2", "2026-02-20T01:01:00Z",
        );

        // Tool execution at T+2 (via tool_executions table + messages for timestamp)
        insert_tool_execution(
            &conn, "msg-tl-3", "tool-tl-003", "Grep",
            Some("found 5 matches"), None,
        );

        let timeline = query_session_timeline(&conn, "sess-tl", 100).unwrap();
        assert!(
            timeline.len() >= 3,
            "Timeline should have at least 3 entries, got: {}",
            timeline.len()
        );

        // Verify chronological ordering
        for i in 1..timeline.len() {
            assert!(
                timeline[i].timestamp >= timeline[i - 1].timestamp,
                "Timeline entries should be in chronological order: {} >= {}",
                timeline[i].timestamp,
                timeline[i - 1].timestamp
            );
        }

        // Verify entry types are present
        let types: Vec<&str> = timeline.iter().map(|t| t.entry_type.as_str()).collect();
        assert!(
            types.contains(&"file_operation"),
            "Timeline should contain file_operation entries"
        );
        assert!(
            types.contains(&"git_operation"),
            "Timeline should contain git_operation entries"
        );
        assert!(
            types.contains(&"tool_execution"),
            "Timeline should contain tool_execution entries"
        );

        // Verify tool_execution has tool_name populated
        let tool_entry = timeline.iter().find(|t| t.entry_type == "tool_execution").unwrap();
        assert_eq!(tool_entry.tool_name.as_deref(), Some("Grep"));
    }

    // -------------------------------------------------------------------
    // Test 7: query_session_artifacts includes tool_executions with truncation
    // -------------------------------------------------------------------
    #[test]
    fn test_query_session_artifacts_with_truncation() {
        let conn = setup_db();
        insert_test_session(&conn, "sess-sa");
        insert_test_message(&conn, "msg-sa-1", "sess-sa", "2026-02-20T01:00:00Z");
        insert_test_message(&conn, "msg-sa-2", "sess-sa", "2026-02-20T01:01:00Z");

        // Insert a file
        insert_file(&conn, "sess-sa", "/src/main.rs", 1);

        // Insert a git operation
        insert_git_op(
            &conn, "sess-sa", "commit",
            "git commit -m \"test\"",
            Some("test"), None,
            "tool-sa-001", "msg-sa-1", "2026-02-20T01:00:00Z",
        );

        // Insert tool execution with long result_content (>500 chars)
        let long_result = "x".repeat(700);
        insert_tool_execution(
            &conn, "msg-sa-2", "tool-sa-002", "Read",
            Some(&long_result), None,
        );

        let artifacts = query_session_artifacts(&conn, "sess-sa").unwrap();

        assert!(!artifacts.files.is_empty(), "Should have files");
        assert!(!artifacts.git_operations.is_empty(), "Should have git operations");
        assert!(!artifacts.tool_executions.is_empty(), "Should have tool executions");

        // Verify result_summary is truncated
        let te = &artifacts.tool_executions[0];
        let summary = te.result_summary.as_ref().unwrap();
        assert!(
            summary.len() <= 503, // 500 chars + "..."
            "result_summary should be truncated, got length: {}",
            summary.len()
        );
        assert!(
            summary.ends_with("..."),
            "Truncated result_summary should end with '...'"
        );
    }
}
