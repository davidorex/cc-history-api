//! Session export logic for JSON, Markdown, and CSV formats.
//!
//! Each export function takes a `&rusqlite::Connection`, a session ID, and a
//! `&mut impl std::io::Write`, writing the formatted output directly to the
//! writer. Messages are loaded in batches (100 at a time) via
//! `query::session_messages_for_export` to avoid unbounded memory use at the
//! query level.
//!
//! For JSON export, all messages are collected into a Vec then serialized with
//! `serde_json::to_writer_pretty`. This holds the full Vec in memory during
//! serialization, which is an acceptable trade-off for Phase 2. A streaming
//! JSON serializer could be a Phase 3+ optimization.
//!
//! For Markdown and CSV, messages are written incrementally as each batch is
//! processed.
//!
//! Requirement ID: CLI-07

use std::io::Write;

use serde::Serialize;
use tokio_rusqlite::rusqlite::Connection;

use claude_history_store::query::{self, ExportContentBlock, ExportMessage};

/// Batch size for loading messages from the database.
const BATCH_SIZE: usize = 100;

/// Session metadata combined with messages for JSON export.
#[derive(Debug, Serialize)]
struct ExportSession {
    session_id: String,
    project_path: Option<String>,
    messages: Vec<ExportMessage>,
}

/// Export a complete session as pretty-printed JSON.
///
/// Loads session metadata from the sessions table, then loads all messages in
/// batches. The resulting JSON structure contains session_id, project_path, and
/// an array of messages with content blocks and token usage.
pub fn export_json(
    conn: &Connection,
    session_id: &str,
    writer: &mut impl Write,
) -> Result<(), Box<dyn std::error::Error>> {
    // Load session metadata
    let (sid, project_path) = load_session_metadata(conn, session_id)?;

    // Load all messages in batches
    let messages = load_all_messages(conn, session_id)?;

    let export = ExportSession {
        session_id: sid,
        project_path,
        messages,
    };

    serde_json::to_writer_pretty(&mut *writer, &export)?;
    writeln!(writer)?;

    Ok(())
}

/// Export a complete session as a readable Markdown transcript.
///
/// Header includes session ID, project path, and date. Each message is rendered
/// with type and timestamp. Text and thinking blocks show their content directly.
/// Tool use blocks show tool name and truncated input. Tool result blocks show
/// truncated content. Messages are streamed to the writer as each batch is
/// processed.
pub fn export_markdown(
    conn: &Connection,
    session_id: &str,
    writer: &mut impl Write,
) -> Result<(), Box<dyn std::error::Error>> {
    let (_sid, project_path) = load_session_metadata(conn, session_id)?;

    // Write header
    writeln!(writer, "# Session: {}", session_id)?;
    if let Some(ref path) = project_path {
        writeln!(writer, "**Project:** {}", path)?;
    }

    // Load and write messages in batches
    let mut offset = 0;
    let mut first_message = true;
    loop {
        let batch = query::session_messages_for_export(conn, session_id, BATCH_SIZE, offset)?;
        if batch.is_empty() {
            break;
        }

        for msg in &batch {
            if first_message {
                // Write date from first message timestamp
                writeln!(writer, "**Date:** {}", msg.timestamp)?;
                writeln!(writer)?;
                first_message = false;
            }

            writeln!(writer, "---")?;
            writeln!(writer, "## {} ({})", capitalize_type(&msg.message_type), msg.timestamp)?;
            writeln!(writer)?;

            for block in &msg.content_blocks {
                write_markdown_block(writer, block)?;
            }
        }

        offset += batch.len();
        if batch.len() < BATCH_SIZE {
            break;
        }
    }

    if first_message {
        writeln!(writer, "\n*No messages found for this session.*")?;
    }

    Ok(())
}

/// Export a complete session as CSV with proper escaping via the csv crate.
///
/// Header row: uuid, session_id, type, timestamp, model, content_preview,
/// input_tokens, output_tokens. Content preview is the first text block's
/// content truncated to 500 chars. Token usage fields come from the message's
/// token_usage if present.
pub fn export_csv(
    conn: &Connection,
    session_id: &str,
    writer: &mut impl Write,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut csv_writer = csv::Writer::from_writer(writer);

    // Write header
    csv_writer.write_record([
        "uuid",
        "session_id",
        "type",
        "timestamp",
        "model",
        "content_preview",
        "input_tokens",
        "output_tokens",
    ])?;

    // Load and write messages in batches
    let mut offset = 0;
    loop {
        let batch = query::session_messages_for_export(conn, session_id, BATCH_SIZE, offset)?;
        if batch.is_empty() {
            break;
        }

        for msg in &batch {
            let content_preview = first_text_content(&msg.content_blocks, 500);
            let input_tokens = msg
                .token_usage
                .as_ref()
                .and_then(|u| u.input_tokens)
                .map(|v| v.to_string())
                .unwrap_or_default();
            let output_tokens = msg
                .token_usage
                .as_ref()
                .and_then(|u| u.output_tokens)
                .map(|v| v.to_string())
                .unwrap_or_default();

            csv_writer.write_record([
                &msg.uuid,
                &msg.session_id,
                &msg.message_type,
                &msg.timestamp,
                msg.model.as_deref().unwrap_or(""),
                &content_preview,
                &input_tokens,
                &output_tokens,
            ])?;
        }

        offset += batch.len();
        if batch.len() < BATCH_SIZE {
            break;
        }
    }

    csv_writer.flush()?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Load session_id and project_path from the sessions table.
///
/// Returns an error if the session does not exist.
fn load_session_metadata(
    conn: &Connection,
    session_id: &str,
) -> Result<(String, Option<String>), Box<dyn std::error::Error>> {
    let result = conn.query_row(
        "SELECT session_id, project_path FROM sessions WHERE session_id = ?1",
        [session_id],
        |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)),
    );

    match result {
        Ok(r) => Ok(r),
        Err(tokio_rusqlite::rusqlite::Error::QueryReturnedNoRows) => {
            Err(format!("Session '{}' not found in database", session_id).into())
        }
        Err(e) => Err(e.into()),
    }
}

/// Load all messages for a session, iterating in batches.
fn load_all_messages(
    conn: &Connection,
    session_id: &str,
) -> Result<Vec<ExportMessage>, Box<dyn std::error::Error>> {
    let mut all_messages = Vec::new();
    let mut offset = 0;

    loop {
        let batch = query::session_messages_for_export(conn, session_id, BATCH_SIZE, offset)?;
        if batch.is_empty() {
            break;
        }
        let batch_len = batch.len();
        all_messages.extend(batch);
        offset += batch_len;
        if batch_len < BATCH_SIZE {
            break;
        }
    }

    Ok(all_messages)
}

/// Capitalize the message type for Markdown display (e.g. "user" -> "User").
fn capitalize_type(t: &str) -> String {
    let mut chars = t.chars();
    match chars.next() {
        None => String::new(),
        Some(c) => c.to_uppercase().to_string() + chars.as_str(),
    }
}

/// Write a single content block as Markdown.
fn write_markdown_block(
    writer: &mut impl Write,
    block: &ExportContentBlock,
) -> Result<(), std::io::Error> {
    match block.block_type.as_str() {
        "text" | "thinking" => {
            if let Some(ref text) = block.text_content {
                writeln!(writer, "{}", text)?;
                writeln!(writer)?;
            }
        }
        "tool_use" => {
            let tool_name = block.tool_name.as_deref().unwrap_or("unknown");
            writeln!(writer, "### Tool Use: {}", tool_name)?;
            if let Some(ref input) = block.tool_input {
                let truncated = truncate_str(input, 200);
                writeln!(writer, "**Input:** `{}`", truncated)?;
            }
            writeln!(writer)?;
        }
        "tool_result" => {
            if let Some(ref text) = block.text_content {
                let truncated = truncate_str(text, 500);
                if text.len() > 500 {
                    writeln!(writer, "{} *(truncated)*", truncated)?;
                } else {
                    writeln!(writer, "{}", truncated)?;
                }
                writeln!(writer)?;
            }
        }
        _ => {
            // Unknown block type — render what we can
            if let Some(ref text) = block.text_content {
                writeln!(writer, "*[{}]* {}", block.block_type, text)?;
                writeln!(writer)?;
            }
        }
    }
    Ok(())
}

/// Truncate a string to at most `max_len` characters.
fn truncate_str(s: &str, max_len: usize) -> &str {
    if s.len() <= max_len {
        s
    } else {
        // Find a valid char boundary at or before max_len
        let mut end = max_len;
        while end > 0 && !s.is_char_boundary(end) {
            end -= 1;
        }
        &s[..end]
    }
}

/// Extract the first text block's content, truncated to max_len.
fn first_text_content(blocks: &[ExportContentBlock], max_len: usize) -> String {
    for block in blocks {
        if block.block_type == "text" || block.block_type == "thinking" {
            if let Some(ref text) = block.text_content {
                return truncate_str(text, max_len).to_string();
            }
        }
    }
    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use claude_history_store::schema::run_migrations;
    use tokio_rusqlite::rusqlite::Connection;

    /// Regression for C2.3 audit rows #28 + #29.
    ///
    /// Migration 011 + decompose_user step 4b project plan_content into
    /// `message_content` as a synthetic row keyed by (message_uuid, -1,
    /// 'plan_content') so the FTS5 external-content table picks it up on
    /// rebuild. Pre-fix, three `query.rs` paths ordered ExportContentBlock
    /// lists by `ORDER BY block_index ASC` without filtering on block_type;
    /// the synthetic row's `block_index = -1` sorted FIRST, ahead of the
    /// real `block_index = 0` text block, and `write_markdown_block`'s `_`
    /// fallthrough arm rendered it as a leading `*[plan_content]* …`
    /// prefix line.
    ///
    /// Aim: drive the markdown export over a plan-bearing user message and
    /// assert (a) the export does not contain the `[plan_content]` marker
    /// the `_` arm would emit, (b) the user's actual text content survives,
    /// (c) the synthetic row is still resident in `message_content` so FTS
    /// indexability is preserved.
    #[test]
    fn export_markdown_filters_synthetic_plan_content_rows() {
        let conn = Connection::open_in_memory().expect("open in-memory db");
        run_migrations(&conn).expect("run migrations through 011");

        // Seed a session, a user message with a real text block, and the
        // C2.3 synthetic plan_content row keyed at block_index = -1.
        conn.execute(
            "INSERT INTO sessions (session_id, project_path, first_seen_at, version)
             VALUES ('s-c2.3-export', '/tmp/c2.3', '2026-05-09T00:00:00Z', '2.1.126')",
            [],
        )
        .expect("seed sessions row");

        conn.execute(
            "INSERT INTO messages (uuid, session_id, type, timestamp, plan_content)
             VALUES ('m-c2.3-export', 's-c2.3-export', 'user', '2026-05-09T00:00:01Z',
                     '# Plan body\n\nstep one: do the thing.')",
            [],
        )
        .expect("seed messages row");

        // Real text block at block_index = 0 — what the user actually typed.
        conn.execute(
            "INSERT INTO message_content (message_uuid, block_index, block_type, text_content)
             VALUES ('m-c2.3-export', 0, 'text', 'visible-user-prose-MARKER')",
            [],
        )
        .expect("seed real text block");

        // Synthetic FTS-only plan_content row at the -1 sentinel.
        conn.execute(
            "INSERT INTO message_content (message_uuid, block_index, block_type, text_content)
             VALUES ('m-c2.3-export', -1, 'plan_content',
                     '# Plan body\n\nstep one: do the thing.')",
            [],
        )
        .expect("seed synthetic plan_content row");

        // Render markdown export.
        let mut buf: Vec<u8> = Vec::new();
        export_markdown(&conn, "s-c2.3-export", &mut buf)
            .expect("export_markdown should succeed");
        let rendered = String::from_utf8(buf).expect("markdown utf-8");

        // Negative assertions — the leak signatures from the `_` fallthrough.
        assert!(
            !rendered.contains("[plan_content]"),
            "rendered markdown must not contain the `[plan_content]` marker \
             the `_` fallthrough arm would emit; got:\n{rendered}"
        );
        assert!(
            !rendered.contains("step one: do the thing."),
            "rendered markdown must not contain the plan body text the \
             synthetic row carried; got:\n{rendered}"
        );

        // Positive assertion — the real text block still surfaces.
        assert!(
            rendered.contains("visible-user-prose-MARKER"),
            "rendered markdown must still contain the user's real text block; \
             got:\n{rendered}"
        );

        // FTS-preservation assertion — the synthetic row remains in
        // message_content so the existing FTS5 rebuild path can index it.
        let synthetic_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM message_content
                 WHERE message_uuid = 'm-c2.3-export'
                   AND block_index = -1
                   AND block_type = 'plan_content'",
                [],
                |row| row.get(0),
            )
            .expect("count synthetic plan_content rows");
        assert_eq!(
            synthetic_count, 1,
            "the synthetic plan_content row must remain in message_content \
             for FTS indexability — the export filter is read-side only"
        );
    }
}
