//! Session-related HTTP endpoint handlers.
//!
//! Provides 6 handlers for session resources:
//! - GET /v1/sessions — list sessions with optional filters
//! - GET /v1/sessions/:id — get single session detail
//! - GET /v1/sessions/:id/conversation — get ordered messages with content
//! - GET /v1/sessions/:id/tree — get flat message tree structure
//! - GET /v1/sessions/:id/agents — get agent hierarchy
//! - GET /v1/sessions/:id/summary — get aggregated session statistics
//!
//! Each handler takes State<SharedState> and calls conn.call() to run
//! synchronous store query functions on a background thread.
//!
//! Requirement IDs: API-02, API-03, API-04, API-05, API-06, API-07

use axum::extract::{Path, Query, State};
use axum::Json;
use serde::Deserialize;

use crate::api::error::ApiError;
use crate::state::SharedState;

// Re-export store query result types used as JSON response bodies.
use claude_history_store::query::{
    AgentEntry, ConversationMessage, SessionDetail, SessionSummary, SessionSummaryStats, TreeNode,
};

// ---------------------------------------------------------------------------
// Query parameter structs
// ---------------------------------------------------------------------------

/// Query parameters for GET /v1/sessions.
#[derive(Debug, Deserialize)]
pub struct SessionsParams {
    /// Filter by project path (substring match).
    pub project: Option<String>,
    /// Show sessions after this date (YYYY-MM-DD or ISO8601).
    pub after: Option<String>,
    /// Show sessions before this date (YYYY-MM-DD or ISO8601).
    pub before: Option<String>,
    /// Maximum sessions to return. Defaults to 50.
    pub limit: Option<usize>,
    /// (C2.4) Filter to sessions holding (`true`) / lacking (`false`) at
    /// least one message with `plan_content IS NOT NULL`. Absent / `None`
    /// preserves legacy behavior (no filter).
    pub has_plan: Option<bool>,
}

/// Query parameters for GET /v1/sessions/:id/conversation.
#[derive(Debug, Deserialize)]
pub struct ConversationParams {
    /// Include thinking content blocks. Defaults to false.
    pub include_thinking: Option<bool>,
    /// Include tool_use and tool_result content blocks. Defaults to false.
    pub include_tool_io: Option<bool>,
    /// Maximum messages to return. Defaults to 100.
    pub limit: Option<usize>,
    /// Offset for pagination. Defaults to 0.
    pub offset: Option<usize>,
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// Handler for GET /v1/sessions.
///
/// Lists sessions with optional project, date range, and limit filters.
/// Calls store::query::list_sessions and returns the result as JSON.
pub async fn list(
    State(state): State<SharedState>,
    Query(params): Query<SessionsParams>,
) -> Result<Json<Vec<SessionSummary>>, ApiError> {
    let limit = params.limit.unwrap_or(50);

    let results = state
        .conn
        .call(move |conn| {
            claude_history_store::query::list_sessions(
                conn,
                params.project.as_deref(),
                params.after.as_deref(),
                params.before.as_deref(),
                params.has_plan,
                limit,
            )
        })
        .await?;

    Ok(Json(results))
}

/// Handler for GET /v1/sessions/:id.
///
/// Returns detailed information about a single session. Returns 404 if the
/// session_id does not exist in the database.
pub async fn detail(
    State(state): State<SharedState>,
    Path(session_id): Path<String>,
) -> Result<Json<SessionDetail>, ApiError> {
    let result = state
        .conn
        .call(move |conn| claude_history_store::query::get_session(conn, &session_id))
        .await?;

    match result {
        Some(detail) => Ok(Json(detail)),
        None => Err(ApiError::NotFound("Session not found".to_string())),
    }
}

/// Handler for GET /v1/sessions/:id/conversation.
///
/// Returns ordered messages with content blocks and token usage.
/// Supports filtering out thinking and tool_io content blocks, plus
/// LIMIT/OFFSET pagination.
pub async fn conversation(
    State(state): State<SharedState>,
    Path(session_id): Path<String>,
    Query(params): Query<ConversationParams>,
) -> Result<Json<Vec<ConversationMessage>>, ApiError> {
    let include_thinking = params.include_thinking.unwrap_or(false);
    let include_tool_io = params.include_tool_io.unwrap_or(false);
    let limit = params.limit.unwrap_or(100);
    let offset = params.offset.unwrap_or(0);

    let results = state
        .conn
        .call(move |conn| {
            claude_history_store::query::session_conversation(
                conn,
                &session_id,
                include_thinking,
                include_tool_io,
                limit,
                offset,
            )
        })
        .await?;

    Ok(Json(results))
}

/// Handler for GET /v1/sessions/:id/tree.
///
/// Returns a flat list of messages with parent_uuid and is_sidechain fields,
/// plus a children_count for each node. Intended for client-side tree
/// construction without loading full content blocks.
pub async fn tree(
    State(state): State<SharedState>,
    Path(session_id): Path<String>,
) -> Result<Json<Vec<TreeNode>>, ApiError> {
    let results = state
        .conn
        .call(move |conn| claude_history_store::query::session_tree(conn, &session_id))
        .await?;

    Ok(Json(results))
}

/// Handler for GET /v1/sessions/:id/agents.
///
/// Returns agent entries associated with the given session, ordered by
/// first_seen_at ascending.
pub async fn agents(
    State(state): State<SharedState>,
    Path(session_id): Path<String>,
) -> Result<Json<Vec<AgentEntry>>, ApiError> {
    let results = state
        .conn
        .call(move |conn| claude_history_store::query::session_agents(conn, &session_id))
        .await?;

    Ok(Json(results))
}

/// Handler for GET /v1/sessions/:id/summary.
///
/// Returns aggregated statistics for a session: message count, total tokens,
/// tool use counts, timestamp range, and duration. Returns 404 if no messages
/// exist for the session (the session may exist but have zero messages).
pub async fn summary(
    State(state): State<SharedState>,
    Path(session_id): Path<String>,
) -> Result<Json<SessionSummaryStats>, ApiError> {
    let result = state
        .conn
        .call(move |conn| claude_history_store::query::session_summary(conn, &session_id))
        .await?;

    match result {
        Some(stats) => Ok(Json(stats)),
        None => Err(ApiError::NotFound(
            "Session not found or has no messages".to_string(),
        )),
    }
}
