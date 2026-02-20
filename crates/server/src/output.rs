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
