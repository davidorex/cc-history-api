//! Combined artifact HTTP endpoint handlers.
//!
//! Provides 2 handlers for session-level artifact views:
//! - GET /v1/artifacts/{session_id} — combined files, git operations, and tool executions
//! - GET /v1/artifacts/{session_id}/timeline — chronological feed of all session activity
//!
//! Each handler takes State<SharedState> and calls conn.call() to run
//! synchronous store query functions on a background thread.
//!
//! Requirement IDs: API-26, API-27

use axum::extract::{Path, Query, State};
use axum::Json;
use serde::Deserialize;

use crate::api::error::ApiError;
use crate::state::SharedState;

// Re-export store types used as JSON response bodies.
use claude_history_store::artifact_queries::{SessionArtifacts, TimelineEntry};

// ---------------------------------------------------------------------------
// Query parameter structs
// ---------------------------------------------------------------------------

/// Query parameters for GET /v1/artifacts/{session_id}/timeline.
#[derive(Debug, Deserialize)]
pub struct TimelineParams {
    /// Maximum timeline entries to return. Defaults to 500.
    pub limit: Option<usize>,
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// Handler for GET /v1/artifacts/{session_id} [API-26].
///
/// Returns combined session artifacts: tracked files, git operations, and
/// tool executions. Tool execution result_summary is truncated to 500 chars.
/// Calls artifact_queries::query_session_artifacts.
pub async fn session_artifacts(
    State(state): State<SharedState>,
    Path(session_id): Path<String>,
) -> Result<Json<SessionArtifacts>, ApiError> {
    let result = state
        .conn
        .call(move |conn| {
            claude_history_store::artifact_queries::query_session_artifacts(conn, &session_id)
        })
        .await?;

    Ok(Json(result))
}

/// Handler for GET /v1/artifacts/{session_id}/timeline [API-27].
///
/// Returns a chronological feed of file operations, git operations, and
/// tool executions for a session. Entries are ordered by timestamp ascending.
/// Calls artifact_queries::query_session_timeline.
pub async fn session_timeline(
    State(state): State<SharedState>,
    Path(session_id): Path<String>,
    Query(params): Query<TimelineParams>,
) -> Result<Json<Vec<TimelineEntry>>, ApiError> {
    let limit = params.limit.unwrap_or(500);

    let results = state
        .conn
        .call(move |conn| {
            claude_history_store::artifact_queries::query_session_timeline(
                conn,
                &session_id,
                limit,
            )
        })
        .await?;

    Ok(Json(results))
}
