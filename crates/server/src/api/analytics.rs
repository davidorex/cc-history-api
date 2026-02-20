//! Analytics HTTP endpoint handlers.
//!
//! Provides 3 handlers for analytics resources:
//! - GET /v1/analytics/tokens — token usage statistics with configurable grouping
//! - GET /v1/analytics/tools — tool invocation frequency and error rates
//! - GET /v1/analytics/models — model usage breakdown with percentages
//!
//! The tokens endpoint supports a `group_by` query parameter that selects
//! the aggregation dimension: "model" (default), "session", or "day".
//! Invalid group_by values return 400 Bad Request.
//!
//! Requirement IDs: API-11, API-12, API-13

use axum::extract::{Query, State};
use axum::Json;
use serde::Deserialize;

use crate::api::error::ApiError;
use crate::state::SharedState;

// Re-export store query result types used as JSON response bodies.
use claude_history_store::query::{ModelStats, TokenStats, ToolStats};

// ---------------------------------------------------------------------------
// Query parameter structs
// ---------------------------------------------------------------------------

/// Query parameters for GET /v1/analytics/tokens.
#[derive(Debug, Deserialize)]
pub struct TokensParams {
    /// Grouping dimension: "model" (default), "session", or "day".
    pub group_by: Option<String>,
    /// Optional session ID filter (only used when group_by=session).
    pub session_id: Option<String>,
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// Handler for GET /v1/analytics/tokens.
///
/// Dispatches to the appropriate store query function based on the `group_by`
/// parameter:
/// - "model" (default): token_stats_by_model — aggregate by model name
/// - "session": token_stats_by_session — aggregate by session, with optional
///   session_id filter
/// - "day": token_stats_by_day — aggregate by DATE(timestamp)
///
/// Returns 400 Bad Request if group_by is not one of the valid values.
pub async fn tokens(
    State(state): State<SharedState>,
    Query(params): Query<TokensParams>,
) -> Result<Json<Vec<TokenStats>>, ApiError> {
    let group_by = params.group_by.as_deref().unwrap_or("model");

    match group_by {
        "model" => {
            let results = state
                .conn
                .call(|conn| claude_history_store::query::token_stats_by_model(conn))
                .await?;
            Ok(Json(results))
        }
        "session" => {
            let session_id = params.session_id;
            let results = state
                .conn
                .call(move |conn| {
                    claude_history_store::query::token_stats_by_session(
                        conn,
                        session_id.as_deref(),
                    )
                })
                .await?;
            Ok(Json(results))
        }
        "day" => {
            let results = state
                .conn
                .call(|conn| claude_history_store::query::token_stats_by_day(conn))
                .await?;
            Ok(Json(results))
        }
        other => Err(ApiError::BadRequest(format!(
            "Invalid group_by value '{}'. Valid values: model, session, day",
            other
        ))),
    }
}

/// Handler for GET /v1/analytics/tools.
///
/// Returns tool invocation frequency with error counts, ordered by
/// invocations descending. No query parameters needed.
pub async fn tools(
    State(state): State<SharedState>,
) -> Result<Json<Vec<ToolStats>>, ApiError> {
    let results = state
        .conn
        .call(|conn| claude_history_store::query::tool_frequency(conn))
        .await?;

    Ok(Json(results))
}

/// Handler for GET /v1/analytics/models.
///
/// Returns model usage breakdown with message counts and percentages of
/// total assistant messages. Only models with non-null values appear.
/// No query parameters needed.
pub async fn models(
    State(state): State<SharedState>,
) -> Result<Json<Vec<ModelStats>>, ApiError> {
    let results = state
        .conn
        .call(|conn| claude_history_store::query::model_breakdown(conn))
        .await?;

    Ok(Json(results))
}
