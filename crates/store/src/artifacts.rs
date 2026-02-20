//! Artifact decomposition pipeline.
//!
//! Extracts file operations and git operations from tool_use content blocks
//! during record decomposition. This is the second-pass extraction engine:
//! after the standard `decompose_record` pipeline stores messages, content,
//! and tool_executions, this module parses the tool_use input JSON for
//! Write, Edit, Read, and Bash tools to populate the `files`,
//! `file_operations`, and `git_operations` tables.
//!
//! All INSERT operations use `INSERT OR IGNORE` for idempotency (PAT-012).
//! Regex patterns for git command parsing are compiled once via `OnceLock`.
//!
//! Requirements: ART-05 (Write), ART-06 (Edit), ART-07 (Read),
//!               ART-08 (git ops), ART-09 (bash file ops)

use std::sync::OnceLock;

use claude_history_core::message::ContentBlock;
use claude_history_core::record::{AssistantRecord, JSONLRecord};
use regex::Regex;
use rusqlite::Transaction;

use crate::decompose::DecomposeError;

// ---------------------------------------------------------------------------
// Compiled regex patterns (OnceLock for one-time initialization)
// ---------------------------------------------------------------------------

/// Detects git subcommands within a Bash command string.
/// Matches patterns like: `git add`, `git commit`, `&& git push`, `; git status`
fn git_cmd_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?:^|&&\s*|;\s*)git\s+(\w+)").unwrap())
}

/// Extracts HEREDOC-style commit messages from git commit commands.
/// Matches: `git commit -m "$(cat <<'EOF'\n...\nEOF\n)"`
fn heredoc_msg_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r#"git\s+commit\s+[^"]*-m\s+"\$\(cat\s+<<'?EOF'?\n([\s\S]*?)\n\s*EOF"#)
            .unwrap()
    })
}

/// Extracts inline commit messages from git commit commands.
/// Matches: `git commit -m "message here"`
fn inline_msg_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"git\s+commit\s+[^"]*-m\s+"([^"]+)""#).unwrap())
}

/// Extracts branch names from `git checkout -b` or `git push <remote>` commands.
fn branch_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"git\s+(?:checkout\s+-b|push\s+\w+)\s+(\S+)").unwrap())
}

/// Detects file-touching shell commands: cp, mv, rm, mkdir, touch.
/// Uses a lookahead for the trailing separator so the `&&`/`;` isn't
/// consumed and remains available as a leading separator for the next match.
fn file_cmd_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?:^|[;&]\s*(?:&\s*)?)\s*(cp|mv|rm|mkdir|touch)\s+([^;&]+)").unwrap()
    })
}

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Extract artifacts from a record's tool_use content blocks.
///
/// Called after the standard `decompose_record` pipeline, in the same
/// transaction. For assistant records, parses tool_use blocks for
/// Write/Edit/Read/Bash operations. Returns the number of artifact rows
/// inserted. Returns 0 for record types without tool_use blocks.
pub fn decompose_artifacts(
    record: &JSONLRecord,
    session_id: &str,
    tx: &Transaction,
) -> Result<usize, DecomposeError> {
    match record {
        JSONLRecord::Assistant(r) => decompose_assistant_artifacts(r, session_id, tx),
        // User records do not produce artifact rows in this pass.
        // Tool_result matching (ART-04) is handled separately.
        _ => Ok(0),
    }
}

// ---------------------------------------------------------------------------
// Assistant artifact decomposition
// ---------------------------------------------------------------------------

/// Iterate over assistant message content blocks and dispatch tool_use
/// blocks to the appropriate extraction function based on tool name.
fn decompose_assistant_artifacts(
    r: &AssistantRecord,
    _session_id: &str,
    tx: &Transaction,
) -> Result<usize, DecomposeError> {
    let mut rows = 0;
    for block in &r.message.content {
        if let ContentBlock::ToolUse {
            id, name, input, ..
        } = block
        {
            match name.as_str() {
                "Write" => {
                    rows += extract_write_operation(
                        &r.base.session_id,
                        id,
                        &r.base.uuid,
                        &r.base.timestamp,
                        input,
                        tx,
                    )?;
                }
                "Edit" => {
                    rows += extract_edit_operation(
                        &r.base.session_id,
                        id,
                        &r.base.uuid,
                        &r.base.timestamp,
                        input,
                        tx,
                    )?;
                }
                "Read" => {
                    rows += extract_read_operation(
                        &r.base.session_id,
                        id,
                        &r.base.uuid,
                        &r.base.timestamp,
                        input,
                        tx,
                    )?;
                }
                "Bash" => {
                    rows += extract_bash_operations(
                        &r.base.session_id,
                        id,
                        &r.base.uuid,
                        &r.base.timestamp,
                        input,
                        tx,
                    )?;
                }
                _ => {} // Other tools have no file/git artifacts
            }
        }
    }
    Ok(rows)
}

// ---------------------------------------------------------------------------
// Per-tool extraction functions
// ---------------------------------------------------------------------------

/// Extract a Write tool_use into a file_operations row. [ART-05]
///
/// Input JSON shape: { "file_path": "/abs/path", "content": "full file content" }
fn extract_write_operation(
    session_id: &str,
    tool_use_id: &str,
    message_uuid: &str,
    timestamp: &str,
    input: &serde_json::Value,
    tx: &Transaction,
) -> Result<usize, DecomposeError> {
    let file_path = input.get("file_path").and_then(|v| v.as_str());
    let content = input.get("content").and_then(|v| v.as_str());

    if let (Some(fp), Some(c)) = (file_path, content) {
        upsert_file(session_id, fp, timestamp, tx)?;

        let changed = tx.execute(
            "INSERT OR IGNORE INTO file_operations
             (session_id, file_path, operation_type, content, old_content, command,
              tool_use_id, message_uuid, timestamp)
             VALUES (?1, ?2, 'write', ?3, NULL, NULL, ?4, ?5, ?6)",
            rusqlite::params![session_id, fp, c, tool_use_id, message_uuid, timestamp],
        )?;

        // Update files operation_count and last_modified
        tx.execute(
            "UPDATE files SET operation_count = operation_count + 1,
                 last_modified = MAX(last_modified, ?1)
             WHERE session_id = ?2 AND file_path = ?3",
            rusqlite::params![timestamp, session_id, fp],
        )?;

        Ok(changed)
    } else {
        tracing::debug!(
            tool_use_id = tool_use_id,
            "Write tool_use missing file_path or content; skipping artifact extraction"
        );
        Ok(0)
    }
}

/// Extract an Edit tool_use into a file_operations row. [ART-06]
///
/// Input JSON shape: { "file_path": "/abs/path", "old_string": "...", "new_string": "..." }
/// Stores new_string in content, old_string in old_content.
fn extract_edit_operation(
    session_id: &str,
    tool_use_id: &str,
    message_uuid: &str,
    timestamp: &str,
    input: &serde_json::Value,
    tx: &Transaction,
) -> Result<usize, DecomposeError> {
    let file_path = input.get("file_path").and_then(|v| v.as_str());
    let old_string = input.get("old_string").and_then(|v| v.as_str());
    let new_string = input.get("new_string").and_then(|v| v.as_str());

    if let (Some(fp), Some(old_s), Some(new_s)) = (file_path, old_string, new_string) {
        upsert_file(session_id, fp, timestamp, tx)?;

        let changed = tx.execute(
            "INSERT OR IGNORE INTO file_operations
             (session_id, file_path, operation_type, content, old_content, command,
              tool_use_id, message_uuid, timestamp)
             VALUES (?1, ?2, 'edit', ?3, ?4, NULL, ?5, ?6, ?7)",
            rusqlite::params![session_id, fp, new_s, old_s, tool_use_id, message_uuid, timestamp],
        )?;

        // Update files operation_count and last_modified
        tx.execute(
            "UPDATE files SET operation_count = operation_count + 1,
                 last_modified = MAX(last_modified, ?1)
             WHERE session_id = ?2 AND file_path = ?3",
            rusqlite::params![timestamp, session_id, fp],
        )?;

        Ok(changed)
    } else {
        tracing::debug!(
            tool_use_id = tool_use_id,
            "Edit tool_use missing file_path, old_string, or new_string; skipping artifact extraction"
        );
        Ok(0)
    }
}

/// Extract a Read tool_use into a file_operations row. [ART-07]
///
/// Input JSON shape: { "file_path": "/abs/path" }
/// No content stored (read is non-mutating).
fn extract_read_operation(
    session_id: &str,
    tool_use_id: &str,
    message_uuid: &str,
    timestamp: &str,
    input: &serde_json::Value,
    tx: &Transaction,
) -> Result<usize, DecomposeError> {
    let file_path = input.get("file_path").and_then(|v| v.as_str());

    if let Some(fp) = file_path {
        upsert_file(session_id, fp, timestamp, tx)?;

        let changed = tx.execute(
            "INSERT OR IGNORE INTO file_operations
             (session_id, file_path, operation_type, content, old_content, command,
              tool_use_id, message_uuid, timestamp)
             VALUES (?1, ?2, 'read', NULL, NULL, NULL, ?3, ?4, ?5)",
            rusqlite::params![session_id, fp, tool_use_id, message_uuid, timestamp],
        )?;

        // Update last_modified only — reads do not increment operation_count
        tx.execute(
            "UPDATE files SET last_modified = MAX(last_modified, ?1)
             WHERE session_id = ?2 AND file_path = ?3",
            rusqlite::params![timestamp, session_id, fp],
        )?;

        Ok(changed)
    } else {
        tracing::debug!(
            tool_use_id = tool_use_id,
            "Read tool_use missing file_path; skipping artifact extraction"
        );
        Ok(0)
    }
}

/// Extract git operations and file-touching commands from a Bash tool_use. [ART-08, ART-09]
///
/// Input JSON shape: { "command": "...", "description": "..." }
/// Parses the command string for:
///   1. Git subcommands (commit, push, checkout, etc.) -> git_operations rows
///   2. File-touching commands (cp, mv, rm, mkdir, touch) -> file_operations rows
fn extract_bash_operations(
    session_id: &str,
    tool_use_id: &str,
    message_uuid: &str,
    timestamp: &str,
    input: &serde_json::Value,
    tx: &Transaction,
) -> Result<usize, DecomposeError> {
    let command = match input.get("command").and_then(|v| v.as_str()) {
        Some(cmd) => cmd,
        None => {
            tracing::debug!(
                tool_use_id = tool_use_id,
                "Bash tool_use missing command field; skipping artifact extraction"
            );
            return Ok(0);
        }
    };

    let mut rows = 0;

    // --- Git operation extraction [ART-08] ---
    rows += extract_git_operations(session_id, tool_use_id, message_uuid, timestamp, command, tx)?;

    // --- File-touching command extraction [ART-09] ---
    rows += extract_file_commands(session_id, tool_use_id, message_uuid, timestamp, command, tx)?;

    Ok(rows)
}

/// Parse a Bash command string for git subcommands and insert git_operations rows.
fn extract_git_operations(
    session_id: &str,
    tool_use_id: &str,
    message_uuid: &str,
    timestamp: &str,
    command: &str,
    tx: &Transaction,
) -> Result<usize, DecomposeError> {
    let git_re = git_cmd_regex();
    let mut rows = 0;

    // Find all git subcommands in the command string
    let git_ops: Vec<String> = git_re
        .captures_iter(command)
        .filter_map(|cap| cap.get(1).map(|m| m.as_str().to_string()))
        .collect();

    if git_ops.is_empty() {
        return Ok(0);
    }

    // Extract commit message (try HEREDOC first, then inline)
    let commit_message = heredoc_msg_regex()
        .captures(command)
        .and_then(|cap| cap.get(1).map(|m| m.as_str().trim().to_string()))
        .or_else(|| {
            inline_msg_regex()
                .captures(command)
                .and_then(|cap| cap.get(1).map(|m| m.as_str().to_string()))
        });

    // Extract branch name
    let branch = branch_regex()
        .captures(command)
        .and_then(|cap| cap.get(1).map(|m| m.as_str().to_string()));

    for op_type in &git_ops {
        let msg = if op_type == "commit" {
            commit_message.as_deref()
        } else {
            None
        };

        let changed = tx.execute(
            "INSERT OR IGNORE INTO git_operations
             (session_id, operation_type, command, commit_message, branch,
              tool_use_id, message_uuid, timestamp)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            rusqlite::params![
                session_id,
                op_type,
                command,
                msg,
                branch,
                tool_use_id,
                message_uuid,
                timestamp,
            ],
        )?;
        rows += changed;
    }

    Ok(rows)
}

/// Parse a Bash command string for file-touching shell commands (cp, mv, rm, mkdir, touch)
/// and insert file_operations rows with bash_* operation types.
fn extract_file_commands(
    session_id: &str,
    tool_use_id: &str,
    message_uuid: &str,
    timestamp: &str,
    command: &str,
    tx: &Transaction,
) -> Result<usize, DecomposeError> {
    let file_re = file_cmd_regex();
    let mut rows = 0;

    for cap in file_re.captures_iter(command) {
        let cmd_name = cap.get(1).map(|m| m.as_str()).unwrap_or("");
        let args = cap.get(2).map(|m| m.as_str().trim()).unwrap_or("");

        let op_type = format!("bash_{}", cmd_name);

        // Extract file paths from arguments (best-effort: take the last argument
        // that looks like a path, skipping flags)
        let file_paths = extract_paths_from_args(args);

        if file_paths.is_empty() {
            // Insert a file_operations row with the command but no specific file_path.
            // Use a synthetic path indicating the command was detected but path unclear.
            let changed = tx.execute(
                "INSERT OR IGNORE INTO file_operations
                 (session_id, file_path, operation_type, content, old_content, command,
                  tool_use_id, message_uuid, timestamp)
                 VALUES (?1, ?2, ?3, NULL, NULL, ?4, ?5, ?6, ?7)",
                rusqlite::params![
                    session_id,
                    format!("<{}>", cmd_name),
                    op_type,
                    command,
                    tool_use_id,
                    message_uuid,
                    timestamp,
                ],
            )?;
            rows += changed;
        } else {
            for fp in &file_paths {
                upsert_file(session_id, fp, timestamp, tx)?;

                // Use a composite tool_use_id to allow multiple file_operations
                // from a single Bash tool_use. The UNIQUE(tool_use_id) constraint
                // means we need distinct tool_use_ids per file path.
                let composite_id = format!("{}:bash:{}:{}", tool_use_id, cmd_name, fp);

                let changed = tx.execute(
                    "INSERT OR IGNORE INTO file_operations
                     (session_id, file_path, operation_type, content, old_content, command,
                      tool_use_id, message_uuid, timestamp)
                     VALUES (?1, ?2, ?3, NULL, NULL, ?4, ?5, ?6, ?7)",
                    rusqlite::params![
                        session_id,
                        fp,
                        op_type,
                        command,
                        composite_id,
                        message_uuid,
                        timestamp,
                    ],
                )?;
                rows += changed;

                // Update files operation_count and last_modified
                tx.execute(
                    "UPDATE files SET operation_count = operation_count + 1,
                         last_modified = MAX(last_modified, ?1)
                     WHERE session_id = ?2 AND file_path = ?3",
                    rusqlite::params![timestamp, session_id, fp],
                )?;
            }
        }
    }

    Ok(rows)
}

/// Best-effort extraction of file paths from command arguments.
/// Skips arguments that start with `-` (flags).
/// Returns non-flag arguments that look like file paths.
fn extract_paths_from_args(args: &str) -> Vec<String> {
    args.split_whitespace()
        .filter(|arg| !arg.starts_with('-'))
        .map(|s| s.to_string())
        .collect()
}

// ---------------------------------------------------------------------------
// Helper functions
// ---------------------------------------------------------------------------

/// Upsert a file row for the given session + file_path combination.
///
/// Creates the row on first encounter; subsequent calls for the same
/// (session_id, file_path) are no-ops due to ON CONFLICT DO NOTHING.
fn upsert_file(
    session_id: &str,
    file_path: &str,
    timestamp: &str,
    tx: &Transaction,
) -> Result<(), DecomposeError> {
    tx.execute(
        "INSERT INTO files (session_id, file_path, first_seen, last_modified, operation_count)
         VALUES (?1, ?2, ?3, ?3, 0)
         ON CONFLICT(session_id, file_path) DO NOTHING",
        rusqlite::params![session_id, file_path, timestamp],
    )?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Retroactive artifact decomposition
// ---------------------------------------------------------------------------

/// Process existing tool_executions to retroactively populate artifact tables.
///
/// This handles data ingested before the artifact layer (migration 003) existed.
/// Queries tool_executions joined with messages, processes each in chronological
/// order, and inserts file_operations/git_operations/files rows.
///
/// Idempotent via INSERT OR IGNORE -- re-running on already-decomposed data
/// produces no duplicates. [ART-04 retroactive support]
pub fn decompose_artifacts_retroactive(
    conn: &rusqlite::Connection,
) -> Result<usize, DecomposeError> {
    let mut stmt = conn.prepare(
        "SELECT te.tool_use_id, te.tool_name, te.input_json, te.result_content, te.is_error,
                m.uuid, m.session_id, m.timestamp
         FROM tool_executions te
         JOIN messages m ON m.uuid = te.message_uuid
         WHERE te.tool_name IN ('Write', 'Edit', 'Read', 'Bash')
         ORDER BY m.timestamp ASC",
    )?;

    struct ToolRow {
        tool_use_id: String,
        tool_name: String,
        input_json: Option<String>,
        _result_content: Option<String>,
        _is_error: Option<i32>,
        message_uuid: String,
        session_id: String,
        timestamp: String,
    }

    let rows: Vec<ToolRow> = stmt
        .query_map([], |row| {
            Ok(ToolRow {
                tool_use_id: row.get(0)?,
                tool_name: row.get(1)?,
                input_json: row.get(2)?,
                _result_content: row.get(3)?,
                _is_error: row.get(4)?,
                message_uuid: row.get(5)?,
                session_id: row.get(6)?,
                timestamp: row.get(7)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    if rows.is_empty() {
        return Ok(0);
    }

    let tx = conn.unchecked_transaction()?;
    let mut total_inserted = 0;

    for row in &rows {
        let input: serde_json::Value = match &row.input_json {
            Some(json_str) => match serde_json::from_str(json_str) {
                Ok(v) => v,
                Err(_) => continue, // Skip rows with unparseable input_json
            },
            None => continue,
        };

        let result = match row.tool_name.as_str() {
            "Write" => extract_write_operation(
                &row.session_id,
                &row.tool_use_id,
                &row.message_uuid,
                &row.timestamp,
                &input,
                &tx,
            ),
            "Edit" => extract_edit_operation(
                &row.session_id,
                &row.tool_use_id,
                &row.message_uuid,
                &row.timestamp,
                &input,
                &tx,
            ),
            "Read" => extract_read_operation(
                &row.session_id,
                &row.tool_use_id,
                &row.message_uuid,
                &row.timestamp,
                &input,
                &tx,
            ),
            "Bash" => extract_bash_operations(
                &row.session_id,
                &row.tool_use_id,
                &row.message_uuid,
                &row.timestamp,
                &input,
                &tx,
            ),
            _ => Ok(0),
        };

        match result {
            Ok(n) => total_inserted += n,
            Err(e) => {
                tracing::debug!(
                    tool_use_id = %row.tool_use_id,
                    error = %e,
                    "Retroactive artifact extraction failed for tool_use, skipping"
                );
            }
        }
    }

    tx.commit()?;

    Ok(total_inserted)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema;
    use claude_history_core::message::{AssistantMessage, ContentBlock};
    use claude_history_core::record::{AssistantRecord, RecordBase};
    use rusqlite::Connection;
    use std::collections::HashMap;

    /// Create an in-memory SQLite database with all migrations applied.
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

    /// Helper to create an AssistantRecord with tool_use content blocks.
    fn assistant_with_tools(
        uuid: &str,
        session_id: &str,
        blocks: Vec<ContentBlock>,
    ) -> AssistantRecord {
        AssistantRecord {
            base: test_base(uuid, session_id),
            message: AssistantMessage {
                id: format!("msg_{}", uuid),
                model: "claude-opus-4-6".to_string(),
                role: "assistant".to_string(),
                content: blocks,
                stop_reason: Some("tool_use".to_string()),
                stop_sequence: None,
                usage: None,
                overflow: HashMap::new(),
            },
            request_id: None,
            is_api_error_message: None,
            error: None,
            overflow: HashMap::new(),
        }
    }

    /// Ensure a session row exists so foreign keys are satisfied.
    fn insert_test_session(conn: &Connection, session_id: &str) {
        conn.execute(
            "INSERT OR IGNORE INTO sessions (session_id, project_path, first_seen_at, version, slug, git_branch)
             VALUES (?1, '/test', '2026-02-20T00:00:00Z', '2.1.49', 'test', 'main')",
            rusqlite::params![session_id],
        )
        .unwrap();
    }

    /// Ensure a message row exists so foreign keys are satisfied.
    fn insert_test_message(conn: &Connection, uuid: &str, session_id: &str) {
        conn.execute(
            "INSERT OR IGNORE INTO messages (uuid, session_id, type, timestamp)
             VALUES (?1, ?2, 'assistant', '2026-02-20T01:00:00.000Z')",
            rusqlite::params![uuid, session_id],
        )
        .unwrap();
    }

    // -------------------------------------------------------------------
    // Test 1: Write extraction produces correct file_operations row
    // -------------------------------------------------------------------
    #[test]
    fn test_write_extraction() {
        let conn = setup_db();
        insert_test_session(&conn, "sess-w");
        insert_test_message(&conn, "assist-w", "sess-w");

        let record = assistant_with_tools(
            "assist-w",
            "sess-w",
            vec![ContentBlock::ToolUse {
                id: "tool-w-001".to_string(),
                name: "Write".to_string(),
                input: serde_json::json!({
                    "file_path": "/home/user/project/src/main.rs",
                    "content": "fn main() {\n    println!(\"hello\");\n}\n"
                }),
                caller: None,
            }],
        );

        let tx = conn.unchecked_transaction().unwrap();
        let rows =
            decompose_assistant_artifacts(&record, "sess-w", &tx).unwrap();
        tx.commit().unwrap();

        assert!(rows >= 1, "Should insert at least 1 file_operations row");

        // Verify file_operations row
        let (op_type, content, file_path): (String, String, String) = conn
            .query_row(
                "SELECT operation_type, content, file_path FROM file_operations WHERE tool_use_id = 'tool-w-001'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(op_type, "write");
        assert!(content.contains("fn main()"));
        assert_eq!(file_path, "/home/user/project/src/main.rs");

        // Verify files row was upserted
        let op_count: i64 = conn
            .query_row(
                "SELECT operation_count FROM files WHERE session_id = 'sess-w' AND file_path = '/home/user/project/src/main.rs'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(op_count, 1);
    }

    // -------------------------------------------------------------------
    // Test 2: Edit extraction stores old_content and content correctly
    // -------------------------------------------------------------------
    #[test]
    fn test_edit_extraction() {
        let conn = setup_db();
        insert_test_session(&conn, "sess-e");
        insert_test_message(&conn, "assist-e", "sess-e");

        let record = assistant_with_tools(
            "assist-e",
            "sess-e",
            vec![ContentBlock::ToolUse {
                id: "tool-e-001".to_string(),
                name: "Edit".to_string(),
                input: serde_json::json!({
                    "file_path": "/home/user/project/src/lib.rs",
                    "old_string": "fn old_function()",
                    "new_string": "fn new_function()"
                }),
                caller: None,
            }],
        );

        let tx = conn.unchecked_transaction().unwrap();
        let rows =
            decompose_assistant_artifacts(&record, "sess-e", &tx).unwrap();
        tx.commit().unwrap();

        assert!(rows >= 1);

        // Verify content (new_string) and old_content (old_string)
        let (op_type, content, old_content): (String, String, String) = conn
            .query_row(
                "SELECT operation_type, content, old_content FROM file_operations WHERE tool_use_id = 'tool-e-001'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(op_type, "edit");
        assert_eq!(content, "fn new_function()");
        assert_eq!(old_content, "fn old_function()");
    }

    // -------------------------------------------------------------------
    // Test 3: Read extraction produces row with NULL content
    // -------------------------------------------------------------------
    #[test]
    fn test_read_extraction() {
        let conn = setup_db();
        insert_test_session(&conn, "sess-r");
        insert_test_message(&conn, "assist-r", "sess-r");

        let record = assistant_with_tools(
            "assist-r",
            "sess-r",
            vec![ContentBlock::ToolUse {
                id: "tool-r-001".to_string(),
                name: "Read".to_string(),
                input: serde_json::json!({
                    "file_path": "/home/user/project/README.md"
                }),
                caller: None,
            }],
        );

        let tx = conn.unchecked_transaction().unwrap();
        let rows =
            decompose_assistant_artifacts(&record, "sess-r", &tx).unwrap();
        tx.commit().unwrap();

        assert!(rows >= 1);

        // Verify content and old_content are NULL
        let (op_type, content, old_content): (String, Option<String>, Option<String>) = conn
            .query_row(
                "SELECT operation_type, content, old_content FROM file_operations WHERE tool_use_id = 'tool-r-001'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(op_type, "read");
        assert!(content.is_none(), "Read should have NULL content");
        assert!(old_content.is_none(), "Read should have NULL old_content");

        // Verify files row was upserted with operation_count 0 (reads don't increment)
        let op_count: i64 = conn
            .query_row(
                "SELECT operation_count FROM files WHERE session_id = 'sess-r' AND file_path = '/home/user/project/README.md'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(op_count, 0, "Read should not increment operation_count");
    }

    // -------------------------------------------------------------------
    // Test 4: Bash git commit with HEREDOC message extracts commit_message
    // -------------------------------------------------------------------
    #[test]
    fn test_bash_git_heredoc_commit() {
        let conn = setup_db();
        insert_test_session(&conn, "sess-g");
        insert_test_message(&conn, "assist-g", "sess-g");

        let heredoc_command = "git add src/main.rs && git commit -m \"$(cat <<'EOF'\nfeat(core): add new feature\n\n- Implement the thing\n- Add tests\nEOF\n)\"";

        let record = assistant_with_tools(
            "assist-g",
            "sess-g",
            vec![ContentBlock::ToolUse {
                id: "tool-g-001".to_string(),
                name: "Bash".to_string(),
                input: serde_json::json!({
                    "command": heredoc_command,
                    "description": "Commit changes"
                }),
                caller: None,
            }],
        );

        let tx = conn.unchecked_transaction().unwrap();
        let rows =
            decompose_assistant_artifacts(&record, "sess-g", &tx).unwrap();
        tx.commit().unwrap();

        assert!(rows >= 2, "Should insert at least 2 git_operations rows (add + commit)");

        // Verify commit operation has the extracted message
        let commit_msg: String = conn
            .query_row(
                "SELECT commit_message FROM git_operations
                 WHERE tool_use_id = 'tool-g-001' AND operation_type = 'commit'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(
            commit_msg.contains("feat(core): add new feature"),
            "Should extract HEREDOC commit message, got: {}",
            commit_msg
        );

        // Verify git add row also exists
        let add_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM git_operations
                 WHERE tool_use_id = 'tool-g-001' AND operation_type = 'add'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(add_count, 1, "Should have git add operation");
    }

    // -------------------------------------------------------------------
    // Test 5: Bash chained commands produce multiple git_operations rows
    // -------------------------------------------------------------------
    #[test]
    fn test_bash_chained_git_commands() {
        let conn = setup_db();
        insert_test_session(&conn, "sess-chain");
        insert_test_message(&conn, "assist-chain", "sess-chain");

        let record = assistant_with_tools(
            "assist-chain",
            "sess-chain",
            vec![ContentBlock::ToolUse {
                id: "tool-chain-001".to_string(),
                name: "Bash".to_string(),
                input: serde_json::json!({
                    "command": "git add . && git commit -m \"fix: resolve bug\" && git push origin main"
                }),
                caller: None,
            }],
        );

        let tx = conn.unchecked_transaction().unwrap();
        let rows = decompose_assistant_artifacts(&record, "sess-chain", &tx).unwrap();
        tx.commit().unwrap();

        assert!(rows >= 3, "Should insert at least 3 git_operations rows (add + commit + push)");

        // Verify all three operations exist
        let op_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM git_operations WHERE tool_use_id = 'tool-chain-001'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(op_count >= 3, "Should have at least 3 git operations");

        // Verify commit message was extracted
        let commit_msg: String = conn
            .query_row(
                "SELECT commit_message FROM git_operations
                 WHERE tool_use_id = 'tool-chain-001' AND operation_type = 'commit'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(commit_msg, "fix: resolve bug");

        // Verify branch was extracted from push
        let branch: String = conn
            .query_row(
                "SELECT branch FROM git_operations
                 WHERE tool_use_id = 'tool-chain-001' AND operation_type = 'push'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(branch, "main");
    }

    // -------------------------------------------------------------------
    // Test 6: Bash file commands (rm, cp) produce file_operations with bash_* types
    // -------------------------------------------------------------------
    #[test]
    fn test_bash_file_commands() {
        let conn = setup_db();
        insert_test_session(&conn, "sess-fc");
        insert_test_message(&conn, "assist-fc", "sess-fc");

        let record = assistant_with_tools(
            "assist-fc",
            "sess-fc",
            vec![ContentBlock::ToolUse {
                id: "tool-fc-001".to_string(),
                name: "Bash".to_string(),
                input: serde_json::json!({
                    "command": "rm /tmp/old_file.txt && cp /tmp/source.txt /tmp/dest.txt"
                }),
                caller: None,
            }],
        );

        let tx = conn.unchecked_transaction().unwrap();
        let rows = decompose_assistant_artifacts(&record, "sess-fc", &tx).unwrap();
        tx.commit().unwrap();

        assert!(rows >= 1, "Should insert file_operations rows for bash commands");

        // Check for bash_rm operation
        let rm_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM file_operations
                 WHERE session_id = 'sess-fc' AND operation_type = 'bash_rm'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(rm_count >= 1, "Should have bash_rm operation");

        // Check for bash_cp operation
        let cp_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM file_operations
                 WHERE session_id = 'sess-fc' AND operation_type = 'bash_cp'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(cp_count >= 1, "Should have bash_cp operation");
    }

    // -------------------------------------------------------------------
    // Test 7: decompose_artifacts returns 0 for non-assistant records
    // -------------------------------------------------------------------
    #[test]
    fn test_decompose_artifacts_non_assistant_returns_zero() {
        let conn = setup_db();
        insert_test_session(&conn, "sess-na");

        let user_record = JSONLRecord::User(claude_history_core::record::UserRecord {
            base: test_base("user-na", "sess-na"),
            message: claude_history_core::message::UserMessage {
                role: "user".to_string(),
                content: claude_history_core::message::MessageContent::Text(
                    "hello".to_string(),
                ),
            },
            source_tool_assistant_uuid: None,
            tool_use_result: None,
            thinking_metadata: None,
            todos: None,
            permission_mode: None,
            overflow: HashMap::new(),
        });

        let tx = conn.unchecked_transaction().unwrap();
        let rows = decompose_artifacts(&user_record, "sess-na", &tx).unwrap();
        tx.commit().unwrap();

        assert_eq!(rows, 0, "Non-assistant records should produce 0 artifact rows");
    }

    // -------------------------------------------------------------------
    // Test 8: Idempotency — decomposing same record twice produces no duplicates
    // -------------------------------------------------------------------
    #[test]
    fn test_artifact_idempotency() {
        let conn = setup_db();
        insert_test_session(&conn, "sess-idem");
        insert_test_message(&conn, "assist-idem", "sess-idem");

        let record = assistant_with_tools(
            "assist-idem",
            "sess-idem",
            vec![ContentBlock::ToolUse {
                id: "tool-idem-001".to_string(),
                name: "Write".to_string(),
                input: serde_json::json!({
                    "file_path": "/tmp/idempotent.rs",
                    "content": "// test"
                }),
                caller: None,
            }],
        );

        // First decomposition
        {
            let tx = conn.unchecked_transaction().unwrap();
            decompose_assistant_artifacts(&record, "sess-idem", &tx).unwrap();
            tx.commit().unwrap();
        }

        let count_first: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM file_operations WHERE tool_use_id = 'tool-idem-001'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count_first, 1);

        // Second decomposition — INSERT OR IGNORE should prevent duplicates
        {
            let tx = conn.unchecked_transaction().unwrap();
            let rows = decompose_assistant_artifacts(&record, "sess-idem", &tx).unwrap();
            tx.commit().unwrap();
            assert_eq!(rows, 0, "No new rows on duplicate decomposition");
        }

        let count_second: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM file_operations WHERE tool_use_id = 'tool-idem-001'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count_second, 1, "Row count unchanged after duplicate");
    }

    // -------------------------------------------------------------------
    // Test 9: Bash git inline commit message extraction
    // -------------------------------------------------------------------
    #[test]
    fn test_bash_git_inline_commit() {
        let conn = setup_db();
        insert_test_session(&conn, "sess-inline");
        insert_test_message(&conn, "assist-inline", "sess-inline");

        let record = assistant_with_tools(
            "assist-inline",
            "sess-inline",
            vec![ContentBlock::ToolUse {
                id: "tool-inline-001".to_string(),
                name: "Bash".to_string(),
                input: serde_json::json!({
                    "command": "git commit -m \"chore: update deps\""
                }),
                caller: None,
            }],
        );

        let tx = conn.unchecked_transaction().unwrap();
        decompose_assistant_artifacts(&record, "sess-inline", &tx).unwrap();
        tx.commit().unwrap();

        let commit_msg: String = conn
            .query_row(
                "SELECT commit_message FROM git_operations
                 WHERE tool_use_id = 'tool-inline-001' AND operation_type = 'commit'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(commit_msg, "chore: update deps");
    }

    // -------------------------------------------------------------------
    // Test 10: Multiple tool_use blocks in single assistant record
    // -------------------------------------------------------------------
    #[test]
    fn test_multiple_tool_use_blocks() {
        let conn = setup_db();
        insert_test_session(&conn, "sess-multi");
        insert_test_message(&conn, "assist-multi", "sess-multi");

        let record = assistant_with_tools(
            "assist-multi",
            "sess-multi",
            vec![
                ContentBlock::ToolUse {
                    id: "tool-multi-001".to_string(),
                    name: "Read".to_string(),
                    input: serde_json::json!({"file_path": "/tmp/a.rs"}),
                    caller: None,
                },
                ContentBlock::Text {
                    text: "I see the contents.".to_string(),
                },
                ContentBlock::ToolUse {
                    id: "tool-multi-002".to_string(),
                    name: "Write".to_string(),
                    input: serde_json::json!({
                        "file_path": "/tmp/a.rs",
                        "content": "// updated"
                    }),
                    caller: None,
                },
                ContentBlock::ToolUse {
                    id: "tool-multi-003".to_string(),
                    name: "Edit".to_string(),
                    input: serde_json::json!({
                        "file_path": "/tmp/b.rs",
                        "old_string": "old",
                        "new_string": "new"
                    }),
                    caller: None,
                },
            ],
        );

        let tx = conn.unchecked_transaction().unwrap();
        let rows = decompose_assistant_artifacts(&record, "sess-multi", &tx).unwrap();
        tx.commit().unwrap();

        assert_eq!(rows, 3, "Should insert 3 file_operations (read + write + edit)");

        let total_ops: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM file_operations WHERE session_id = 'sess-multi'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(total_ops, 3);

        // Verify 2 distinct files were tracked
        let file_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM files WHERE session_id = 'sess-multi'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(file_count, 2, "Should track 2 distinct files (a.rs and b.rs)");
    }

    // -------------------------------------------------------------------
    // Test 11: Git regex patterns compile and match correctly
    // -------------------------------------------------------------------
    #[test]
    fn test_git_regex_patterns() {
        // Test git command detection
        let git_re = git_cmd_regex();
        let caps: Vec<String> = git_re
            .captures_iter("git add file.rs && git commit -m \"msg\"")
            .filter_map(|c| c.get(1).map(|m| m.as_str().to_string()))
            .collect();
        assert_eq!(caps, vec!["add", "commit"]);

        // Test HEREDOC extraction
        let heredoc_re = heredoc_msg_regex();
        let heredoc_cmd = "git commit -m \"$(cat <<'EOF'\nmy commit msg\nEOF\n)\"";
        let msg = heredoc_re
            .captures(heredoc_cmd)
            .and_then(|c| c.get(1).map(|m| m.as_str().trim().to_string()));
        assert_eq!(msg, Some("my commit msg".to_string()));

        // Test inline message extraction
        let inline_re = inline_msg_regex();
        let inline_cmd = "git commit -m \"fix: the bug\"";
        let msg = inline_re
            .captures(inline_cmd)
            .and_then(|c| c.get(1).map(|m| m.as_str().to_string()));
        assert_eq!(msg, Some("fix: the bug".to_string()));

        // Test branch extraction
        let branch_re = branch_regex();
        let push_cmd = "git push origin feature-branch";
        let branch = branch_re
            .captures(push_cmd)
            .and_then(|c| c.get(1).map(|m| m.as_str().to_string()));
        assert_eq!(branch, Some("feature-branch".to_string()));

        let checkout_cmd = "git checkout -b new-branch";
        let branch = branch_re
            .captures(checkout_cmd)
            .and_then(|c| c.get(1).map(|m| m.as_str().to_string()));
        assert_eq!(branch, Some("new-branch".to_string()));
    }

    // -------------------------------------------------------------------
    // Test 12: Unknown tools are silently skipped
    // -------------------------------------------------------------------
    #[test]
    fn test_unknown_tool_skipped() {
        let conn = setup_db();
        insert_test_session(&conn, "sess-unk");
        insert_test_message(&conn, "assist-unk", "sess-unk");

        let record = assistant_with_tools(
            "assist-unk",
            "sess-unk",
            vec![ContentBlock::ToolUse {
                id: "tool-unk-001".to_string(),
                name: "Grep".to_string(),
                input: serde_json::json!({"pattern": "foo", "path": "/tmp"}),
                caller: None,
            }],
        );

        let tx = conn.unchecked_transaction().unwrap();
        let rows = decompose_assistant_artifacts(&record, "sess-unk", &tx).unwrap();
        tx.commit().unwrap();

        assert_eq!(rows, 0, "Unknown tools should produce no artifact rows");
    }
}
