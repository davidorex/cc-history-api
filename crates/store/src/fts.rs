//! FTS5 full-text search over message content, file operations, and
//! attachment textual payloads.
//!
//! Provides rebuild and search functions for three FTS5 virtual tables:
//!   - `fts_message_content` (migration 002): full-text search over message content blocks
//!   - `fts_file_operations` (migration 003): full-text search over file operation
//!     content, old_content, and command columns
//!   - `fts_attachment_text_content` (migration 009, C1.3): full-text search over
//!     textual payloads from four AttachmentBody subtypes
//!     (mcp_instructions_delta.added_blocks joined as one+ documents per row,
//!     skill_listing.content, edited_text_file.snippet,
//!     nested_memory.content.content)
//!
//! `fts_message_content` and `fts_file_operations` use external-content mode,
//! so their indexes must be rebuilt after sync operations to reflect new data.
//! `fts_attachment_text_content` is contentless (does not externalize a single
//! source column) — its rebuild path DELETEs all rows and re-INSERTs from a
//! json_extract / json_each projection over `attachments.body_json`.
//!
//! Search queries are sanitized by wrapping user input in double quotes,
//! treating the query as a phrase match and preventing FTS5 syntax injection.
//! For advanced FTS5 query syntax, callers can extend this module with a
//! raw-query variant.
//!
//! Requirement IDs: FTS-01, FTS-02, FTS-03

use rusqlite::Connection;
use serde::{Deserialize, Serialize};

/// Discriminator on a search result to distinguish message-content matches
/// (the pre-C1.3 default surface) from attachment-content matches (added
/// by C1.3). Serialized into JSON via the default tag/variant convention so
/// REST and MCP consumers can branch on the source without re-deriving from
/// `block_type`.
///
/// The variant carries the attachment subtype string (e.g. `"skill_listing"`)
/// for `Attachment` matches; `Message` carries no extra data because the
/// existing `block_type` field already covers the per-block discriminator
/// for messages.
///
/// Backwards compatibility: the field is `#[serde(default)]` on
/// `SearchResult` so JSON payloads from older daemons (or older bundled
/// MCPB binaries) parse with `source = SearchResultSource::Message`. The
/// CLI formatter renders an empty source as the implicit "message" — no
/// existing CLI output regresses.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", content = "subtype", rename_all = "snake_case")]
pub enum SearchResultSource {
    /// The match comes from `fts_message_content`. The existing
    /// `SearchResult.block_type` distinguishes text/thinking/tool_result.
    Message,
    /// The match comes from `fts_attachment_text_content`. The carried
    /// string is the AttachmentBody inner_type discriminator (one of
    /// `mcp_instructions_delta`, `skill_listing`, `edited_text_file`,
    /// `nested_memory`).
    Attachment(String),
}

impl Default for SearchResultSource {
    fn default() -> Self {
        SearchResultSource::Message
    }
}

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
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    /// UUID of the message containing the matched content block. For
    /// attachment-source matches this carries the attachment uuid (which is
    /// itself the JSONL record uuid, mirroring the messages table shape).
    pub message_uuid: String,
    /// Session ID the (message or attachment) belongs to.
    pub session_id: String,
    /// Message type discriminator (user, assistant). For attachment-source
    /// matches this is set to the literal `"attachment"` so existing CLI
    /// consumers that print this column see a clear discriminator without
    /// new branches.
    pub message_type: String,
    /// ISO8601 timestamp.
    pub timestamp: String,
    /// Content block type (text, thinking, tool_result, tool_use) for
    /// message-source matches; the AttachmentBody inner_type
    /// (mcp_instructions_delta, skill_listing, edited_text_file,
    /// nested_memory) for attachment-source matches.
    pub block_type: String,
    /// Context snippet around the match with >>> <<< delimiters.
    pub snippet: String,
    /// BM25 relevance score. Lower (more negative) values indicate better matches.
    pub rank: f64,
    /// Source discriminator added in C1.3 to distinguish message-content
    /// matches from attachment-content matches. Defaulted to
    /// `SearchResultSource::Message` via `#[serde(default)]` so JSON
    /// payloads from clients pre-dating C1.3 (or older bundled binaries)
    /// deserialize without breakage.
    #[serde(default)]
    pub source: SearchResultSource,
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
                source: SearchResultSource::Message,
            })
        },
    )?;

    results.collect()
}

/// Rebuild the FTS5 index for `fts_attachment_text_content` from the current
/// content of the `attachments` table.
///
/// The contentless FTS5 table is rebuilt by DELETE-then-INSERT (the FTS5
/// `'rebuild'` command operates on external-content tables; a contentless
/// table re-population is structurally a manual delete + insert). All four
/// indexed subtypes are pulled in a single multi-branch INSERT...SELECT,
/// using `json_each` to flatten `mcp_instructions_delta.addedBlocks` into
/// one document per block (preserving phrase boundaries and keeping
/// per-block ranking informative).
///
/// The four extraction paths (mirrored in migration 009's header docstring):
///
///   - mcp_instructions_delta — `json_each(body_json, '$.addedBlocks')`
///     yields one row per block string.
///   - skill_listing — `json_extract(body_json, '$.content')` (single string).
///   - edited_text_file — `json_extract(body_json, '$.snippet')` (single string).
///   - nested_memory — `json_extract(body_json, '$.content.content')` (the
///     inner triple's content, distinct from the outer envelope).
///
/// Rows whose extracted text is NULL or empty are skipped so the FTS table
/// is not polluted with blank documents from records whose body_json shape
/// is unexpectedly empty.
///
/// Idempotency: re-invoking yields the same row count for the same source
/// table (DELETE + INSERT pattern). The contract aligns with the project's
/// idempotency-after-rebuild assumption.
///
/// [FTS-04] (C1.3)
pub fn rebuild_fts_attachment_text_content(
    conn: &Connection,
) -> Result<(), rusqlite::Error> {
    // Delete then re-insert. A single execute_batch keeps the two
    // statements in the same implicit-transaction tick under sqlite's
    // default deferred-transaction shape.
    conn.execute_batch(
        "DELETE FROM fts_attachment_text_content;

         INSERT INTO fts_attachment_text_content
             (attachment_uuid, session_id, inner_type, text_content)
         SELECT
             a.uuid,
             a.session_id,
             a.inner_type,
             je.value
         FROM attachments a, json_each(a.body_json, '$.addedBlocks') je
         WHERE a.inner_type = 'mcp_instructions_delta'
           AND a.body_json IS NOT NULL
           AND je.value IS NOT NULL
           AND length(je.value) > 0;

         INSERT INTO fts_attachment_text_content
             (attachment_uuid, session_id, inner_type, text_content)
         SELECT
             a.uuid,
             a.session_id,
             a.inner_type,
             json_extract(a.body_json, '$.content')
         FROM attachments a
         WHERE a.inner_type = 'skill_listing'
           AND a.body_json IS NOT NULL
           AND json_extract(a.body_json, '$.content') IS NOT NULL
           AND length(json_extract(a.body_json, '$.content')) > 0;

         INSERT INTO fts_attachment_text_content
             (attachment_uuid, session_id, inner_type, text_content)
         SELECT
             a.uuid,
             a.session_id,
             a.inner_type,
             json_extract(a.body_json, '$.snippet')
         FROM attachments a
         WHERE a.inner_type = 'edited_text_file'
           AND a.body_json IS NOT NULL
           AND json_extract(a.body_json, '$.snippet') IS NOT NULL
           AND length(json_extract(a.body_json, '$.snippet')) > 0;

         INSERT INTO fts_attachment_text_content
             (attachment_uuid, session_id, inner_type, text_content)
         SELECT
             a.uuid,
             a.session_id,
             a.inner_type,
             json_extract(a.body_json, '$.content.content')
         FROM attachments a
         WHERE a.inner_type = 'nested_memory'
           AND a.body_json IS NOT NULL
           AND json_extract(a.body_json, '$.content.content') IS NOT NULL
           AND length(json_extract(a.body_json, '$.content.content')) > 0;",
    )?;
    tracing::info!("FTS5 attachment text content index rebuilt");
    Ok(())
}

/// Search attachment textual content using FTS5 full-text matching.
///
/// Searches across the indexed text_content column of
/// `fts_attachment_text_content`. Joins back to `attachments` on
/// attachment_uuid to retrieve the timestamp (the FTS row carries
/// session_id and inner_type as UNINDEXED columns directly).
///
/// The query is sanitized identically to `search_messages` /
/// `search_file_operations` by wrapping in double quotes and escaping
/// internal quotes via doubling. Results are ranked by BM25 ascending.
///
/// Returns `SearchResult`s with `source = SearchResultSource::Attachment(inner_type)`,
/// `message_type = "attachment"`, and `block_type = inner_type` so existing
/// CLI/REST consumers that key off block_type see the subtype directly.
///
/// [FTS-04, API-22] (C1.3)
pub fn search_attachment_text_content(
    conn: &Connection,
    query: &str,
    limit: usize,
    offset: usize,
) -> Result<Vec<SearchResult>, rusqlite::Error> {
    let safe_query = format!("\"{}\"", query.replace('"', "\"\""));

    let mut stmt = conn.prepare(
        "SELECT
            f.attachment_uuid,
            f.session_id,
            f.inner_type,
            a.timestamp,
            snippet(fts_attachment_text_content, 3, '>>>', '<<<', '...', 30),
            bm25(fts_attachment_text_content)
         FROM fts_attachment_text_content f
         LEFT JOIN attachments a ON a.uuid = f.attachment_uuid
         WHERE fts_attachment_text_content MATCH ?1
         ORDER BY bm25(fts_attachment_text_content)
         LIMIT ?2 OFFSET ?3",
    )?;

    let results = stmt.query_map(
        rusqlite::params![safe_query, limit as i64, offset as i64],
        |row| {
            let inner_type: String = row.get(2)?;
            let timestamp: Option<String> = row.get(3)?;
            Ok(SearchResult {
                message_uuid: row.get(0)?,
                session_id: row.get(1)?,
                message_type: "attachment".to_string(),
                timestamp: timestamp.unwrap_or_default(),
                block_type: inner_type.clone(),
                snippet: row.get(4)?,
                rank: row.get(5)?,
                source: SearchResultSource::Attachment(inner_type),
            })
        },
    )?;

    results.collect()
}

/// Combined search across both `fts_message_content` and
/// `fts_attachment_text_content`, merged and re-ranked by BM25 ascending.
///
/// Preserves the existing message-search contract: a query that does not
/// match any attachment text returns the same result set as
/// `search_messages` alone (modulo the new `source` field, which defaults
/// to `Message` for those rows). New attachment-content matches show up
/// with `source = Attachment(inner_type)`.
///
/// Invoked by the CLI `search` subcommand, the REST `/v1/search` handler,
/// and the MCP `search_messages` tool — the three surfaces that
/// pre-C1.3 were single-source. The `limit` / `offset` are applied to the
/// merged ranked set, so a `limit=20` query may pull from either source
/// depending on which side has lower BM25 scores.
///
/// [FTS-04] (C1.3)
pub fn search_messages_and_attachments(
    conn: &Connection,
    query: &str,
    limit: usize,
    offset: usize,
) -> Result<Vec<SearchResult>, rusqlite::Error> {
    // Pull `limit + offset` from each side, merge, sort by rank, then
    // apply the requested offset/limit. Pulling the full pool from each
    // side guarantees the merged top-N is correct (a per-side LIMIT of
    // limit+offset is the minimum that can support a merged top-(limit+offset)).
    let pool = limit.saturating_add(offset);
    let mut all = search_messages(conn, query, pool, 0)?;
    all.extend(search_attachment_text_content(conn, query, pool, 0)?);
    all.sort_by(|a, b| {
        a.rank
            .partial_cmp(&b.rank)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    Ok(all.into_iter().skip(offset).take(limit).collect())
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

    // -------------------------------------------------------------------
    // C1.3 — fts_attachment_text_content rebuild + search
    // -------------------------------------------------------------------

    /// Insert an attachment row directly (bypassing decompose) so the
    /// FTS rebuild path is exercised against a known shape per subtype.
    fn insert_attachment(
        conn: &Connection,
        uuid: &str,
        session_id: &str,
        inner_type: &str,
        body_json: &str,
        timestamp: &str,
    ) {
        conn.execute(
            "INSERT INTO attachments
             (uuid, session_id, parent_uuid, timestamp, cwd, version,
              git_branch, slug, entrypoint, inner_type, body_json)
             VALUES (?1, ?2, NULL, ?3, NULL, NULL, NULL, NULL, NULL, ?4, ?5)",
            rusqlite::params![uuid, session_id, timestamp, inner_type, body_json],
        )
        .unwrap();
    }

    #[test]
    fn test_rebuild_fts_attachment_text_content_empty_table() {
        let conn = setup_db();
        let r = rebuild_fts_attachment_text_content(&conn);
        assert!(
            r.is_ok(),
            "rebuild on empty attachments table must succeed: {:?}",
            r.err()
        );
        let n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM fts_attachment_text_content",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(n, 0, "empty source must produce empty FTS table");
    }

    #[test]
    fn test_rebuild_fts_attachment_text_content_indexes_all_four_subtypes() {
        let conn = setup_db();
        insert_test_session(&conn, "sess-att-fts");

        // mcp_instructions_delta with two added blocks → two FTS rows
        insert_attachment(
            &conn,
            "att-mcp-1",
            "sess-att-fts",
            "mcp_instructions_delta",
            r#"{"addedNames":["foo"],"addedBlocks":["block one mentions FizzBuzz","block two mentions Quokka"]}"#,
            "2026-02-20T02:00:00Z",
        );
        // skill_listing → one FTS row
        insert_attachment(
            &conn,
            "att-skill-1",
            "sess-att-fts",
            "skill_listing",
            r#"{"content":"skill listing content includes Marmot","skillCount":3}"#,
            "2026-02-20T02:01:00Z",
        );
        // edited_text_file → one FTS row
        insert_attachment(
            &conn,
            "att-edit-1",
            "sess-att-fts",
            "edited_text_file",
            r#"{"filename":"/x.rs","snippet":"fn aardvark() { let pangolin = 1; }"}"#,
            "2026-02-20T02:02:00Z",
        );
        // nested_memory → one FTS row from content.content
        insert_attachment(
            &conn,
            "att-nested-1",
            "sess-att-fts",
            "nested_memory",
            r#"{"path":"/m","content":{"path":"/m","type":"file","content":"memory body says Capybara"}}"#,
            "2026-02-20T02:03:00Z",
        );
        // task_reminder is NOT in the indexed-subtypes set → must not appear
        insert_attachment(
            &conn,
            "att-task-1",
            "sess-att-fts",
            "task_reminder",
            r#"{"content":[],"itemCount":0}"#,
            "2026-02-20T02:04:00Z",
        );

        rebuild_fts_attachment_text_content(&conn).unwrap();

        let total: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM fts_attachment_text_content",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            total, 5,
            "expected 2 mcp blocks + 1 skill + 1 edit + 1 nested = 5 FTS rows"
        );

        // Subtype breakdown
        let mut stmt = conn
            .prepare(
                "SELECT inner_type, COUNT(*) FROM fts_attachment_text_content
                 GROUP BY inner_type ORDER BY inner_type",
            )
            .unwrap();
        let counts: Vec<(String, i64)> = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        assert_eq!(
            counts,
            vec![
                ("edited_text_file".to_string(), 1),
                ("mcp_instructions_delta".to_string(), 2),
                ("nested_memory".to_string(), 1),
                ("skill_listing".to_string(), 1),
            ]
        );
    }

    #[test]
    fn test_search_attachment_text_content_returns_match_per_subtype() {
        let conn = setup_db();
        insert_test_session(&conn, "sess-att-search");
        insert_attachment(
            &conn,
            "att-mcp-2",
            "sess-att-search",
            "mcp_instructions_delta",
            r#"{"addedBlocks":["block alpha contains TypeBox sentinel"]}"#,
            "2026-02-20T03:00:00Z",
        );
        insert_attachment(
            &conn,
            "att-edit-2",
            "sess-att-search",
            "edited_text_file",
            r#"{"filename":"/y.ts","snippet":"const TypeBox = require('typebox');"}"#,
            "2026-02-20T03:01:00Z",
        );
        rebuild_fts_attachment_text_content(&conn).unwrap();

        let results = search_attachment_text_content(&conn, "TypeBox", 10, 0).unwrap();
        assert_eq!(results.len(), 2, "both rows mention TypeBox");
        for r in &results {
            assert_eq!(r.message_type, "attachment");
            match &r.source {
                SearchResultSource::Attachment(s) => {
                    assert!(
                        s == "mcp_instructions_delta" || s == "edited_text_file",
                        "unexpected subtype in source: {s}"
                    );
                    assert_eq!(&r.block_type, s, "block_type mirrors source subtype");
                }
                SearchResultSource::Message => panic!("expected Attachment source"),
            }
        }
    }

    #[test]
    fn test_rebuild_fts_attachment_idempotent() {
        let conn = setup_db();
        insert_test_session(&conn, "sess-att-idem");
        insert_attachment(
            &conn,
            "att-skill-idem",
            "sess-att-idem",
            "skill_listing",
            r#"{"content":"idempotency canary"}"#,
            "2026-02-20T04:00:00Z",
        );
        rebuild_fts_attachment_text_content(&conn).unwrap();
        let n1: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM fts_attachment_text_content",
                [],
                |row| row.get(0),
            )
            .unwrap();
        rebuild_fts_attachment_text_content(&conn).unwrap();
        let n2: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM fts_attachment_text_content",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(n1, n2, "rebuild is idempotent in row count");
        assert_eq!(n1, 1);
    }

    #[test]
    fn test_search_messages_and_attachments_unions_both_sources() {
        let conn = setup_db();
        insert_test_session(&conn, "sess-union");
        insert_test_message(
            &conn,
            "msg-union-1",
            "sess-union",
            "2026-02-20T05:00:00Z",
        );
        // Insert message_content row that mentions our canary phrase
        conn.execute(
            "INSERT INTO message_content
                (message_uuid, block_index, block_type, text_content)
             VALUES (?1, 0, 'text', ?2)",
            rusqlite::params![
                "msg-union-1",
                "the message body mentions ZebraFlux explicitly"
            ],
        )
        .unwrap();
        rebuild_fts_index(&conn).unwrap();

        // Insert an attachment that also mentions the canary
        insert_attachment(
            &conn,
            "att-union-1",
            "sess-union",
            "skill_listing",
            r#"{"content":"skill text also mentions ZebraFlux for coverage"}"#,
            "2026-02-20T05:01:00Z",
        );
        rebuild_fts_attachment_text_content(&conn).unwrap();

        let results =
            search_messages_and_attachments(&conn, "ZebraFlux", 10, 0).unwrap();
        assert_eq!(results.len(), 2, "union should yield both rows");
        let sources: Vec<_> = results.iter().map(|r| r.source.clone()).collect();
        assert!(
            sources.iter().any(|s| matches!(s, SearchResultSource::Message)),
            "expected at least one Message-source result"
        );
        assert!(
            sources
                .iter()
                .any(|s| matches!(s, SearchResultSource::Attachment(_))),
            "expected at least one Attachment-source result"
        );
    }

    #[test]
    fn test_search_messages_default_source_is_message() {
        // Existing message-content searches must keep their source field
        // populated as Message (not regress to a missing/null value).
        let conn = setup_db();
        insert_test_session(&conn, "sess-default-src");
        insert_test_message(
            &conn,
            "msg-default-src-1",
            "sess-default-src",
            "2026-02-20T06:00:00Z",
        );
        conn.execute(
            "INSERT INTO message_content
                (message_uuid, block_index, block_type, text_content)
             VALUES (?1, 0, 'text', ?2)",
            rusqlite::params!["msg-default-src-1", "MetaTetra needle phrase"],
        )
        .unwrap();
        rebuild_fts_index(&conn).unwrap();

        let results = search_messages(&conn, "MetaTetra", 10, 0).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].source, SearchResultSource::Message);
    }

    #[test]
    fn test_search_attachment_text_content_skips_empty_extracts() {
        // Attachments whose body_json shape lacks the expected text path
        // must not produce empty FTS rows.
        let conn = setup_db();
        insert_test_session(&conn, "sess-empty-skip");
        // mcp_instructions_delta with empty addedBlocks
        insert_attachment(
            &conn,
            "att-empty-mcp",
            "sess-empty-skip",
            "mcp_instructions_delta",
            r#"{"addedBlocks":[]}"#,
            "2026-02-20T07:00:00Z",
        );
        // skill_listing with empty content string
        insert_attachment(
            &conn,
            "att-empty-skill",
            "sess-empty-skip",
            "skill_listing",
            r#"{"content":""}"#,
            "2026-02-20T07:01:00Z",
        );
        rebuild_fts_attachment_text_content(&conn).unwrap();
        let n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM fts_attachment_text_content",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(n, 0, "empty-body attachments must not produce FTS rows");
    }
}
