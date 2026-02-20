//! Git operation HTTP endpoint handlers.
//!
//! Provides 3 handlers for git artifact resources:
//! - GET /v1/git — list git operations with optional session_id, operation_type filters
//! - GET /v1/git/commits — list commit operations across all sessions
//! - GET /v1/git/commits/{session_id} — list commits for a specific session
//!
//! Each handler takes State<SharedState> and calls conn.call() to run
//! synchronous store query functions on a background thread.
//!
//! Requirement IDs: API-23, API-24, API-25

use axum::extract::{Path, Query, State};
use axum::Json;
use serde::Deserialize;

use crate::api::error::ApiError;
use crate::state::SharedState;

// Re-export store type used as JSON response body.
use claude_history_store::artifact_queries::GitOperation;

// ---------------------------------------------------------------------------
// Query parameter structs
// ---------------------------------------------------------------------------

/// Query parameters for GET /v1/git.
#[derive(Debug, Deserialize)]
pub struct GitParams {
    /// Filter by session ID.
    pub session_id: Option<String>,
    /// Filter by operation type (commit, push, pull, checkout, etc.).
    pub operation_type: Option<String>,
    /// Maximum results to return. Defaults to 100.
    pub limit: Option<usize>,
}

/// Query parameters for GET /v1/git/commits.
#[derive(Debug, Deserialize)]
pub struct GitCommitsParams {
    /// Maximum results to return. Defaults to 100.
    pub limit: Option<usize>,
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// Handler for GET /v1/git [API-23].
///
/// Lists git operations with optional session_id, operation_type, and limit filters.
/// Calls artifact_queries::list_git_operations and returns the result as JSON.
pub async fn list_git(
    State(state): State<SharedState>,
    Query(params): Query<GitParams>,
) -> Result<Json<Vec<GitOperation>>, ApiError> {
    let limit = params.limit.unwrap_or(100);

    let results = state
        .conn
        .call(move |conn| {
            claude_history_store::artifact_queries::list_git_operations(
                conn,
                params.session_id.as_deref(),
                params.operation_type.as_deref(),
                limit,
            )
        })
        .await?;

    Ok(Json(results))
}

/// Handler for GET /v1/git/commits [API-24].
///
/// Lists commit operations across all sessions, ordered by timestamp descending.
/// Calls artifact_queries::list_git_commits with no session filter.
pub async fn git_commits(
    State(state): State<SharedState>,
    Query(params): Query<GitCommitsParams>,
) -> Result<Json<Vec<GitOperation>>, ApiError> {
    let limit = params.limit.unwrap_or(100);

    let results = state
        .conn
        .call(move |conn| {
            claude_history_store::artifact_queries::list_git_commits(conn, None, limit)
        })
        .await?;

    Ok(Json(results))
}

/// Handler for GET /v1/git/commits/{session_id} [API-25].
///
/// Lists commit operations for a specific session, ordered by timestamp descending.
/// Calls artifact_queries::list_git_commits with session_id filter.
pub async fn session_git_commits(
    State(state): State<SharedState>,
    Path(session_id): Path<String>,
) -> Result<Json<Vec<GitOperation>>, ApiError> {
    let results = state
        .conn
        .call(move |conn| {
            claude_history_store::artifact_queries::list_git_commits(
                conn,
                Some(&session_id),
                1000,
            )
        })
        .await?;

    Ok(Json(results))
}
