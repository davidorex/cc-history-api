//! Message-related HTTP endpoint handlers.
//!
//! Provides 2 handlers for message resources:
//! - POST /v1/messages/query — query messages with JSON body filters
//! - GET /v1/messages/:uuid — get single message detail by UUID
//!
//! Each handler takes State<SharedState> and calls conn.call() to run
//! synchronous store query functions on a background thread.
//!
//! Requirement IDs: API-08, API-09

use axum::extract::{Path, State};
use axum::Json;
use serde::Deserialize;

use crate::api::error::ApiError;
use crate::state::SharedState;

// Re-export store query result types used as JSON response bodies.
use claude_history_store::query::{ExportMessage, MessageResult};

// ---------------------------------------------------------------------------
// Request body structs
// ---------------------------------------------------------------------------

/// JSON request body for POST /v1/messages/query.
///
/// All fields are optional — omitting a field means no filter on that
/// dimension. The handler compiles these into parameterized SQL via
/// store::query::query_messages.
#[derive(Debug, Deserialize)]
pub struct MessageQuery {
    /// Filter by session ID (exact match).
    pub session_id: Option<String>,
    /// Filter by message type (e.g., "user", "assistant").
    pub message_type: Option<String>,
    /// Filter by model name (exact match).
    pub model: Option<String>,
    /// Filter by tool name used (EXISTS subquery on tool_executions).
    pub tool: Option<String>,
    /// Show messages after this date (YYYY-MM-DD or ISO8601).
    pub after: Option<String>,
    /// Show messages before this date (YYYY-MM-DD or ISO8601).
    pub before: Option<String>,
    /// Maximum results to return. Defaults to 100.
    pub limit: Option<usize>,
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// Handler for POST /v1/messages/query.
///
/// Accepts a JSON body with optional filter fields and returns matching
/// messages. All parameters are compiled into parameterized SQL by the
/// store::query::query_messages function — no user input is interpolated
/// directly into SQL strings.
pub async fn query(
    State(state): State<SharedState>,
    Json(body): Json<MessageQuery>,
) -> Result<Json<Vec<MessageResult>>, ApiError> {
    let limit = body.limit.unwrap_or(100);

    let results = state
        .conn
        .call(move |conn| {
            claude_history_store::query::query_messages(
                conn,
                body.session_id.as_deref(),
                body.message_type.as_deref(),
                body.model.as_deref(),
                body.tool.as_deref(),
                body.after.as_deref(),
                body.before.as_deref(),
                limit,
            )
        })
        .await?;

    Ok(Json(results))
}

/// Handler for GET /v1/messages/:uuid.
///
/// Returns a single message with its content blocks and token usage.
/// Returns 404 if the UUID does not exist in the database.
pub async fn by_uuid(
    State(state): State<SharedState>,
    Path(uuid): Path<String>,
) -> Result<Json<ExportMessage>, ApiError> {
    let result = state
        .conn
        .call(move |conn| claude_history_store::query::get_message(conn, &uuid))
        .await?;

    match result {
        Some(msg) => Ok(Json(msg)),
        None => Err(ApiError::NotFound("Message not found".to_string())),
    }
}
