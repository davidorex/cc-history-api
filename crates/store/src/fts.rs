//! FTS5 full-text search over message content.
//!
//! Provides rebuild and search functions for the `fts_message_content` FTS5
//! virtual table created by migration 002. The table uses external-content
//! mode referencing `message_content`, so the index must be rebuilt after
//! each sync operation to reflect new data.
//!
//! Search queries are sanitized by wrapping user input in double quotes,
//! treating the query as a phrase match and preventing FTS5 syntax injection.
//! For advanced FTS5 query syntax, callers can extend this module with a
//! raw-query variant.
//!
//! Requirement IDs: FTS-01, FTS-03

use rusqlite::Connection;
use serde::Serialize;

/// A single search result from FTS5 full-text search over message content.
///
/// Includes session context (session_id, message_type, timestamp) joined from
/// the messages table, plus the FTS5 snippet and BM25 rank score.
#[derive(Debug, Serialize)]
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
