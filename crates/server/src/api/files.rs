//! File-related HTTP endpoint handlers.
//!
//! Provides 6 handlers for file artifact resources:
//! - GET /v1/files — list tracked files with optional session_id, path, limit filters
//! - GET /v1/files/{file_id} — get file entry with all its operations
//! - GET /v1/files/{file_id}/content — reconstruct file content with optional ?at= point-in-time
//! - GET /v1/files/{file_id}/diff — unified diff of all edits to a file
//! - GET /v1/files/search?q= — FTS5 full-text search over file operation content
//! - POST /v1/files/query — glob-pattern query with JSON body filters
//!
//! Each handler takes State<SharedState> and calls conn.call() to run
//! synchronous store query/fts functions on a background thread.
//!
//! Requirement IDs: API-17, API-18, API-19, API-20, API-21, API-22

use axum::extract::{Path, Query, State};
use axum::http::header;
use axum::response::IntoResponse;
use axum::Json;
use serde::Deserialize;

use crate::api::error::ApiError;
use crate::state::SharedState;

// Re-export store types used as JSON response bodies.
use claude_history_store::artifact_queries::{FileEntry, FileOperation};
use claude_history_store::fts::FileOperationSearchResult;

// ---------------------------------------------------------------------------
// Query parameter structs
// ---------------------------------------------------------------------------

/// Query parameters for GET /v1/files.
#[derive(Debug, Deserialize)]
pub struct FilesParams {
    /// Filter by session ID.
    pub session_id: Option<String>,
    /// Filter by file path substring match.
    pub path: Option<String>,
    /// Maximum files to return. Defaults to 100.
    pub limit: Option<usize>,
}

/// Query parameters for GET /v1/files/{file_id}/content.
#[derive(Debug, Deserialize)]
pub struct FileContentParams {
    /// Optional message UUID for point-in-time reconstruction.
    pub at: Option<String>,
}

/// Query parameters for GET /v1/files/search.
#[derive(Debug, Deserialize)]
pub struct FileSearchParams {
    /// The search query string (FTS5 phrase matching via double-quote wrapping).
    pub q: String,
    /// Maximum results to return. Defaults to 20.
    pub limit: Option<usize>,
    /// Offset for pagination. Defaults to 0.
    pub offset: Option<usize>,
}

/// JSON body for POST /v1/files/query.
#[derive(Debug, Deserialize)]
pub struct FileQueryBody {
    /// Glob pattern to match file paths (e.g., "*.rs", "src/**/*.ts").
    pub pattern: Option<String>,
    /// Filter by session ID.
    pub session_id: Option<String>,
    /// Maximum results to return. Defaults to 100.
    pub limit: Option<usize>,
}

/// Response body for GET /v1/files/{file_id} combining entry and operations.
#[derive(Debug, serde::Serialize)]
pub struct FileDetailResponse {
    pub file: FileEntry,
    pub operations: Vec<FileOperation>,
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// Handler for GET /v1/files [API-17].
///
/// Lists tracked files with optional session_id, path substring, and limit filters.
/// Calls artifact_queries::list_files and returns the result as JSON.
pub async fn list_files(
    State(state): State<SharedState>,
    Query(params): Query<FilesParams>,
) -> Result<Json<Vec<FileEntry>>, ApiError> {
    let limit = params.limit.unwrap_or(100);

    let results = state
        .conn
        .call(move |conn| {
            claude_history_store::artifact_queries::list_files(
                conn,
                params.session_id.as_deref(),
                params.path.as_deref(),
                limit,
            )
        })
        .await?;

    Ok(Json(results))
}

/// Handler for GET /v1/files/{file_id} [API-18].
///
/// Returns the file entry and all its operations. Returns 404 if the
/// file_id does not exist in the database.
pub async fn file_detail(
    State(state): State<SharedState>,
    Path(file_id): Path<i64>,
) -> Result<Json<FileDetailResponse>, ApiError> {
    let result = state
        .conn
        .call(move |conn| {
            let file = claude_history_store::artifact_queries::get_file(conn, file_id)?;
            match file {
                Some(f) => {
                    let ops = claude_history_store::artifact_queries::query_file_operations(
                        conn,
                        &f.file_path,
                        Some(&f.session_id),
                        1000,
                    )?;
                    Ok(Some(FileDetailResponse {
                        file: f,
                        operations: ops,
                    }))
                }
                None => Ok(None),
            }
        })
        .await?;

    match result {
        Some(detail) => Ok(Json(detail)),
        None => Err(ApiError::NotFound("File not found".to_string())),
    }
}

/// Handler for GET /v1/files/{file_id}/content [API-19].
///
/// Reconstructs file content by replaying Write and Edit operations.
/// Supports optional `?at=<message_uuid>` for point-in-time reconstruction.
/// Returns plain text (Content-Type: text/plain) or 404 if no content
/// can be reconstructed.
pub async fn file_content(
    State(state): State<SharedState>,
    Path(file_id): Path<i64>,
    Query(params): Query<FileContentParams>,
) -> Result<impl IntoResponse, ApiError> {
    let result = state
        .conn
        .call(move |conn| {
            // Look up file entry to get file_path and session_id
            let file = claude_history_store::artifact_queries::get_file(conn, file_id)?;
            match file {
                Some(f) => {
                    let content = claude_history_store::artifact_queries::reconstruct_file_content(
                        conn,
                        &f.file_path,
                        &f.session_id,
                        params.at.as_deref(),
                    )?;
                    Ok(Some(content))
                }
                None => Ok(None),
            }
        })
        .await?;

    match result {
        Some(Some(content)) => Ok((
            [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
            content,
        )),
        Some(None) => Err(ApiError::NotFound(
            "No reconstructable content for this file".to_string(),
        )),
        None => Err(ApiError::NotFound("File not found".to_string())),
    }
}

/// Handler for GET /v1/files/{file_id}/diff [API-20].
///
/// Generates unified diff output for all mutations to a file in a session.
/// Returns plain text (Content-Type: text/plain).
pub async fn file_diff(
    State(state): State<SharedState>,
    Path(file_id): Path<i64>,
) -> Result<impl IntoResponse, ApiError> {
    let result = state
        .conn
        .call(move |conn| {
            let file = claude_history_store::artifact_queries::get_file(conn, file_id)?;
            match file {
                Some(f) => {
                    let diff = claude_history_store::artifact_queries::generate_file_diff(
                        conn,
                        &f.file_path,
                        &f.session_id,
                    )?;
                    Ok(Some(diff))
                }
                None => Ok(None),
            }
        })
        .await?;

    match result {
        Some(diff) => Ok((
            [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
            diff,
        )),
        None => Err(ApiError::NotFound("File not found".to_string())),
    }
}

/// Handler for GET /v1/files/search [API-21].
///
/// Validates that query string `q` is non-empty, then calls
/// fts::search_file_operations. Results are ranked by BM25 relevance.
pub async fn search_files(
    State(state): State<SharedState>,
    Query(params): Query<FileSearchParams>,
) -> Result<Json<Vec<FileOperationSearchResult>>, ApiError> {
    if params.q.trim().is_empty() {
        return Err(ApiError::BadRequest(
            "Query parameter 'q' must not be empty".to_string(),
        ));
    }

    let limit = params.limit.unwrap_or(20);
    let offset = params.offset.unwrap_or(0);

    let results = state
        .conn
        .call(move |conn| {
            claude_history_store::fts::search_file_operations(conn, &params.q, limit, offset)
        })
        .await?;

    Ok(Json(results))
}

/// Handler for POST /v1/files/query [API-22].
///
/// Accepts JSON body with optional glob pattern and session_id filter.
/// Fetches file candidates from the database and filters in Rust using
/// glob::Pattern::matches_with() for the glob pattern.
pub async fn query_files(
    State(state): State<SharedState>,
    Json(body): Json<FileQueryBody>,
) -> Result<Json<Vec<FileEntry>>, ApiError> {
    let limit = body.limit.unwrap_or(100);

    let results = state
        .conn
        .call(move |conn| {
            claude_history_store::artifact_queries::list_files(
                conn,
                body.session_id.as_deref(),
                None, // no substring filter; glob filtering done in Rust below
                10000, // fetch a generous set for glob filtering
            )
        })
        .await?;

    // Apply glob pattern filtering in Rust if pattern is provided
    let filtered = if let Some(ref pattern_str) = body.pattern {
        let pattern = glob::Pattern::new(pattern_str).map_err(|e| {
            ApiError::BadRequest(format!("Invalid glob pattern: {}", e))
        })?;
        let opts = glob::MatchOptions {
            case_sensitive: true,
            require_literal_separator: false,
            require_literal_leading_dot: false,
        };
        results
            .into_iter()
            .filter(|f| pattern.matches_with(&f.file_path, opts))
            .take(limit)
            .collect()
    } else {
        results.into_iter().take(limit).collect()
    };

    Ok(Json(filtered))
}
