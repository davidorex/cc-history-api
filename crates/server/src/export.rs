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
