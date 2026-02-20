//! Streaming JSONL parser with byte-offset tracking and error isolation.
//!
//! This module provides the [`parse_jsonl`] function that reads JSONL files
//! from an arbitrary byte offset and returns parsed records along with the
//! new offset for incremental sync.
//!
//! Key design choices:
//! - Line-by-line parsing (not `serde_json::StreamDeserializer`) to enable
//!   byte-offset tracking needed for incremental sync
//! - Malformed lines produce [`ParseWarning`] entries but do not halt parsing
//!   of subsequent valid lines (per CORE-07)
//! - Byte offsets are tracked using `line.as_bytes().len() + 1` (the +1 accounts
//!   for the newline stripped by `BufReader::lines()`)
//! - Final offset is clamped to file length to handle files without trailing newline

use std::fs::File;
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::Path;

use crate::record::JSONLRecord;

/// Result of parsing a JSONL file (or segment thereof).
#[derive(Debug)]
pub struct ParseResult {
    /// Successfully parsed records, each paired with the byte offset
    /// of the line's start position in the file.
    pub records: Vec<(JSONLRecord, u64)>,
    /// Warnings for lines that failed to parse as valid JSON records.
    pub warnings: Vec<ParseWarning>,
    /// The byte offset to use for the next incremental sync call.
    /// After a complete parse, this points past the last line read.
    pub new_offset: u64,
    /// Number of lines that were parsed (attempted deserialization).
    pub lines_parsed: usize,
    /// Number of empty/whitespace-only lines skipped.
    pub lines_skipped: usize,
    /// Number of lines that failed deserialization.
    pub lines_failed: usize,
}

/// A warning produced when a JSONL line cannot be deserialized.
///
/// These are expected during normal operation — Claude Code may be
/// actively writing to a file, producing a partial final line.
#[derive(Debug)]
pub struct ParseWarning {
    /// Line number (0-based from the start of parsing, i.e. from `from_offset`)
    pub line_number: usize,
    /// Absolute byte offset of the start of the malformed line
    pub byte_offset: u64,
    /// The serde error message describing what went wrong
    pub error: String,
    /// First 500 characters of the raw line (for diagnostics)
    pub raw_line_preview: String,
}

/// Errors that can occur during JSONL parsing (file-level, not line-level).
///
/// Line-level parse failures are captured as [`ParseWarning`], not errors.
/// This error type covers I/O failures that prevent reading the file at all.
#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    #[error("I/O error reading JSONL file: {0}")]
    Io(#[from] std::io::Error),
}

/// Parse a JSONL file starting from a byte offset, returning records and the new offset.
///
/// This is the core ingestion function. It:
/// 1. Opens the file and checks if there is new data past `from_offset`
/// 2. Seeks to `from_offset` and reads line by line
/// 3. Tracks byte offsets for each line
/// 4. Isolates per-line errors as warnings (does not halt on malformed lines)
/// 5. Returns the new offset for the next incremental call
///
/// # Byte-offset safety
///
/// The offset always advances past every line (parsed, failed, or empty).
/// After the loop, `current_offset` is clamped to `min(current_offset, file_length)`
/// to handle the case where the last line has no trailing newline (BufReader::lines()
/// strips newlines, and we add +1 for each line — but the last line may not actually
/// have a newline byte). This means a partial line at EOF will be re-read on the
/// next sync because the offset won't advance past it.
pub fn parse_jsonl(path: &Path, from_offset: u64) -> Result<ParseResult, ParseError> {
    let mut file = File::open(path)?;
    let file_length = file.metadata()?.len();

    // If there is no new data, return an empty result immediately
    if from_offset >= file_length {
        return Ok(ParseResult {
            records: Vec::new(),
            warnings: Vec::new(),
            new_offset: from_offset,
            lines_parsed: 0,
            lines_skipped: 0,
            lines_failed: 0,
        });
    }

    file.seek(SeekFrom::Start(from_offset))?;
    let reader = BufReader::new(file);

    let mut records: Vec<(JSONLRecord, u64)> = Vec::new();
    let mut warnings: Vec<ParseWarning> = Vec::new();
    let mut current_offset = from_offset;
    let mut lines_parsed: usize = 0;
    let mut lines_skipped: usize = 0;
    let mut lines_failed: usize = 0;

    for (line_num, line_result) in reader.lines().enumerate() {
        let line = line_result?;
        // +1 for the newline character that BufReader::lines() strips
        let line_byte_len = line.as_bytes().len() as u64 + 1;
        let line_start_offset = current_offset;

        // Skip empty/whitespace-only lines
        if line.trim().is_empty() {
            current_offset += line_byte_len;
            lines_skipped += 1;
            continue;
        }

        lines_parsed += 1;

        match serde_json::from_str::<JSONLRecord>(&line) {
            Ok(record) => {
                records.push((record, line_start_offset));
            }
            Err(e) => {
                tracing::warn!(
                    file = %path.display(),
                    line = line_num,
                    offset = line_start_offset,
                    error = %e,
                    "Malformed JSONL line"
                );
                let preview = if line.len() > 500 {
                    let mut end = 500;
                    while !line.is_char_boundary(end) {
                        end -= 1;
                    }
                    line[..end].to_string()
                } else {
                    line.clone()
                };
                warnings.push(ParseWarning {
                    line_number: line_num,
                    byte_offset: line_start_offset,
                    error: e.to_string(),
                    raw_line_preview: preview,
                });
                lines_failed += 1;
            }
        }

        current_offset += line_byte_len;
    }

    // Clamp offset to file length to handle missing trailing newline.
    // If the last line had no newline, we added +1 that doesn't correspond
    // to an actual byte — clamping prevents the offset from exceeding file size.
    if current_offset > file_length {
        current_offset = file_length;
    }

    Ok(ParseResult {
        records,
        warnings,
        new_offset: current_offset,
        lines_parsed,
        lines_skipped,
        lines_failed,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// Helper: create a temporary JSONL file with the given content and return its path.
    fn write_temp_jsonl(content: &str) -> tempfile::NamedTempFile {
        let mut file = tempfile::NamedTempFile::new().expect("should create temp file");
        file.write_all(content.as_bytes())
            .expect("should write content");
        file.flush().expect("should flush");
        file
    }

    /// Minimal valid user record JSON for testing the parser.
    fn valid_user_line(uuid: &str) -> String {
        format!(
            r#"{{"type":"user","uuid":"{}","timestamp":"2026-02-20T01:00:00Z","sessionId":"s1","version":"2.1.49","cwd":"/tmp","isSidechain":false,"userType":"external","gitBranch":"main","message":{{"role":"user","content":"hello"}}}}"#,
            uuid
        )
    }

    /// Minimal valid assistant record JSON for testing the parser.
    fn valid_assistant_line(uuid: &str) -> String {
        format!(
            r#"{{"type":"assistant","uuid":"{}","timestamp":"2026-02-20T01:01:00Z","sessionId":"s1","version":"2.1.49","cwd":"/tmp","isSidechain":false,"userType":"external","gitBranch":"main","message":{{"id":"msg1","role":"assistant","model":"claude-opus-4-6","content":[{{"type":"text","text":"hi"}}],"stop_reason":"end_turn","usage":{{"input_tokens":10,"output_tokens":5}}}}}}"#,
            uuid
        )
    }

    /// Test 1: Parse a valid 3-line JSONL file from offset 0 — returns 3 records, new_offset == file length
    #[test]
    fn test_parse_three_valid_lines() {
        let content = format!(
            "{}\n{}\n{}\n",
            valid_user_line("u1"),
            valid_assistant_line("a1"),
            valid_user_line("u2")
        );
        let file = write_temp_jsonl(&content);
        let result = parse_jsonl(file.path(), 0).expect("should parse successfully");

        assert_eq!(result.records.len(), 3, "should have 3 records");
        assert_eq!(result.lines_parsed, 3, "should have parsed 3 lines");
        assert_eq!(result.lines_failed, 0, "no lines should fail");
        assert_eq!(result.warnings.len(), 0, "no warnings expected");
        assert_eq!(
            result.new_offset,
            content.len() as u64,
            "new_offset should equal file length"
        );

        // Verify first record offset is 0
        assert_eq!(result.records[0].1, 0, "first record should be at offset 0");

        // Verify second record offset is after first line
        let first_line_len = valid_user_line("u1").len() as u64 + 1; // +1 for newline
        assert_eq!(
            result.records[1].1, first_line_len,
            "second record should be at correct offset"
        );
    }

    /// Test 2: Parse from a mid-file offset — returns only records after that offset
    #[test]
    fn test_parse_from_mid_offset() {
        let line1 = valid_user_line("u1");
        let line2 = valid_assistant_line("a1");
        let line3 = valid_user_line("u2");
        let content = format!("{}\n{}\n{}\n", line1, line2, line3);
        let file = write_temp_jsonl(&content);

        // Start from offset past the first line
        let offset_after_first = line1.len() as u64 + 1;
        let result =
            parse_jsonl(file.path(), offset_after_first).expect("should parse from mid-offset");

        assert_eq!(
            result.records.len(),
            2,
            "should have 2 records (skipping first line)"
        );
        assert_eq!(result.lines_parsed, 2);
        assert_eq!(result.records[0].1, offset_after_first);
    }

    /// Test 3: Parse a file with a malformed line in the middle — records before and after are parsed, plus one warning
    #[test]
    fn test_parse_with_malformed_line() {
        let line1 = valid_user_line("u1");
        let malformed = r#"{"type": "user", "this is broken json"#;
        let line3 = valid_user_line("u2");
        let content = format!("{}\n{}\n{}\n", line1, malformed, line3);
        let file = write_temp_jsonl(&content);

        let result = parse_jsonl(file.path(), 0).expect("should parse despite malformed line");

        assert_eq!(
            result.records.len(),
            2,
            "should have 2 valid records (malformed line skipped)"
        );
        assert_eq!(result.lines_failed, 1, "one line should have failed");
        assert_eq!(result.warnings.len(), 1, "one warning expected");
        assert_eq!(result.lines_parsed, 3, "3 lines attempted");

        // Verify the warning captures the correct offset
        let expected_malformed_offset = line1.len() as u64 + 1;
        assert_eq!(result.warnings[0].byte_offset, expected_malformed_offset);
        assert!(
            result.warnings[0].raw_line_preview.contains("broken json"),
            "warning should contain the malformed content"
        );
    }

    /// Test 4: Parse from an offset at file end — returns empty ParseResult
    #[test]
    fn test_parse_at_eof() {
        let content = format!("{}\n", valid_user_line("u1"));
        let file = write_temp_jsonl(&content);
        let file_len = content.len() as u64;

        let result = parse_jsonl(file.path(), file_len).expect("should handle EOF offset");

        assert_eq!(result.records.len(), 0, "no records at EOF");
        assert_eq!(result.new_offset, file_len, "offset unchanged at EOF");
        assert_eq!(result.lines_parsed, 0);
    }

    /// Test 5: Parse a file with empty lines interspersed — empty lines skipped, valid records parsed
    #[test]
    fn test_parse_with_empty_lines() {
        let line1 = valid_user_line("u1");
        let line2 = valid_user_line("u2");
        let content = format!("{}\n\n  \n{}\n\n", line1, line2);
        let file = write_temp_jsonl(&content);

        let result =
            parse_jsonl(file.path(), 0).expect("should parse with empty lines interspersed");

        assert_eq!(result.records.len(), 2, "should have 2 valid records");
        assert_eq!(result.lines_skipped, 3, "should skip 3 empty lines");
        assert_eq!(result.lines_failed, 0);
    }

    /// Test 6: Parse a file where last line has no trailing newline — still parses correctly
    #[test]
    fn test_parse_no_trailing_newline() {
        let line1 = valid_user_line("u1");
        let line2 = valid_user_line("u2");
        // No trailing newline on last line
        let content = format!("{}\n{}", line1, line2);
        let file = write_temp_jsonl(&content);

        let result =
            parse_jsonl(file.path(), 0).expect("should parse file without trailing newline");

        assert_eq!(result.records.len(), 2, "should have 2 records");
        // new_offset should be clamped to file length since the last line
        // has no trailing newline
        assert_eq!(
            result.new_offset,
            content.len() as u64,
            "new_offset should be clamped to file length"
        );
    }

    /// Test: Parse beyond file length returns empty result
    #[test]
    fn test_parse_beyond_file_length() {
        let content = format!("{}\n", valid_user_line("u1"));
        let file = write_temp_jsonl(&content);
        let file_len = content.len() as u64;

        let result = parse_jsonl(file.path(), file_len + 100)
            .expect("should handle offset beyond file length");

        assert_eq!(result.records.len(), 0);
        assert_eq!(result.new_offset, file_len + 100);
    }

    /// Test: Byte offsets are accurate across multiple records
    #[test]
    fn test_byte_offset_accuracy() {
        let line1 = valid_user_line("u1");
        let line2 = valid_assistant_line("a1");
        let line3 = valid_user_line("u3");
        let content = format!("{}\n{}\n{}\n", line1, line2, line3);
        let file = write_temp_jsonl(&content);

        let result = parse_jsonl(file.path(), 0).expect("should parse all lines");
        assert_eq!(result.records.len(), 3);

        let expected_offsets: Vec<u64> = vec![
            0,
            line1.len() as u64 + 1,
            line1.len() as u64 + 1 + line2.len() as u64 + 1,
        ];

        for (i, (_, offset)) in result.records.iter().enumerate() {
            assert_eq!(
                *offset, expected_offsets[i],
                "record {} should be at byte offset {}",
                i, expected_offsets[i]
            );
        }
    }

    /// Test: Warning preview is truncated to 500 chars for very long lines
    #[test]
    fn test_warning_preview_truncation() {
        let long_malformed = format!(r#"{{"type":"user","garbage":"{}"}}"#, "x".repeat(600));
        let content = format!("{}\n", long_malformed);
        let file = write_temp_jsonl(&content);

        let result = parse_jsonl(file.path(), 0).expect("should parse");
        assert_eq!(result.warnings.len(), 1);
        assert_eq!(
            result.warnings[0].raw_line_preview.len(),
            500,
            "preview should be truncated to 500 chars"
        );
    }

    /// Test: Records from different record types are all parsed correctly
    #[test]
    fn test_mixed_record_types() {
        let user_line = valid_user_line("u1");
        let assistant_line = valid_assistant_line("a1");
        let queue_line =
            r#"{"type":"queue-operation","operation":"enqueue","timestamp":"2026-02-20T01:00:00Z","sessionId":"s1","content":"hello"}"#;
        let summary_line =
            r#"{"type":"summary","summary":"A conversation about testing.","leafUuid":"leaf-1"}"#;
        let content = format!(
            "{}\n{}\n{}\n{}\n",
            user_line, assistant_line, queue_line, summary_line
        );
        let file = write_temp_jsonl(&content);

        let result = parse_jsonl(file.path(), 0).expect("should parse mixed types");
        assert_eq!(result.records.len(), 4, "should have 4 records");

        // Verify types
        assert!(matches!(result.records[0].0, JSONLRecord::User(_)));
        assert!(matches!(result.records[1].0, JSONLRecord::Assistant(_)));
        assert!(matches!(result.records[2].0, JSONLRecord::QueueOperation(_)));
        assert!(matches!(result.records[3].0, JSONLRecord::Summary(_)));
    }
}
