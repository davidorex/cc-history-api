//! Incremental sync engine for Claude Code JSONL session files.
//!
//! This module bridges the parser (crates/core) and decomposer (crates/store)
//! into a working pipeline that:
//! - Discovers JSONL files recursively via walkdir [SYNC-02]
//! - Tracks byte offsets per file in sync_metadata for incremental sync [SYNC-01]
//! - Processes records in batched transactions of up to BATCH_SIZE [SYNC-03]
//! - Updates sync_metadata atomically within record insertion transactions [SYNC-04]
//! - Extracts session IDs from file paths (handling both main and subagent patterns)
//!
//! The entry points are [`sync_file`] (single file) and [`sync_all`] (directory walk).

use std::path::{Path, PathBuf};

use claude_history_core::parser::{parse_jsonl, ParseWarning};
use walkdir::WalkDir;

use crate::decompose;

/// Number of records per transaction batch.
/// Chosen as a balance between memory usage and transaction overhead.
/// Most files will complete in a single batch; very large files (580MB+)
/// will commit in increments to avoid holding the WAL checkpoint too long.
const BATCH_SIZE: usize = 1000;

/// Result of syncing a single JSONL file.
#[derive(Debug)]
pub struct SyncFileResult {
    /// Canonical file path that was synced.
    pub file_path: String,
    /// Number of records successfully decomposed into the database.
    pub records_synced: usize,
    /// Number of records that failed decomposition (logged but not fatal).
    pub records_failed: usize,
    /// Parser warnings (malformed lines, partial lines, etc.).
    pub warnings: Vec<ParseWarning>,
    /// Number of overflow fields logged to schema_drift_log.
    pub overflow_fields_logged: usize,
    /// True if the file had no new data since the last sync.
    pub skipped: bool,
}

/// Aggregate result of syncing all JSONL files in a directory tree.
#[derive(Debug, Default)]
pub struct SyncAllResult {
    /// Number of .jsonl files discovered by directory walking.
    pub files_discovered: usize,
    /// Number of files that had new data and were synced.
    pub files_synced: usize,
    /// Number of files that had no new data (byte offset unchanged).
    pub files_skipped: usize,
    /// Number of files that encountered errors during sync.
    pub files_errored: usize,
    /// Total records decomposed across all files.
    pub total_records: usize,
    /// Total parser warnings across all files.
    pub total_warnings: usize,
    /// Total overflow fields logged across all files.
    pub total_overflow_fields: usize,
}

/// Errors that can occur during sync operations.
#[derive(Debug, thiserror::Error)]
pub enum SyncError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Parse error: {0}")]
    Parse(#[from] claude_history_core::parser::ParseError),

    #[error("SQLite error: {0}")]
    Sqlite(#[from] rusqlite::Error),

    #[error("Decompose error: {0}")]
    Decompose(#[from] decompose::DecomposeError),

    #[error("tokio-rusqlite error: {0}")]
    TokioRusqlite(#[from] tokio_rusqlite::Error),
}

/// Extract a session ID from a JSONL file path.
///
/// Handles two path patterns observed in ~/.claude/projects/:
///
/// 1. **Main session files:** `{projects_dir}/{project-path}/{session-uuid}.jsonl`
///    -> session_id = filename stem (the UUID)
///
/// 2. **Subagent files:** `{projects_dir}/{project-path}/{session-uuid}/subagents/agent-{id}.jsonl`
///    -> session_id = the parent session UUID (two directories above "subagents")
///
/// Returns None if the path cannot be parsed into either pattern.
pub fn extract_session_id(path: &Path) -> Option<String> {
    // Check for subagent pattern: .../subagents/agent-xxx.jsonl
    // The session UUID is the directory two levels up from "subagents"
    let components: Vec<_> = path.components().collect();
    for (i, component) in components.iter().enumerate() {
        if let std::path::Component::Normal(name) = component {
            if name.to_str() == Some("subagents") && i >= 1 {
                // The session UUID is the directory just before "subagents"
                if let std::path::Component::Normal(session_dir) = &components[i - 1] {
                    return session_dir.to_str().map(|s| s.to_string());
                }
            }
        }
    }

    // Main session file: filename stem is the session UUID
    path.file_stem()
        .and_then(|s| s.to_str())
        .map(|s| s.to_string())
}

/// Sync a single JSONL file into the database.
///
/// This function:
/// 1. Queries sync_metadata for the last byte offset for this file
/// 2. Checks if the file has grown since the last sync (skips if not)
/// 3. Parses new data from the last offset via parse_jsonl
/// 4. Processes parsed records in batches of BATCH_SIZE
/// 5. For each batch, opens a transaction, decomposes all records, and
///    updates sync_metadata with the batch boundary offset
/// 6. Returns a SyncFileResult summarizing what happened
///
/// The sync_metadata update is atomic with the record insertions — if a
/// batch transaction is interrupted, sync_metadata still reflects the
/// last successfully committed batch boundary.
pub async fn sync_file(
    conn: &tokio_rusqlite::Connection,
    path: &Path,
    session_id: &str,
) -> Result<SyncFileResult, SyncError> {
    let path_buf = path.to_path_buf();
    let path_str = path_buf.to_string_lossy().to_string();
    let session_id = session_id.to_string();

    // Get file size to check for new data before entering conn.call()
    let file_size = std::fs::metadata(&path_buf)?.len();

    let result = conn
        .call(move |conn| {
            // 1. Query sync_metadata for last byte offset
            let last_offset: u64 = conn
                .query_row(
                    "SELECT last_byte_offset FROM sync_metadata WHERE file_path = ?1",
                    [&path_str],
                    |row| row.get::<_, i64>(0),
                )
                .map(|v| v as u64)
                .unwrap_or(0);

            // 2. Skip if no new data
            if file_size <= last_offset {
                return Ok(SyncFileResult {
                    file_path: path_str,
                    records_synced: 0,
                    records_failed: 0,
                    warnings: Vec::new(),
                    overflow_fields_logged: 0,
                    skipped: true,
                });
            }

            // 3. Parse new data from the last offset
            let parsed = parse_jsonl(&path_buf, last_offset)
                .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;

            let warnings = parsed.warnings;
            let records_failed = parsed.lines_failed;

            // If no records and no significant data, still update the offset
            // so we don't re-read empty trailing content
            if parsed.records.is_empty() {
                let tx = conn.unchecked_transaction()?;
                tx.execute(
                    "INSERT INTO sync_metadata (file_path, last_byte_offset, record_count, last_synced_at)
                     VALUES (?1, ?2, 0, datetime('now'))
                     ON CONFLICT(file_path) DO UPDATE SET
                       last_byte_offset = ?2,
                       last_synced_at = datetime('now')",
                    rusqlite::params![&path_str, parsed.new_offset as i64],
                )?;
                tx.commit()?;

                return Ok(SyncFileResult {
                    file_path: path_str,
                    records_synced: 0,
                    records_failed,
                    warnings,
                    overflow_fields_logged: 0,
                    skipped: false,
                });
            }

            // 4. Process records in batches
            let mut total_synced: usize = 0;
            let mut total_overflow: usize = 0;
            let mut total_decompose_failed: usize = 0;

            let record_count = parsed.records.len();
            let chunks: Vec<_> = parsed.records.chunks(BATCH_SIZE).collect();
            let num_chunks = chunks.len();

            for (chunk_idx, chunk) in chunks.into_iter().enumerate() {
                let tx = conn.unchecked_transaction()?;

                for (record, _line_offset) in chunk {
                    match decompose::decompose_record(record, &session_id, &tx) {
                        Ok(dr) => {
                            total_synced += 1;
                            total_overflow += dr.overflow_fields;
                        }
                        Err(e) => {
                            tracing::warn!(
                                file = %path_str,
                                error = %e,
                                "Failed to decompose record"
                            );
                            total_decompose_failed += 1;
                        }
                    }
                }

                // Compute the batch boundary offset:
                // - For intermediate batches, use the line_end offset of the last
                //   record in the chunk (the start offset of what comes after it).
                //   We approximate this by looking at the next chunk's first record offset.
                // - For the final batch, use parsed.new_offset.
                let batch_offset = if chunk_idx == num_chunks - 1 {
                    // Last batch: commit the parser's computed new_offset
                    parsed.new_offset
                } else {
                    // Intermediate batch: the next record's start offset is the
                    // boundary. We already have chunk boundaries from the slice.
                    // Since chunks are slices into parsed.records, the next chunk
                    // starts at the next record. We need the byte position.
                    // However, the parsed records include their line start offsets.
                    // The "end" of this chunk's last record is approximately the
                    // start of the next chunk's first record.
                    //
                    // We access the next record's offset from the original records vec.
                    // chunk_idx * BATCH_SIZE + chunk.len() gives us the index of the
                    // next record in the original vec.
                    let next_record_idx = (chunk_idx + 1) * BATCH_SIZE;
                    if next_record_idx < record_count {
                        parsed.records[next_record_idx].1
                    } else {
                        parsed.new_offset
                    }
                };

                let batch_records = chunk.len() as i64;

                // Update sync_metadata atomically within this transaction
                tx.execute(
                    "INSERT INTO sync_metadata (file_path, last_byte_offset, record_count, last_synced_at)
                     VALUES (?1, ?2, ?3, datetime('now'))
                     ON CONFLICT(file_path) DO UPDATE SET
                       last_byte_offset = ?2,
                       record_count = record_count + ?3,
                       last_synced_at = datetime('now')",
                    rusqlite::params![&path_str, batch_offset as i64, batch_records],
                )?;

                tx.commit()?;

                tracing::debug!(
                    file = %path_str,
                    batch = chunk_idx + 1,
                    batch_records = chunk.len(),
                    batch_offset = batch_offset,
                    "Committed batch"
                );
            }

            tracing::info!(
                file = %path_str,
                records = total_synced,
                warnings = warnings.len(),
                overflow_fields = total_overflow,
                "Synced file"
            );

            Ok(SyncFileResult {
                file_path: path_str,
                records_synced: total_synced,
                records_failed: records_failed + total_decompose_failed,
                warnings,
                overflow_fields_logged: total_overflow,
                skipped: false,
            })
        })
        .await
        .map_err(|e| match e {
            tokio_rusqlite::Error::Error(re) => SyncError::Sqlite(re),
            other => SyncError::TokioRusqlite(other),
        })?;

    Ok(result)
}

/// Sync all JSONL files discovered under a directory tree.
///
/// Recursively walks the given directory, finds all .jsonl files,
/// extracts session IDs from their paths, and calls sync_file for each.
///
/// Files that error are logged and counted but do not halt the bulk import.
/// This is critical for robustness — a single corrupted file should not
/// prevent syncing the other 6,000+ files.
pub async fn sync_all(
    conn: &tokio_rusqlite::Connection,
    projects_dir: &Path,
) -> Result<SyncAllResult, SyncError> {
    let mut result = SyncAllResult::default();

    // Discover all .jsonl files
    let jsonl_files: Vec<PathBuf> = WalkDir::new(projects_dir)
        .follow_links(false)
        .into_iter()
        .filter_map(|entry| entry.ok())
        .filter(|entry| {
            entry.file_type().is_file()
                && entry
                    .path()
                    .extension()
                    .map(|ext| ext == "jsonl")
                    .unwrap_or(false)
        })
        .map(|entry| entry.into_path())
        .collect();

    result.files_discovered = jsonl_files.len();

    tracing::info!(
        projects_dir = %projects_dir.display(),
        files_discovered = result.files_discovered,
        "Starting sync"
    );

    for file_path in &jsonl_files {
        let session_id = match extract_session_id(file_path) {
            Some(id) => id,
            None => {
                tracing::warn!(
                    file = %file_path.display(),
                    "Could not extract session ID from file path, skipping"
                );
                result.files_errored += 1;
                continue;
            }
        };

        match sync_file(conn, file_path, &session_id).await {
            Ok(file_result) => {
                if file_result.skipped {
                    result.files_skipped += 1;
                } else {
                    result.files_synced += 1;
                    result.total_records += file_result.records_synced;
                    result.total_warnings += file_result.warnings.len();
                    result.total_overflow_fields += file_result.overflow_fields_logged;
                }
            }
            Err(e) => {
                tracing::warn!(
                    file = %file_path.display(),
                    error = %e,
                    "Error syncing file, continuing to next"
                );
                result.files_errored += 1;
            }
        }
    }

    tracing::info!(
        files_synced = result.files_synced,
        files_discovered = result.files_discovered,
        files_skipped = result.files_skipped,
        files_errored = result.files_errored,
        total_records = result.total_records,
        "Sync complete"
    );

    // Backfill version_history from sessions table at end of sync_all.
    //
    // Mirrors the watcher's startup-time backfill at crates/server/src/watcher.rs:512-526
    // so that `claude-history sync` (CLI, no daemon) keeps version_history current
    // even when the daemon has been down for a stretch and one or more new Claude
    // Code versions appeared in JSONL during that window.
    //
    // INSERT OR IGNORE keeps the operation idempotent: versions already in
    // version_history are left alone, and only newly-observed versions get rows.
    // Because of OR IGNORE this also does not re-derive session_count for
    // already-present versions — that semantics question is the subject of D3
    // (unify session_count semantics) and is intentionally out of scope here.
    //
    // Failures are logged at warn level and do not propagate; sync_all has
    // already done its primary work (sessions/messages/etc. are committed),
    // and version_history will be re-attempted on the next sync or daemon start.
    let backfill_result = conn.call(|c| {
        c.execute_batch(
            "INSERT OR IGNORE INTO version_history (version, first_seen_at, last_seen_at, session_id, session_count)
             SELECT
                 version,
                 MIN(first_seen_at),
                 MAX(COALESCE(last_seen_at, first_seen_at)),
                 (SELECT s2.session_id FROM sessions s2 WHERE s2.version = sessions.version
                  ORDER BY s2.first_seen_at ASC LIMIT 1),
                 COUNT(*)
             FROM sessions
             WHERE version IS NOT NULL AND version != ''
             GROUP BY version"
        )
    }).await;

    match backfill_result {
        Ok(()) => tracing::info!("version_history backfill (sync_all) processed"),
        Err(e) => tracing::warn!(error = %e, "version_history backfill (sync_all) failed — will be retried on next sync or daemon start"),
    }

    // Rebuild FTS5 index if any files were synced (new data ingested).
    // Skip rebuild when nothing changed to avoid unnecessary work on
    // no-op syncs. The rebuild re-indexes all message_content rows in
    // a single pass, keeping the external-content FTS5 table consistent.
    if result.files_synced > 0 {
        conn.call(|conn| {
            crate::fts::rebuild_fts_index(conn)
        })
        .await
        .map_err(|e| match e {
            tokio_rusqlite::Error::Error(re) => SyncError::Sqlite(re),
            other => SyncError::TokioRusqlite(other),
        })?;
    }

    // Retroactive artifact decomposition: backfill files/file_operations/git_operations
    // from existing tool_executions. Idempotent via INSERT OR IGNORE, safe to run on
    // every sync. Only runs when new records were ingested (tool_executions rows to process).
    // Failures are logged but do not prevent sync_all from returning successfully.
    if result.total_records > 0 {
        match conn.call(|conn| {
            crate::artifacts::decompose_artifacts_retroactive(conn)
                .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))
        }).await {
            Ok(rows) => tracing::info!(artifact_rows = rows, "Retroactive artifact decomposition complete"),
            Err(e) => tracing::warn!(error = %e, "Retroactive artifact decomposition failed (non-fatal)"),
        }
    }

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use std::io::Write;

    /// Helper: create a temporary JSONL file in a directory, returning the file path.
    fn write_jsonl_file(dir: &Path, filename: &str, content: &str) -> PathBuf {
        let file_path = dir.join(filename);
        if let Some(parent) = file_path.parent() {
            std::fs::create_dir_all(parent).expect("should create parent dirs");
        }
        let mut file = std::fs::File::create(&file_path).expect("should create file");
        file.write_all(content.as_bytes())
            .expect("should write content");
        file.flush().expect("should flush");
        file_path
    }

    /// Minimal valid user record JSON for testing.
    fn valid_user_line(uuid: &str, session_id: &str) -> String {
        format!(
            r#"{{"type":"user","uuid":"{}","timestamp":"2026-02-20T01:00:00Z","sessionId":"{}","version":"2.1.49","cwd":"/tmp","isSidechain":false,"userType":"external","gitBranch":"main","message":{{"role":"user","content":"hello"}}}}"#,
            uuid, session_id
        )
    }

    /// Variant of `valid_user_line` that lets the test pin an explicit `version`
    /// string. Used by the version_history backfill regression test to construct
    /// sessions tagged with several distinct Claude Code versions.
    fn valid_user_line_with_version(uuid: &str, session_id: &str, version: &str) -> String {
        format!(
            r#"{{"type":"user","uuid":"{}","timestamp":"2026-02-20T01:00:00Z","sessionId":"{}","version":"{}","cwd":"/tmp","isSidechain":false,"userType":"external","gitBranch":"main","message":{{"role":"user","content":"hello"}}}}"#,
            uuid, session_id, version
        )
    }

    /// Minimal valid assistant record JSON for testing.
    fn valid_assistant_line(uuid: &str, session_id: &str) -> String {
        format!(
            r#"{{"type":"assistant","uuid":"{}","timestamp":"2026-02-20T01:01:00Z","sessionId":"{}","version":"2.1.49","cwd":"/tmp","isSidechain":false,"userType":"external","gitBranch":"main","message":{{"id":"msg1","role":"assistant","model":"claude-opus-4-6","content":[{{"type":"text","text":"hi"}}],"stop_reason":"end_turn","usage":{{"input_tokens":10,"output_tokens":5}}}}}}"#,
            uuid, session_id
        )
    }

    /// Minimal valid summary record JSON.
    fn valid_summary_line() -> String {
        r#"{"type":"summary","summary":"A conversation about testing.","leafUuid":"leaf-1"}"#
            .to_string()
    }

    // -----------------------------------------------------------------------
    // Test: extract_session_id from main session file
    // -----------------------------------------------------------------------
    #[test]
    fn test_extract_session_id_main_file() {
        let path = Path::new("/Users/david/.claude/projects/some-project/eb3ca04b-0383-4955-8606-7c5c9cabe3d7.jsonl");
        let id = extract_session_id(path);
        assert_eq!(id, Some("eb3ca04b-0383-4955-8606-7c5c9cabe3d7".to_string()));
    }

    // -----------------------------------------------------------------------
    // Test: extract_session_id from subagent file
    // -----------------------------------------------------------------------
    #[test]
    fn test_extract_session_id_subagent_file() {
        let path = Path::new("/Users/david/.claude/projects/some-project/eb3ca04b-0383-4955-8606-7c5c9cabe3d7/subagents/agent-abc123.jsonl");
        let id = extract_session_id(path);
        assert_eq!(id, Some("eb3ca04b-0383-4955-8606-7c5c9cabe3d7".to_string()));
    }

    // -----------------------------------------------------------------------
    // Test 1: sync_all with 2 valid JSONL files -> records exist, sync_metadata has 2 rows
    // -----------------------------------------------------------------------
    #[tokio::test]
    async fn test_sync_all_two_files() {
        let tmp_dir = tempfile::tempdir().expect("should create temp dir");
        let projects_dir = tmp_dir.path().join("projects");
        std::fs::create_dir_all(&projects_dir).unwrap();

        let db_path = tmp_dir.path().join("test.db");
        let conn = db::init_db(&db_path).await.expect("should init db");

        // Create two JSONL files
        let session1 = "sess-aaa-111";
        let session2 = "sess-bbb-222";
        let content1 = format!(
            "{}\n{}\n",
            valid_user_line("u1", session1),
            valid_assistant_line("a1", session1),
        );
        let content2 = format!(
            "{}\n{}\n{}\n",
            valid_user_line("u2", session2),
            valid_assistant_line("a2", session2),
            valid_summary_line(),
        );

        write_jsonl_file(&projects_dir, &format!("{}.jsonl", session1), &content1);
        write_jsonl_file(&projects_dir, &format!("{}.jsonl", session2), &content2);

        let result = sync_all(&conn, &projects_dir).await.expect("sync should succeed");

        assert_eq!(result.files_discovered, 2);
        assert_eq!(result.files_synced, 2);
        assert_eq!(result.files_skipped, 0);
        assert_eq!(result.files_errored, 0);
        // 2 records from file1 + 3 records from file2
        assert_eq!(result.total_records, 5);

        // Verify messages table has the expected records
        let msg_count: i64 = conn
            .call(|conn| {
                conn.query_row("SELECT COUNT(*) FROM messages", [], |row| row.get(0))
            })
            .await
            .expect("should count messages");
        // user + assistant records go to messages table (2 from each file = 4)
        // summary goes to summaries table, not messages
        assert_eq!(msg_count, 4, "Should have 4 messages (2 user + 2 assistant)");

        // Verify sync_metadata has 2 rows
        let meta_count: i64 = conn
            .call(|conn| {
                conn.query_row("SELECT COUNT(*) FROM sync_metadata", [], |row| row.get(0))
            })
            .await
            .expect("should count sync_metadata");
        assert_eq!(meta_count, 2);
    }

    // -----------------------------------------------------------------------
    // Test 2: sync_all again on same files -> files_skipped=2, total_records=0
    // -----------------------------------------------------------------------
    #[tokio::test]
    async fn test_sync_all_incremental_skip() {
        let tmp_dir = tempfile::tempdir().expect("should create temp dir");
        let projects_dir = tmp_dir.path().join("projects");
        std::fs::create_dir_all(&projects_dir).unwrap();

        let db_path = tmp_dir.path().join("test.db");
        let conn = db::init_db(&db_path).await.expect("should init db");

        let session1 = "sess-inc-001";
        let session2 = "sess-inc-002";
        let content1 = format!("{}\n", valid_user_line("u1", session1));
        let content2 = format!("{}\n", valid_user_line("u2", session2));

        write_jsonl_file(&projects_dir, &format!("{}.jsonl", session1), &content1);
        write_jsonl_file(&projects_dir, &format!("{}.jsonl", session2), &content2);

        // First sync
        let result1 = sync_all(&conn, &projects_dir).await.expect("first sync should succeed");
        assert_eq!(result1.files_synced, 2);
        assert_eq!(result1.total_records, 2);

        // Second sync — should skip both files
        let result2 = sync_all(&conn, &projects_dir).await.expect("second sync should succeed");
        assert_eq!(result2.files_skipped, 2, "Both files should be skipped on second sync");
        assert_eq!(result2.files_synced, 0);
        assert_eq!(result2.total_records, 0, "No new records on second sync");
    }

    // -----------------------------------------------------------------------
    // Test 3: Append new records -> only appended records are processed
    // -----------------------------------------------------------------------
    #[tokio::test]
    async fn test_sync_incremental_append() {
        let tmp_dir = tempfile::tempdir().expect("should create temp dir");
        let projects_dir = tmp_dir.path().join("projects");
        std::fs::create_dir_all(&projects_dir).unwrap();

        let db_path = tmp_dir.path().join("test.db");
        let conn = db::init_db(&db_path).await.expect("should init db");

        let session = "sess-append-001";
        let initial_content = format!("{}\n", valid_user_line("u1", session));
        let file_path = write_jsonl_file(
            &projects_dir,
            &format!("{}.jsonl", session),
            &initial_content,
        );

        // First sync
        let result1 = sync_all(&conn, &projects_dir).await.expect("first sync");
        assert_eq!(result1.total_records, 1);

        // Append a new record to the file
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&file_path)
            .expect("should open for append");
        let append_content = format!("{}\n", valid_user_line("u2", session));
        file.write_all(append_content.as_bytes())
            .expect("should append");
        file.flush().expect("should flush");

        // Second sync — should process only the appended record
        let result2 = sync_all(&conn, &projects_dir).await.expect("second sync");
        assert_eq!(result2.files_synced, 1, "One file has new data");
        assert_eq!(result2.total_records, 1, "Only the appended record should be processed");

        // Verify total messages in DB
        let msg_count: i64 = conn
            .call(|conn| {
                conn.query_row("SELECT COUNT(*) FROM messages", [], |row| row.get(0))
            })
            .await
            .expect("should count");
        assert_eq!(msg_count, 2, "Should have 2 messages total");
    }

    // -----------------------------------------------------------------------
    // Test 4: Malformed line in middle -> valid records still decomposed
    // -----------------------------------------------------------------------
    #[tokio::test]
    async fn test_sync_file_with_malformed_line() {
        let tmp_dir = tempfile::tempdir().expect("should create temp dir");
        let db_path = tmp_dir.path().join("test.db");
        let conn = db::init_db(&db_path).await.expect("should init db");

        let session = "sess-malformed";
        let content = format!(
            "{}\n{}\n{}\n",
            valid_user_line("u1", session),
            r#"{"type": "user", "this is broken json"#,
            valid_user_line("u2", session),
        );
        let file_path = write_jsonl_file(
            tmp_dir.path(),
            &format!("{}.jsonl", session),
            &content,
        );

        let result = sync_file(&conn, &file_path, session)
            .await
            .expect("sync should succeed despite malformed line");

        assert_eq!(result.records_synced, 2, "Should sync 2 valid records");
        assert!(!result.warnings.is_empty(), "Should have warnings for malformed line");
        assert!(!result.skipped, "File had new data");
    }

    // -----------------------------------------------------------------------
    // Test 5: Unknown field in record -> schema_drift_log has entry
    // -----------------------------------------------------------------------
    #[tokio::test]
    async fn test_sync_file_with_unknown_field() {
        let tmp_dir = tempfile::tempdir().expect("should create temp dir");
        let db_path = tmp_dir.path().join("test.db");
        let conn = db::init_db(&db_path).await.expect("should init db");

        let session = "sess-drift";
        // User record with an unknown field "brandNewField"
        let content = format!(
            r#"{{"type":"user","uuid":"u-drift","timestamp":"2026-02-20T01:00:00Z","sessionId":"{}","version":"2.1.49","cwd":"/tmp","isSidechain":false,"userType":"external","gitBranch":"main","message":{{"role":"user","content":"hello"}},"brandNewField":"surprise"}}"#,
            session
        );
        let content = format!("{}\n", content);
        let file_path = write_jsonl_file(
            tmp_dir.path(),
            &format!("{}.jsonl", session),
            &content,
        );

        let result = sync_file(&conn, &file_path, session)
            .await
            .expect("sync should succeed");

        assert_eq!(result.records_synced, 1);
        assert!(result.overflow_fields_logged > 0, "Should log overflow field");

        // Verify schema_drift_log has the entry
        let drift_count: i64 = conn
            .call(|conn| {
                conn.query_row(
                    "SELECT COUNT(*) FROM schema_drift_log WHERE field_name = 'brandNewField'",
                    [],
                    |row| row.get(0),
                )
            })
            .await
            .expect("should count drift entries");
        assert_eq!(drift_count, 1, "Should have exactly one drift entry for brandNewField");
    }

    // -----------------------------------------------------------------------
    // Test: Subagent file path session ID extraction in sync context
    // -----------------------------------------------------------------------
    #[tokio::test]
    async fn test_sync_subagent_file_path() {
        let tmp_dir = tempfile::tempdir().expect("should create temp dir");
        let projects_dir = tmp_dir.path().join("projects");
        let session_uuid = "eb3ca04b-0383-4955-8606-7c5c9cabe3d7";

        // Create subagent directory structure
        let subagent_dir = projects_dir
            .join("some-project")
            .join(session_uuid)
            .join("subagents");
        std::fs::create_dir_all(&subagent_dir).unwrap();

        let db_path = tmp_dir.path().join("test.db");
        let conn = db::init_db(&db_path).await.expect("should init db");

        let content = format!("{}\n", valid_user_line("u-sub", session_uuid));
        write_jsonl_file(
            &subagent_dir,
            "agent-abc123.jsonl",
            &content,
        );

        let result = sync_all(&conn, &projects_dir).await.expect("sync should succeed");

        assert_eq!(result.files_discovered, 1);
        assert_eq!(result.files_synced, 1);
        assert_eq!(result.total_records, 1);

        // Verify the session_id in the database matches the parent directory UUID
        let session_id: String = conn
            .call(|conn| {
                conn.query_row(
                    "SELECT session_id FROM sessions LIMIT 1",
                    [],
                    |row| row.get(0),
                )
            })
            .await
            .expect("should get session");
        assert_eq!(session_id, session_uuid);
    }

    // -----------------------------------------------------------------------
    // Test (D1): sync_all backfills version_history from the sessions table.
    //
    // Aim: lock in the new behavior added by D1 — `claude-history sync` (CLI,
    // no daemon) keeps version_history current. Pre-D1 this test would fail
    // with version_history row count = 0, because nothing in the sync_all
    // path touched version_history.
    //
    // Setup: three JSONL files, each with two user records. Each file uses
    // a distinct `version` string. Decomposing user records creates rows in
    // the sessions table with the version from the JSONL record.
    //
    // Expected post-condition:
    //   - version_history has exactly 3 rows (one per distinct version)
    //   - per-version session_count matches COUNT(*) FROM sessions WHERE version=?
    //
    // Note on session_count: the backfill SQL uses INSERT OR IGNORE. On a
    // fresh DB with no prior version_history rows, every version gets its
    // first row written here, so session_count is the COUNT(*) result. The
    // case where version_history already has a row for a version (and OR
    // IGNORE skips the update) is the subject of D3 — explicitly out of
    // scope for this test.
    // -----------------------------------------------------------------------
    #[tokio::test]
    async fn test_sync_all_backfills_version_history() {
        let tmp_dir = tempfile::tempdir().expect("should create temp dir");
        let projects_dir = tmp_dir.path().join("projects");
        std::fs::create_dir_all(&projects_dir).unwrap();

        let db_path = tmp_dir.path().join("test.db");
        let conn = db::init_db(&db_path).await.expect("should init db");

        // Three sessions, each at a distinct version, two records each.
        let fixtures: &[(&str, &str)] = &[
            ("sess-vh-001", "2.1.100"),
            ("sess-vh-002", "2.1.115"),
            ("sess-vh-003", "2.1.130"),
        ];

        for (i, (session, version)) in fixtures.iter().enumerate() {
            let content = format!(
                "{}\n{}\n",
                valid_user_line_with_version(&format!("u{}-a", i), session, version),
                valid_user_line_with_version(&format!("u{}-b", i), session, version),
            );
            write_jsonl_file(&projects_dir, &format!("{}.jsonl", session), &content);
        }

        let result = sync_all(&conn, &projects_dir)
            .await
            .expect("sync_all should succeed");
        assert_eq!(result.files_synced, 3);

        // version_history must contain one row per distinct version.
        let vh_count: i64 = conn
            .call(|conn| {
                conn.query_row("SELECT COUNT(*) FROM version_history", [], |row| row.get(0))
            })
            .await
            .expect("should count version_history");
        assert_eq!(
            vh_count, 3,
            "version_history must have exactly 3 rows after sync_all (one per distinct version in fixtures)"
        );

        // Per-version session_count in version_history must match the count
        // of sessions rows at that version. With one session per version in
        // this fixture, that count is 1 each.
        for (_session, version) in fixtures.iter() {
            let version_owned = (*version).to_string();
            let v_for_vh = version_owned.clone();
            let vh_session_count: i64 = conn
                .call(move |conn| {
                    conn.query_row(
                        "SELECT session_count FROM version_history WHERE version = ?1",
                        [&v_for_vh],
                        |row| row.get(0),
                    )
                })
                .await
                .expect("should read version_history.session_count");

            let v_for_sess = version_owned.clone();
            let sessions_at_version: i64 = conn
                .call(move |conn| {
                    conn.query_row(
                        "SELECT COUNT(*) FROM sessions WHERE version = ?1",
                        [&v_for_sess],
                        |row| row.get(0),
                    )
                })
                .await
                .expect("should count sessions at version");

            assert_eq!(
                vh_session_count, sessions_at_version,
                "version_history.session_count for version {} must equal COUNT(*) FROM sessions WHERE version = ?",
                version_owned
            );
            assert_eq!(
                sessions_at_version, 1,
                "fixture invariant: exactly one session per version"
            );
        }
    }
}
