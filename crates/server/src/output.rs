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

use chrono::{DateTime, Local, Utc};
use claude_history_store::artifact_queries::{FileEntry, FileOperation, GitOperation, SessionArtifacts};
use claude_history_store::fts::{SearchResult, SearchResultSource};
use claude_history_store::query::{
    AttachmentRow, HookExecutionRow, ModelStats, PlanFullRow, PlanRow, RecordTypeDriftEntry,
    SessionSummary, TokenStats, ToolStats, VersionDriftGroup, VersionHistoryEntry,
};

/// Convert a UTC timestamp string to local time for display.
///
/// Attempts to parse ISO 8601 / RFC 3339 timestamps (the format stored in the
/// database, originating from Claude Code's JSONL). On parse failure, returns
/// the original string unchanged — avoids panicking on unexpected formats.
///
/// Output format: "2026-02-22 20:18:19" (local time, no timezone suffix,
/// compact enough for column-aligned tables).
fn to_local(utc_str: &str) -> String {
    utc_str
        .parse::<DateTime<Utc>>()
        .map(|dt| dt.with_timezone(&Local).format("%Y-%m-%d %H:%M:%S").to_string())
        .unwrap_or_else(|_| utc_str.to_string())
}

/// Print search results in human-readable format.
///
/// Each result shows session_id (truncated to 8 chars), timestamp, message_type,
/// block_type, and the FTS5 snippet with >>> <<< markers displayed as-is.
/// Results are separated by `---` dividers.
///
/// Source label (C1.3): when a result's `source` is
/// `SearchResultSource::Attachment`, the line is prefixed with `[attachment]`
/// to distinguish it visually from the pre-C1.3 message-content rows. Message
/// rows render unchanged for backwards-compatible terminal output.
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
        let source_tag = match &r.source {
            SearchResultSource::Message => String::new(),
            SearchResultSource::Attachment(_) => "[attachment] ".to_string(),
        };
        println!(
            "{}{} | {} | {} | {}",
            source_tag,
            sid_short,
            to_local(&r.timestamp),
            r.message_type,
            r.block_type
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
        let date_raw = s.first_seen_at.as_deref().unwrap_or("-");
        let date_local = to_local(date_raw);
        let date_display = if date_local.len() > 20 {
            &date_local[..20]
        } else {
            &date_local
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
        let modified_local = to_local(&f.last_modified);
        let modified_display = if modified_local.len() > 24 {
            &modified_local[..24]
        } else {
            &modified_local
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
        let ts_local = to_local(&op.timestamp);
        let ts_display = if ts_local.len() > 24 {
            &ts_local[..24]
        } else {
            &ts_local
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
        let ts_local = to_local(&op.timestamp);
        let ts_display = if ts_local.len() > 24 {
            &ts_local[..24]
        } else {
            &ts_local
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

/// Print version history in a 5-column table.
///
/// Columns: VERSION (30 chars), FIRST_SEEN (24 chars), LAST_SEEN (24 chars),
/// SESSIONS (right-aligned 8 chars), NEW_FIELDS (right-aligned 10 chars).
pub fn print_version_history(entries: &[VersionHistoryEntry]) {
    if entries.is_empty() {
        println!("No version data found.");
        return;
    }

    println!(
        "{:<30}  {:<24}  {:<24}  {:>8}  {:>10}",
        "VERSION", "FIRST_SEEN", "LAST_SEEN", "SESSIONS", "NEW_FIELDS"
    );
    println!(
        "{:<30}  {:<24}  {:<24}  {:>8}  {:>10}",
        "------------------------------",
        "------------------------",
        "------------------------",
        "--------",
        "----------"
    );

    for entry in entries {
        let version_display = if entry.version.len() > 30 {
            &entry.version[..30]
        } else {
            &entry.version
        };
        let first_local = to_local(&entry.first_seen_at);
        let first_display = if first_local.len() > 24 {
            &first_local[..24]
        } else {
            &first_local
        };
        let last_local = to_local(&entry.last_seen_at);
        let last_display = if last_local.len() > 24 {
            &last_local[..24]
        } else {
            &last_local
        };
        println!(
            "{:<30}  {:<24}  {:<24}  {:>8}  {:>10}",
            version_display,
            first_display,
            last_display,
            entry.session_count,
            entry.new_fields_count
        );
    }
}

/// Print drift entries in a grouped format by version and record type.
///
/// Output format:
/// ```text
/// Version: 1.0.16
///   Record Type: user
///     FIELD                      OCCURRENCES  STATUS      SAMPLE
///     -------------------------  -----------  ----------  --------------------------------------------------
///     isCompactSummary                    42  promoted    true
/// ```
pub fn print_drift_grouped(groups: &[VersionDriftGroup]) {
    if groups.is_empty() {
        println!("No schema drift detected.");
        return;
    }

    for (vi, group) in groups.iter().enumerate() {
        if vi > 0 {
            println!();
        }
        println!("Version: {}", group.version);

        for rt_group in &group.record_types {
            println!("  Record Type: {}", rt_group.record_type);
            println!(
                "    {:<25}  {:>11}  {:<10}  {:<50}",
                "FIELD", "OCCURRENCES", "STATUS", "SAMPLE"
            );
            println!(
                "    {:<25}  {:>11}  {:<10}  {:<50}",
                "-------------------------",
                "-----------",
                "----------",
                "--------------------------------------------------"
            );

            for field in &rt_group.fields {
                let field_display = if field.field_name.len() > 25 {
                    &field.field_name[..25]
                } else {
                    &field.field_name
                };
                let sample = field.sample_value.as_deref().unwrap_or("-");
                let sample_display = if sample.len() > 50 {
                    &sample[..50]
                } else {
                    sample
                };
                println!(
                    "    {:<25}  {:>11}  {:<10}  {:<50}",
                    field_display,
                    field.occurrence_count,
                    field.promotion_status,
                    sample_display
                );
            }
        }
    }
}

/// Print record-type drift entries in a column-aligned table.
///
/// Columns: TYPE_NAME (24 chars), VERSION (10 chars), OCCURRENCES (right-aligned),
/// LAST_SEEN (local time), SAMPLE (truncated to 60 chars). Mirrors the column
/// shape used by [`print_drift_grouped`] but operates on the flat list shape
/// of `record_type_drift_log` (no version/record-type grouping).
pub fn print_record_type_drift(entries: &[RecordTypeDriftEntry]) {
    if entries.is_empty() {
        println!("No record-type drift detected.");
        return;
    }

    println!(
        "{:<24}  {:<10}  {:>11}  {:<19}  {:<60}",
        "TYPE_NAME", "VERSION", "OCCURRENCES", "LAST_SEEN", "SAMPLE"
    );
    println!(
        "{:<24}  {:<10}  {:>11}  {:<19}  {:<60}",
        "------------------------",
        "----------",
        "-----------",
        "-------------------",
        "------------------------------------------------------------"
    );

    for entry in entries {
        let type_display = if entry.type_name.len() > 24 {
            &entry.type_name[..24]
        } else {
            &entry.type_name
        };
        let version_display = entry.version.as_deref().unwrap_or("-");
        let version_truncated = if version_display.len() > 10 {
            &version_display[..10]
        } else {
            version_display
        };
        let last_seen = to_local(&entry.last_seen_at);
        let sample_default = "-".to_string();
        let sample = entry.sample_value.as_ref().unwrap_or(&sample_default);
        let sample_display = if sample.len() > 60 {
            &sample[..60]
        } else {
            sample.as_str()
        };
        println!(
            "{:<24}  {:<10}  {:>11}  {:<19}  {:<60}",
            type_display, version_truncated, entry.occurrence_count, last_seen, sample_display
        );
    }
}

/// Print attachment rows in a column-aligned table (C1.4).
///
/// Columns: UUID (8 chars), TIMESTAMP (local time), INNER_TYPE (24 chars),
/// SESSION (8 chars), VERSION (10 chars). The body_json is intentionally
/// omitted from the table; callers wanting the full body should use
/// `--json` or the `attachments show <uuid>` subcommand.
pub fn print_attachments_table(rows: &[AttachmentRow]) {
    if rows.is_empty() {
        println!("No attachments found.");
        return;
    }

    println!(
        "{:<8}  {:<19}  {:<24}  {:<8}  {:<10}",
        "UUID", "TIMESTAMP", "INNER_TYPE", "SESSION", "VERSION"
    );
    println!(
        "{:<8}  {:<19}  {:<24}  {:<8}  {:<10}",
        "--------",
        "-------------------",
        "------------------------",
        "--------",
        "----------"
    );

    for r in rows {
        let uuid_short = if r.uuid.len() > 8 { &r.uuid[..8] } else { &r.uuid };
        let session_short = if r.session_id.len() > 8 {
            &r.session_id[..8]
        } else {
            &r.session_id
        };
        let inner_type_display = if r.inner_type.len() > 24 {
            &r.inner_type[..24]
        } else {
            &r.inner_type
        };
        let version = r.version.as_deref().unwrap_or("-");
        let version_truncated = if version.len() > 10 { &version[..10] } else { version };

        println!(
            "{:<8}  {:<19}  {:<24}  {:<8}  {:<10}",
            uuid_short,
            to_local(&r.timestamp),
            inner_type_display,
            session_short,
            version_truncated
        );
    }
}

/// Print a single attachment row in a section-formatted block (C1.4).
///
/// Renders the envelope fields as labelled lines and pretty-prints
/// `body_json` if present. Used by `claude-history attachments show <uuid>`
/// for human-readable terminal output.
pub fn print_attachment_show(row: &AttachmentRow) {
    println!("UUID:        {}", row.uuid);
    println!("SESSION:     {}", row.session_id);
    println!(
        "PARENT:      {}",
        row.parent_uuid.as_deref().unwrap_or("-")
    );
    println!("TIMESTAMP:   {}", to_local(&row.timestamp));
    println!("CWD:         {}", row.cwd.as_deref().unwrap_or("-"));
    println!(
        "VERSION:     {}",
        row.version.as_deref().unwrap_or("-")
    );
    println!(
        "GIT_BRANCH:  {}",
        row.git_branch.as_deref().unwrap_or("-")
    );
    println!("SLUG:        {}", row.slug.as_deref().unwrap_or("-"));
    println!(
        "ENTRYPOINT:  {}",
        row.entrypoint.as_deref().unwrap_or("-")
    );
    println!("INNER_TYPE:  {}", row.inner_type);
    println!("---");
    match row.body_json.as_deref() {
        Some(b) => match serde_json::from_str::<serde_json::Value>(b) {
            Ok(v) => match serde_json::to_string_pretty(&v) {
                Ok(s) => println!("{}", s),
                Err(_) => println!("{}", b),
            },
            Err(_) => println!("{}", b),
        },
        None => println!("(no body_json)"),
    }
}

/// Print hook execution rows in a column-aligned table (C1.4).
///
/// Columns: ID (right-aligned), HOOK_EVENT (18 chars), TOOL_USE_ID (24 chars),
/// EXIT_CODE (right-aligned), DURATION_MS (right-aligned), DECISION (12 chars).
/// stdout/stderr/command are intentionally omitted from the table for
/// readability; callers wanting full payloads should use `--json`.
pub fn print_hook_executions(rows: &[HookExecutionRow]) {
    if rows.is_empty() {
        println!("No hook executions found.");
        return;
    }

    println!(
        "{:>6}  {:<18}  {:<24}  {:>9}  {:>11}  {:<12}",
        "ID", "HOOK_EVENT", "TOOL_USE_ID", "EXIT_CODE", "DURATION_MS", "DECISION"
    );
    println!(
        "{:>6}  {:<18}  {:<24}  {:>9}  {:>11}  {:<12}",
        "------",
        "------------------",
        "------------------------",
        "---------",
        "-----------",
        "------------"
    );

    for r in rows {
        let event = r.hook_event.as_deref().unwrap_or("-");
        let event_truncated = if event.len() > 18 { &event[..18] } else { event };
        let tu = r.tool_use_id.as_deref().unwrap_or("-");
        let tu_truncated = if tu.len() > 24 { &tu[..24] } else { tu };
        let exit_display = r
            .exit_code
            .map(|c| c.to_string())
            .unwrap_or_else(|| "-".to_string());
        let duration_display = r
            .duration_ms
            .map(|d| d.to_string())
            .unwrap_or_else(|| "-".to_string());
        let decision = r.decision.as_deref().unwrap_or("-");
        let decision_truncated = if decision.len() > 12 {
            &decision[..12]
        } else {
            decision
        };

        println!(
            "{:>6}  {:<18}  {:<24}  {:>9}  {:>11}  {:<12}",
            r.id, event_truncated, tu_truncated, exit_display, duration_display, decision_truncated
        );
    }
}

/// Print plan rows in a column-aligned table (C2.4).
///
/// Columns: SESSION (8 chars truncated), TIMESTAMP (local), PROJECT
/// (40 chars rtrim, multibyte-safe left-truncation via char_indices),
/// LEN (right-aligned char count from SQLite `length(plan_content)`),
/// PREVIEW (first ~50 chars of the markdown, with newlines collapsed to
/// spaces so the table stays single-line). Full markdown body is omitted
/// from the table; callers who need it should use `--json` or
/// `plans show <session-id>`.
pub fn print_plans_list(rows: &[PlanRow]) {
    if rows.is_empty() {
        println!("No plans found.");
        return;
    }

    println!(
        "{:<8}  {:<19}  {:<40}  {:>7}  {}",
        "SESSION", "TIMESTAMP", "PROJECT", "LEN", "PREVIEW"
    );
    println!(
        "{:<8}  {:<19}  {:<40}  {:>7}  {}",
        "--------",
        "-------------------",
        "----------------------------------------",
        "-------",
        "----------------------------------------------------"
    );

    for r in rows {
        let sid_short = if r.session_id.len() > 8 {
            &r.session_id[..8]
        } else {
            &r.session_id
        };
        let project = r.project_path.as_deref().unwrap_or("-");
        // Multibyte-safe left-truncation: project paths are arbitrary
        // strings from sessions.project_path and are not guaranteed to
        // be ASCII. Byte-indexing into a multibyte UTF-8 char would
        // panic at runtime. char_indices() lets us pick a code-point
        // boundary by counting from the end and slicing on the byte
        // index of that boundary.
        let project_display = {
            let char_count = project.chars().count();
            if char_count > 40 {
                // Skip leading chars so the suffix has 37 chars; prefix
                // with "..." for a 40-wide cell. nth(N) returns the
                // byte index of the (N+1)th char; we want to start the
                // suffix at the (char_count - 37)th char.
                let start_char = char_count - 37;
                let start_byte = project
                    .char_indices()
                    .nth(start_char)
                    .map(|(i, _)| i)
                    .unwrap_or(0);
                format!("...{}", &project[start_byte..])
            } else {
                project.to_string()
            }
        };
        // Collapse newlines / tabs to single spaces so table rows stay on
        // one line. char_indices()-based truncation keeps this safe for
        // the multibyte UTF-8 the markdown may contain.
        let preview_oneline: String = r
            .plan_content_preview
            .chars()
            .map(|c| if c == '\n' || c == '\r' || c == '\t' { ' ' } else { c })
            .collect();
        let preview_short = match preview_oneline.char_indices().nth(50) {
            Some((idx, _)) => format!("{}...", &preview_oneline[..idx]),
            None => preview_oneline.clone(),
        };

        println!(
            "{:<8}  {:<19}  {:<40}  {:>7}  {}",
            sid_short,
            to_local(&r.timestamp),
            project_display,
            r.plan_content_length,
            preview_short
        );
    }
}

/// Print full plan markdown bodies for a session (C2.4).
///
/// Each row gets a header block (session, project, message uuid,
/// timestamp, length) followed by a `---` separator and the verbatim
/// markdown body. Multiple plan-bearing messages render in chronological
/// order matching `plan_show`'s SQL `ORDER BY timestamp ASC`.
pub fn print_plan_show(rows: &[PlanFullRow]) {
    for (i, r) in rows.iter().enumerate() {
        if i > 0 {
            println!();
            println!("===");
            println!();
        }
        println!("SESSION:    {}", r.session_id);
        println!(
            "PROJECT:    {}",
            r.project_path.as_deref().unwrap_or("-")
        );
        println!("MESSAGE:    {}", r.message_uuid);
        println!("TIMESTAMP:  {}", to_local(&r.timestamp));
        // Char count to align with `plans_list`'s `plan_content_length`,
        // which comes from SQLite `length(plan_content)` (char count for
        // TEXT). The unit label is "chars" to make the semantic explicit
        // and to avoid the prior "bytes" label that disagreed with the
        // list-view value for non-ASCII plans.
        println!("LENGTH:     {} chars", r.plan_content.chars().count());
        println!("---");
        println!("{}", r.plan_content);
    }
}

/// Print canned queries in a column-aligned table.
///
/// Columns: NAME (20 chars), DESCRIPTION (40 chars), PARAMS (remaining).
/// Parameters are shown as `name[=default]` separated by commas.
pub fn print_queries_list(queries: &[&claude_history_store::query_registry::CannedQuery]) {
    if queries.is_empty() {
        // Caller handles empty-directory messaging to stderr; this
        // handles the table-body-empty case if called directly.
        return;
    }

    println!(
        "{:<20}  {:<40}  {}",
        "NAME", "DESCRIPTION", "PARAMS"
    );
    println!(
        "{:<20}  {:<40}  {}",
        "--------------------",
        "----------------------------------------",
        "------------------------------"
    );

    for q in queries {
        let name_display = if q.name.len() > 20 {
            &q.name[..20]
        } else {
            &q.name
        };
        let desc_display = if q.description.len() > 40 {
            format!("{}...", &q.description[..37])
        } else {
            q.description.clone()
        };
        let params_str: Vec<String> = q
            .params
            .iter()
            .map(|p| {
                if let Some(ref d) = p.default {
                    format!("{}={}", p.name, d)
                } else {
                    p.name.clone()
                }
            })
            .collect();
        let params_display = params_str.join(", ");

        println!(
            "{:<20}  {:<40}  {}",
            name_display, desc_display, params_display
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
