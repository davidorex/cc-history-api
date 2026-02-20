//! Output formatting for CLI subcommands.
//!
//! Each function formats data for human-readable terminal output or JSON.
//! Human-readable output goes to stdout with column alignment.
//! JSON output uses serde_json::to_string_pretty to stdout.
//! Diagnostic/logging messages go to stderr (PAT-020).
//!
//! Two output modes:
//! - Human-readable: formatted columns and section headers (default)
//! - JSON: `--json` flag triggers machine-readable output via `print_json`

use claude_history_store::artifact_queries::{FileEntry, FileOperation, GitOperation, SessionArtifacts};
use claude_history_store::fts::SearchResult;
use claude_history_store::query::{ModelStats, SessionSummary, TokenStats, ToolStats};

/// Print search results in human-readable format.
///
/// Each result shows session_id (truncated to 8 chars), timestamp, message_type,
/// block_type, and the FTS5 snippet with >>> <<< markers displayed as-is.
/// Results are separated by `---` dividers.
pub fn print_search_results(results: &[SearchResult]) {
    if results.is_empty() {
        println!("No results found.");
        return;
    }

    for (i, r) in results.iter().enumerate() {
        if i > 0 {
            println!("---");
        }
        let sid_short = if r.session_id.len() > 8 {
            &r.session_id[..8]
        } else {
            &r.session_id
        };
        println!(
            "{} | {} | {} | {}",
            sid_short, r.timestamp, r.message_type, r.block_type
        );
        println!("  {}", r.snippet);
    }
}

/// Print sessions in a column-aligned table.
///
/// Columns: SESSION_ID (8 chars), PROJECT, DATE, MODEL, MESSAGES.
/// Long project paths are truncated to fit the column width.
pub fn print_sessions_table(sessions: &[SessionSummary]) {
    if sessions.is_empty() {
        println!("No sessions found.");
        return;
    }

    println!(
        "{:<8}  {:<40}  {:<20}  {:<20}  {:>8}",
        "SESSION", "PROJECT", "DATE", "MODEL", "MESSAGES"
    );
    println!(
        "{:<8}  {:<40}  {:<20}  {:<20}  {:>8}",
        "--------",
        "----------------------------------------",
        "--------------------",
        "--------------------",
        "--------"
    );

    for s in sessions {
        let sid_short = if s.session_id.len() > 8 {
            &s.session_id[..8]
        } else {
            &s.session_id
        };
        let project = s
            .project_path
            .as_deref()
            .unwrap_or("-");
        let project_display = if project.len() > 40 {
            format!("...{}", &project[project.len() - 37..])
        } else {
            project.to_string()
        };
        let date = s.first_seen_at.as_deref().unwrap_or("-");
        let date_display = if date.len() > 20 {
            &date[..20]
        } else {
            date
        };
        let model = s.model.as_deref().unwrap_or("-");
        let model_display = if model.len() > 20 {
            &model[..20]
        } else {
            model
        };

        println!(
            "{:<8}  {:<40}  {:<20}  {:<20}  {:>8}",
            sid_short, project_display, date_display, model_display, s.message_count
        );
    }
}

/// Print stats in three human-readable sections.
///
/// Sections:
/// 1. "Token Usage by Model" — MODEL, MESSAGES, INPUT_TOKENS, OUTPUT_TOKENS
/// 2. "Tool Frequency" — TOOL, INVOCATIONS, ERRORS
/// 3. "Model Breakdown" — MODEL, MESSAGES, PERCENT
pub fn print_stats(
    token_stats: &[TokenStats],
    tool_stats: &[ToolStats],
    model_stats: &[ModelStats],
) {
    // Section 1: Token Usage by Model
    println!("Token Usage by Model");
    println!(
        "{:<30}  {:>10}  {:>14}  {:>14}",
        "MODEL", "MESSAGES", "INPUT_TOKENS", "OUTPUT_TOKENS"
    );
    println!(
        "{:<30}  {:>10}  {:>14}  {:>14}",
        "------------------------------",
        "----------",
        "--------------",
        "--------------"
    );
    for t in token_stats {
        let model_display = if t.group_key.len() > 30 {
            &t.group_key[..30]
        } else {
            &t.group_key
        };
        println!(
            "{:<30}  {:>10}  {:>14}  {:>14}",
            model_display, t.message_count, t.total_input_tokens, t.total_output_tokens
        );
    }

    println!();

    // Section 2: Tool Frequency
    println!("Tool Frequency");
    println!(
        "{:<30}  {:>12}  {:>8}",
        "TOOL", "INVOCATIONS", "ERRORS"
    );
    println!(
        "{:<30}  {:>12}  {:>8}",
        "------------------------------", "------------", "--------"
    );
    for t in tool_stats {
        let tool_display = if t.tool_name.len() > 30 {
            &t.tool_name[..30]
        } else {
            &t.tool_name
        };
        println!(
            "{:<30}  {:>12}  {:>8}",
            tool_display, t.invocations, t.errors
        );
    }

    println!();

    // Section 3: Model Breakdown
    println!("Model Breakdown");
    println!(
        "{:<30}  {:>10}  {:>8}",
        "MODEL", "MESSAGES", "PERCENT"
    );
    println!(
        "{:<30}  {:>10}  {:>8}",
        "------------------------------", "----------", "--------"
    );
    for m in model_stats {
        let model_display = if m.model.len() > 30 {
            &m.model[..30]
        } else {
            &m.model
        };
        println!(
            "{:<30}  {:>10}  {:>7.1}%",
            model_display, m.message_count, m.percentage
        );
    }
}

/// Print tracked files in a column-aligned table.
///
/// Columns: FILE_PATH (40 chars, truncated with leading ellipsis), SESSION (12 chars),
/// OPS (operation_count), LAST_MODIFIED (24 chars).
pub fn print_files_table(files: &[FileEntry]) {
    if files.is_empty() {
        println!("No files found.");
        return;
    }

    println!(
        "{:<40}  {:<12}  {:>4}  {:<24}",
        "FILE_PATH", "SESSION", "OPS", "LAST_MODIFIED"
    );
    println!(
        "{:<40}  {:<12}  {:>4}  {:<24}",
        "----------------------------------------",
        "------------",
        "----",
        "------------------------"
    );

    for f in files {
        let path_display = if f.file_path.len() > 40 {
            format!("...{}", &f.file_path[f.file_path.len() - 37..])
        } else {
            f.file_path.clone()
        };
        let sid_short = if f.session_id.len() > 12 {
            &f.session_id[..12]
        } else {
            &f.session_id
        };
        let modified_display = if f.last_modified.len() > 24 {
            &f.last_modified[..24]
        } else {
            &f.last_modified
        };

        println!(
            "{:<40}  {:<12}  {:>4}  {:<24}",
            path_display, sid_short, f.operation_count, modified_display
        );
    }
}

/// Print file operations in a column-aligned table.
///
/// Columns: TIMESTAMP (24 chars), TYPE (operation_type, 10 chars),
/// FILE_PATH (40 chars). For write/edit ops, prints a content preview
/// (first 80 chars) on the next line.
pub fn print_file_operations(ops: &[FileOperation]) {
    if ops.is_empty() {
        println!("No file operations found.");
        return;
    }

    println!(
        "{:<24}  {:<10}  {:<40}",
        "TIMESTAMP", "TYPE", "FILE_PATH"
    );
    println!(
        "{:<24}  {:<10}  {:<40}",
        "------------------------",
        "----------",
        "----------------------------------------"
    );

    for op in ops {
        let ts_display = if op.timestamp.len() > 24 {
            &op.timestamp[..24]
        } else {
            &op.timestamp
        };
        let type_display = if op.operation_type.len() > 10 {
            &op.operation_type[..10]
        } else {
            &op.operation_type
        };
        let path_display = if op.file_path.len() > 40 {
            format!("...{}", &op.file_path[op.file_path.len() - 37..])
        } else {
            op.file_path.clone()
        };

        println!(
            "{:<24}  {:<10}  {:<40}",
            ts_display, type_display, path_display
        );

        // Show content preview for write/edit operations
        if matches!(op.operation_type.as_str(), "write" | "edit") {
            if let Some(ref content) = op.content {
                let preview = if content.len() > 80 {
                    format!("{}...", &content[..80])
                } else {
                    content.clone()
                };
                // Replace newlines for single-line display
                let preview = preview.replace('\n', "\\n");
                println!("  {}", preview);
            }
        }
    }
}

/// Print git operations in a column-aligned table.
///
/// Columns: TIMESTAMP (24 chars), TYPE (operation_type, 10 chars),
/// BRANCH (12 chars, or "-"), MESSAGE (commit_message truncated to 60 chars, or "-").
pub fn print_git_operations(ops: &[GitOperation]) {
    if ops.is_empty() {
        println!("No git operations found.");
        return;
    }

    println!(
        "{:<24}  {:<10}  {:<12}  {:<60}",
        "TIMESTAMP", "TYPE", "BRANCH", "MESSAGE"
    );
    println!(
        "{:<24}  {:<10}  {:<12}  {:<60}",
        "------------------------",
        "----------",
        "------------",
        "------------------------------------------------------------"
    );

    for op in ops {
        let ts_display = if op.timestamp.len() > 24 {
            &op.timestamp[..24]
        } else {
            &op.timestamp
        };
        let type_display = if op.operation_type.len() > 10 {
            &op.operation_type[..10]
        } else {
            &op.operation_type
        };
        let branch = op.branch.as_deref().unwrap_or("-");
        let branch_display = if branch.len() > 12 {
            &branch[..12]
        } else {
            branch
        };
        let message = op.commit_message.as_deref().unwrap_or("-");
        let msg_display = if message.len() > 60 {
            format!("{}...", &message[..57])
        } else {
            message.to_string()
        };

        println!(
            "{:<24}  {:<10}  {:<12}  {:<60}",
            ts_display, type_display, branch_display, msg_display
        );
    }
}

/// Print combined session artifacts: files section then git operations section.
///
/// Prints "Files:" header followed by print_files_table, then a blank line,
/// then "Git Operations:" header followed by print_git_operations.
pub fn print_artifacts(artifacts: &SessionArtifacts) {
    println!("Files:");
    print_files_table(&artifacts.files);
    println!();
    println!("Git Operations:");
    print_git_operations(&artifacts.git_operations);
    println!();
    println!(
        "Tool Executions: {} total",
        artifacts.tool_executions.len()
    );
}

/// Print any Serialize value as pretty-printed JSON to stdout.
///
/// On serialization error, prints error to stderr and returns Err.
pub fn print_json<T: serde::Serialize>(data: &T) -> Result<(), String> {
    match serde_json::to_string_pretty(data) {
        Ok(json) => {
            println!("{}", json);
            Ok(())
        }
        Err(e) => {
            eprintln!("Error: Failed to serialize JSON: {}", e);
            Err(e.to_string())
        }
    }
}
