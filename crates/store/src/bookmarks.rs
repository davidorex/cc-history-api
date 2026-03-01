//! Query functions for ClaudeHistoryBrowser (CHB) bookmarks.
//!
//! CHB stores bookmarks in a separate Core Data SQLite database at
//! `~/.claude/cache/chb/ClaudeHistory.sqlite`. This module opens a read-only
//! connection to that database — it does NOT use the main claude-history
//! connection, preserving the dual-database design where bookmarks survive
//! rebuilds of the session history database.
//!
//! Core Data uses Z-prefixed table/column names:
//! - `ZCDBOOKMARK` — bookmark records
//! - `ZCDPROJECT` — project records (FK via `ZCDBOOKMARK.ZPROJECT → ZCDPROJECT.Z_PK`)
//! - Timestamps are NSDate epoch (seconds since 2001-01-01 00:00:00 UTC)

use chrono::{TimeZone, Utc};
use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// NSDate epoch (2001-01-01T00:00:00Z) to Unix epoch offset in seconds.
const NSDATE_TO_UNIX: i64 = 978_307_200;

/// A bookmark result returned by all query functions.
#[derive(Debug, Serialize, Deserialize)]
pub struct BookmarkResult {
    pub id: String,
    pub assistant_uuid: String,
    pub session_id: String,
    pub label: String,
    pub tags: Vec<String>,
    pub created_at: String, // ISO8601
    pub project_name: String,
    pub project_path: String,
}

/// Resolve the CHB database path: `~/.claude/cache/chb/ClaudeHistory.sqlite`.
fn chb_db_path() -> Option<PathBuf> {
    std::env::var("HOME")
        .ok()
        .map(|h| PathBuf::from(h).join(".claude/cache/chb/ClaudeHistory.sqlite"))
}

/// Open a read-only connection to the CHB database.
fn open_chb_db() -> Result<Connection, String> {
    let path = chb_db_path().ok_or("Could not determine home directory")?;
    if !path.exists() {
        return Err(format!(
            "ClaudeHistoryBrowser database not found at {}",
            path.display()
        ));
    }
    Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|e| format!("Failed to open CHB database: {e}"))
}

/// Convert an NSDate timestamp (seconds since 2001-01-01) to ISO8601 string.
fn nsdate_to_iso8601(nsdate: f64) -> String {
    let unix_secs = nsdate as i64 + NSDATE_TO_UNIX;
    Utc.timestamp_opt(unix_secs, 0)
        .single()
        .map(|dt| dt.to_rfc3339())
        .unwrap_or_else(|| format!("invalid-timestamp:{nsdate}"))
}

/// Deserialize ZTAGSJSON from a JSON string to Vec<String>.
/// Returns empty vec if null or malformed.
fn parse_tags(tags_json: Option<String>) -> Vec<String> {
    tags_json
        .and_then(|s| serde_json::from_str::<Vec<String>>(&s).ok())
        .unwrap_or_default()
}

/// Extract a BookmarkResult from a row using column names.
fn row_to_bookmark(row: &rusqlite::Row<'_>) -> Result<BookmarkResult, rusqlite::Error> {
    let id: String = row.get("ZID")?;
    let assistant_uuid: String = row.get("ZASSISTANTUUID")?;
    let session_id: String = row.get("ZSESSIONID")?;
    let label: String = row.get("ZLABEL")?;
    let tags_json: Option<String> = row.get("ZTAGSJSON")?;
    let created_at: f64 = row.get("ZCREATEDAT")?;
    let project_name: String = row.get("ZDISPLAYNAME")?;
    let project_path: String = row.get("ZDECODEDPATH")?;

    Ok(BookmarkResult {
        id,
        assistant_uuid,
        session_id,
        label,
        tags: parse_tags(tags_json),
        created_at: nsdate_to_iso8601(created_at),
        project_name,
        project_path,
    })
}

/// Shared base SELECT for all bookmark queries.
const BASE_SELECT: &str = "\
    SELECT b.ZID, b.ZASSISTANTUUID, b.ZSESSIONID, b.ZLABEL, b.ZTAGSJSON, \
           b.ZCREATEDAT, p.ZDISPLAYNAME, p.ZDECODEDPATH \
    FROM ZCDBOOKMARK b \
    JOIN ZCDPROJECT p ON b.ZPROJECT = p.Z_PK";

/// Helper: collect rows from a prepared statement with dynamic params.
fn collect_bookmarks(
    stmt: &mut rusqlite::Statement<'_>,
    params: &[&dyn rusqlite::types::ToSql],
) -> Result<Vec<BookmarkResult>, String> {
    let rows = stmt
        .query_map(params, row_to_bookmark)
        .map_err(|e| format!("Query error: {e}"))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("Row extraction error: {e}"))
}

/// List all bookmarks for a project, sorted by creation date (newest first).
///
/// `project` — optional project path substring or encoded dir name.
/// `limit` — max results (caller default: 50).
pub fn list_bookmarks(
    project: Option<&str>,
    limit: usize,
) -> Result<Vec<BookmarkResult>, String> {
    let conn = open_chb_db()?;
    let limit_i64 = limit as i64;

    if let Some(proj) = project {
        let sql = format!(
            "{BASE_SELECT} \
             WHERE (p.ZDECODEDPATH LIKE '%' || ?1 || '%' OR p.ZENCODEDDIRNAME = ?1) \
             ORDER BY b.ZCREATEDAT DESC LIMIT ?2"
        );
        let mut stmt = conn.prepare(&sql).map_err(|e| format!("SQL prepare error: {e}"))?;
        collect_bookmarks(&mut stmt, &[&proj, &limit_i64])
    } else {
        let sql = format!("{BASE_SELECT} ORDER BY b.ZCREATEDAT DESC LIMIT ?1");
        let mut stmt = conn.prepare(&sql).map_err(|e| format!("SQL prepare error: {e}"))?;
        collect_bookmarks(&mut stmt, &[&limit_i64])
    }
}

/// Search bookmarks by label or tag text (LIKE %query%).
///
/// `query` — search text matched against label and tags.
/// `project` — optional project path substring.
/// `limit` — max results.
pub fn search_bookmarks(
    query: &str,
    project: Option<&str>,
    limit: usize,
) -> Result<Vec<BookmarkResult>, String> {
    let conn = open_chb_db()?;
    let limit_i64 = limit as i64;

    if let Some(proj) = project {
        let sql = format!(
            "{BASE_SELECT} \
             WHERE (b.ZLABEL LIKE '%' || ?1 || '%' OR b.ZTAGSJSON LIKE '%' || ?1 || '%') \
             AND (p.ZDECODEDPATH LIKE '%' || ?2 || '%' OR p.ZENCODEDDIRNAME = ?2) \
             ORDER BY b.ZCREATEDAT DESC LIMIT ?3"
        );
        let mut stmt = conn.prepare(&sql).map_err(|e| format!("SQL prepare error: {e}"))?;
        collect_bookmarks(&mut stmt, &[&query, &proj, &limit_i64])
    } else {
        let sql = format!(
            "{BASE_SELECT} \
             WHERE (b.ZLABEL LIKE '%' || ?1 || '%' OR b.ZTAGSJSON LIKE '%' || ?1 || '%') \
             ORDER BY b.ZCREATEDAT DESC LIMIT ?2"
        );
        let mut stmt = conn.prepare(&sql).map_err(|e| format!("SQL prepare error: {e}"))?;
        collect_bookmarks(&mut stmt, &[&query, &limit_i64])
    }
}

/// Retrieve a single bookmark by ID or by assistant message UUID.
///
/// `id` — bookmark UUID (ZID).
/// `assistant_uuid` — assistant message UUID (ZASSISTANTUUID).
/// `project` — project scope (recommended when using assistant_uuid).
///
/// At least one of `id` or `assistant_uuid` must be provided.
pub fn get_bookmark(
    id: Option<&str>,
    assistant_uuid: Option<&str>,
    project: Option<&str>,
) -> Result<Option<BookmarkResult>, String> {
    if id.is_none() && assistant_uuid.is_none() {
        return Err("At least one of 'id' or 'assistant_uuid' must be provided".to_string());
    }

    let conn = open_chb_db()?;

    // Build WHERE clauses and params dynamically with sequential positional params.
    let mut clauses: Vec<String> = Vec::new();
    let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
    let mut idx = 1;

    if let Some(id_val) = id {
        clauses.push(format!("b.ZID = ?{idx}"));
        params.push(Box::new(id_val.to_string()));
        idx += 1;
    }
    if let Some(uuid_val) = assistant_uuid {
        clauses.push(format!("b.ZASSISTANTUUID = ?{idx}"));
        params.push(Box::new(uuid_val.to_string()));
        idx += 1;
    }
    if let Some(proj) = project {
        clauses.push(format!(
            "(p.ZDECODEDPATH LIKE '%' || ?{idx} || '%' OR p.ZENCODEDDIRNAME = ?{idx})"
        ));
        params.push(Box::new(proj.to_string()));
    }

    let where_clause = clauses.join(" AND ");
    let sql = format!("{BASE_SELECT} WHERE {where_clause} LIMIT 1");

    let mut stmt = conn.prepare(&sql).map_err(|e| format!("SQL prepare error: {e}"))?;
    let param_refs: Vec<&dyn rusqlite::types::ToSql> = params.iter().map(|p| p.as_ref()).collect();

    stmt.query_row(param_refs.as_slice(), row_to_bookmark)
        .optional()
        .map_err(|e| format!("Query error: {e}"))
}

/// Extension trait to add `.optional()` to rusqlite results.
trait OptionalExt<T> {
    fn optional(self) -> Result<Option<T>, rusqlite::Error>;
}

impl<T> OptionalExt<T> for Result<T, rusqlite::Error> {
    fn optional(self) -> Result<Option<T>, rusqlite::Error> {
        match self {
            Ok(v) => Ok(Some(v)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e),
        }
    }
}
