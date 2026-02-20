//! FTS5 full-text search over message content and file operations.
//!
//! Provides rebuild and search functions for two FTS5 virtual tables:
//!   - `fts_message_content` (migration 002): full-text search over message content blocks
//!   - `fts_file_operations` (migration 003): full-text search over file operation
//!     content, old_content, and command columns
//!
//! Both tables use external-content mode, so their indexes must be rebuilt after
//! sync operations to reflect new data.
//!
//! Search queries are sanitized by wrapping user input in double quotes,
//! treating the query as a phrase match and preventing FTS5 syntax injection.
//! For advanced FTS5 query syntax, callers can extend this module with a
//! raw-query variant.
//!
//! Requirement IDs: FTS-01, FTS-02, FTS-03

use rusqlite::Connection;
use serde::{Deserialize, Serialize};

/// A single search result from FTS5 full-text search over file operations.
///
/// Includes session context, file path, operation type, and the FTS5 snippet
/// from whichever indexed column matched (content, old_content, or command).
/// The rank score comes from FTS5 BM25 ranking.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileOperationSearchResult {
    /// Primary key of the file_operations row.
    pub id: i64,
    /// Session ID the file operation belongs to.
    pub session_id: String,
    /// Path of the file that was operated on.
    pub file_path: String,
    /// Operation type (write, edit, read, bash_cp, bash_mv, etc.).
    pub operation_type: String,
    /// FTS5 snippet from the matching column with <b></b> delimiters.
    pub snippet: String,
    /// ISO8601 timestamp of the file operation.
    pub timestamp: String,
    /// BM25 relevance score. Lower (more negative) values indicate better matches.
    pub rank: f64,
}

/// A single search result from FTS5 full-text search over message content.
///
/// Includes session context (session_id, message_type, timestamp) joined from
/// the messages table, plus the FTS5 snippet and BM25 rank score.
#[derive(Debug, Serialize, Deserialize)]
pub struct SearchResult {
    /// UUID of the message containing the matched content block.
    pub message_uuid: String,
    /// Session ID the message belongs to.
    pub session_id: String,
    /// Message type discriminator (user, assistant).
    pub message_type: String,
    /// ISO8601 timestamp of the message.
    pub timestamp: String,
    /// Content block type (text, thinking, tool_result, tool_use).
    pub block_type: String,
    /// Context snippet around the match with >>> <<< delimiters.
    pub snippet: String,
    /// BM25 relevance score. Lower (more negative) values indicate better matches.
    pub rank: f64,
}

/// Rebuild the FTS5 index from the current content of `message_content`.
///
/// Issues the FTS5 'rebuild' command which deletes the entire full-text index
/// and reconstructs it from the external content table. This must be called
/// after sync operations to keep the FTS index consistent.
///
/// For large databases (100K+ rows), this may take several seconds. An
/// optimization to skip rebuild when no new content was added is handled
/// by the caller (sync_all checks files_synced > 0).
pub fn rebuild_fts_index(conn: &Connection) -> Result<(), rusqlite::Error> {
    conn.execute_batch(
        "INSERT INTO fts_message_content(fts_message_content) VALUES('rebuild');",
    )?;
    tracing::info!("FTS5 message content index rebuilt");
    Ok(())
}

/// Search across message content using FTS5 full-text search.
///
/// Returns ranked results with context snippets. The query is wrapped in
/// double quotes to sanitize user input (treats as phrase search), with
/// internal quotes escaped by doubling.
///
/// Results are ordered by BM25 relevance (ascending -- lower values indicate
/// better matches per SQLite FTS5 documentation). Pagination is supported
/// via `limit` and `offset` parameters.
///
/// The SQL joins fts_message_content -> message_content -> messages to
/// provide session context for each match.
pub fn search_messages(
    conn: &Connection,
    query: &str,
    limit: usize,
    offset: usize,
) -> Result<Vec<SearchResult>, rusqlite::Error> {
    // Sanitize: wrap in quotes for phrase matching, escape internal quotes
    // by doubling them. This prevents FTS5 syntax injection (Research Pitfall 3).
    let safe_query = format!("\"{}\"", query.replace('"', "\"\""));

    let mut stmt = conn.prepare(
        "SELECT
            mc.message_uuid,
            m.session_id,
            m.type,
            m.timestamp,
            mc.block_type,
            snippet(fts_message_content, 0, '>>>', '<<<', '...', 30),
            bm25(fts_message_content)
         FROM fts_message_content
         JOIN message_content mc ON mc.id = fts_message_content.rowid
         JOIN messages m ON m.uuid = mc.message_uuid
         WHERE fts_message_content MATCH ?1
         ORDER BY bm25(fts_message_content)
         LIMIT ?2 OFFSET ?3",
    )?;

    let results = stmt.query_map(
        rusqlite::params![safe_query, limit as i64, offset as i64],
        |row| {
            Ok(SearchResult {
                message_uuid: row.get(0)?,
                session_id: row.get(1)?,
                message_type: row.get(2)?,
                timestamp: row.get(3)?,
                block_type: row.get(4)?,
                snippet: row.get(5)?,
                rank: row.get(6)?,
            })
        },
    )?;

    results.collect()
}

/// Rebuild the FTS5 index for file_operations content.
///
/// External-content FTS5 tables require explicit rebuild after content table
/// changes. This should be called periodically (e.g., every 30 seconds in
/// the watcher loop alongside rebuild_fts_index for message content).
///
/// [FTS-02]
pub fn rebuild_fts_file_operations(conn: &Connection) -> Result<(), rusqlite::Error> {
    conn.execute(
        "INSERT INTO fts_file_operations(fts_file_operations) VALUES('rebuild')",
        [],
    )?;
    tracing::info!("FTS5 file operations index rebuilt");
    Ok(())
}

/// Search file operations content using FTS5 full-text matching.
///
/// Searches across content (written/edited text), old_content (pre-edit text),
/// and command (bash commands) columns. Returns matching file_operations rows
/// ranked by FTS5 relevance.
///
/// The query is sanitized by wrapping in double quotes (same pattern as
/// search_messages) to prevent FTS5 syntax injection.
///
/// Results are ordered by BM25 relevance (ascending -- lower values indicate
/// better matches). Pagination is supported via `limit` and `offset`.
///
/// The SQL joins fts_file_operations with file_operations on rowid to retrieve
/// the full row context (session_id, file_path, operation_type, timestamp).
///
/// [FTS-02, API-21]
pub fn search_file_operations(
    conn: &Connection,
    query: &str,
    limit: usize,
    offset: usize,
) -> Result<Vec<FileOperationSearchResult>, rusqlite::Error> {
    // Sanitize: wrap in quotes for phrase matching, escape internal quotes
    // by doubling them. Same pattern as search_messages.
    let safe_query = format!("\"{}\"", query.replace('"', "\"\""));

    let mut stmt = conn.prepare(
        "SELECT fo.id, fo.session_id, fo.file_path, fo.operation_type,
                snippet(fts_file_operations, 0, '<b>', '</b>', '...', 32) as snippet,
                fo.timestamp,
                rank
         FROM fts_file_operations
         JOIN file_operations fo ON fo.id = fts_file_operations.rowid
         WHERE fts_file_operations MATCH ?1
         ORDER BY rank
         LIMIT ?2 OFFSET ?3",
    )?;

    let results = stmt.query_map(
        rusqlite::params![safe_query, limit as i64, offset as i64],
        |row| {
            Ok(FileOperationSearchResult {
                id: row.get(0)?,
                session_id: row.get(1)?,
                file_path: row.get(2)?,
                operation_type: row.get(3)?,
                snippet: row.get(4)?,
                timestamp: row.get(5)?,
                rank: row.get(6)?,
            })
        },
    )?;

    results.collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema;

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

    /// Insert a file operation row with content and command fields for FTS testing.
    fn insert_file_op_with_content(
        conn: &Connection,
        session_id: &str,
        file_path: &str,
        op_type: &str,
        content: Option<&str>,
        old_content: Option<&str>,
        command: Option<&str>,
        tool_use_id: &str,
        message_uuid: &str,
        timestamp: &str,
    ) {
        conn.execute(
            "INSERT INTO file_operations
             (session_id, file_path, operation_type, content, old_content, command,
              tool_use_id, message_uuid, timestamp)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            rusqlite::params![
                session_id, file_path, op_type, content, old_content,
                command, tool_use_id, message_uuid, timestamp
            ],
        )
        .unwrap();
    }

    // -------------------------------------------------------------------
    // Test 1: rebuild_fts_file_operations does not error on empty table
    // -------------------------------------------------------------------
    #[test]
    fn test_fts_rebuild_file_operations_empty_table() {
        let conn = setup_db();
        // Rebuilding when file_operations has zero rows should succeed without error.
        let result = rebuild_fts_file_operations(&conn);
        assert!(
            result.is_ok(),
            "rebuild_fts_file_operations should succeed on empty table: {:?}",
            result.err()
        );
    }

    // -------------------------------------------------------------------
    // Test 2: search returns results after insert + rebuild
    // -------------------------------------------------------------------
    #[test]
    fn test_fts_search_file_operations_returns_results() {
        let conn = setup_db();
        insert_test_session(&conn, "sess-fts-fo");
        insert_test_message(&conn, "msg-fts-fo-1", "sess-fts-fo", "2026-02-20T01:00:00Z");

        // Insert a write operation with searchable content
        insert_file_op_with_content(
            &conn,
            "sess-fts-fo",
            "/src/main.rs",
            "write",
            Some("fn main() { println!(\"hello world\"); }"),
            None,
            None,
            "tool-fts-fo-1",
            "msg-fts-fo-1",
            "2026-02-20T01:00:00Z",
        );

        // Insert a bash operation with searchable command
        insert_file_op_with_content(
            &conn,
            "sess-fts-fo",
            "/src/lib.rs",
            "bash_touch",
            None,
            None,
            Some("touch /src/lib.rs && echo created"),
            "tool-fts-fo-2",
            "msg-fts-fo-1",
            "2026-02-20T01:01:00Z",
        );

        // Rebuild index to incorporate new rows
        rebuild_fts_file_operations(&conn).unwrap();

        // Search for content that exists in the write operation
        let results = search_file_operations(&conn, "hello world", 10, 0).unwrap();
        assert_eq!(results.len(), 1, "Should find 1 result matching 'hello world'");
        assert_eq!(results[0].file_path, "/src/main.rs");
        assert_eq!(results[0].operation_type, "write");
        assert_eq!(results[0].session_id, "sess-fts-fo");
        // Snippet should contain the matched text with <b></b> delimiters
        assert!(
            results[0].snippet.contains("hello"),
            "Snippet should contain matched text, got: {}",
            results[0].snippet
        );
        // Rank should be a finite number (BM25 score)
        assert!(results[0].rank.is_finite(), "Rank should be finite");
    }

    // -------------------------------------------------------------------
    // Test 3: search returns empty for non-matching query
    // -------------------------------------------------------------------
    #[test]
    fn test_fts_search_file_operations_empty_for_nonmatch() {
        let conn = setup_db();
        insert_test_session(&conn, "sess-fts-fo2");
        insert_test_message(&conn, "msg-fts-fo2-1", "sess-fts-fo2", "2026-02-20T01:00:00Z");

        // Insert a file operation with known content
        insert_file_op_with_content(
            &conn,
            "sess-fts-fo2",
            "/src/config.rs",
            "write",
            Some("struct Config { port: u16 }"),
            None,
            None,
            "tool-fts-fo2-1",
            "msg-fts-fo2-1",
            "2026-02-20T01:00:00Z",
        );

        rebuild_fts_file_operations(&conn).unwrap();

        // Search for text that does not exist in any file operation
        let results = search_file_operations(&conn, "nonexistent_xyzzy_query", 10, 0).unwrap();
        assert!(
            results.is_empty(),
            "Should return empty results for non-matching query, got {} results",
            results.len()
        );
    }
}
