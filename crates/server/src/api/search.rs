//! Full-text search HTTP endpoint handler.
//!
//! GET /v1/search?q= returns FTS5-ranked search results from the
//! fts_message_content virtual table.
//!
//! Requirement ID: API-10

use axum::extract::{Query, State};
use axum::Json;
use serde::Deserialize;

use crate::api::error::ApiError;
use crate::state::SharedState;

// Re-export the search result type from the store crate.
use claude_history_store::fts::SearchResult;

// ---------------------------------------------------------------------------
// Query parameter structs
// ---------------------------------------------------------------------------

/// Query parameters for GET /v1/search.
#[derive(Debug, Deserialize)]
pub struct SearchParams {
    /// The search query string (FTS5 phrase matching via double-quote wrapping).
    pub q: String,
    /// Maximum results to return. Defaults to 20.
    pub limit: Option<usize>,
    /// Offset for pagination. Defaults to 0.
    pub offset: Option<usize>,
}

// ---------------------------------------------------------------------------
// Handler
// ---------------------------------------------------------------------------

/// Handler for GET /v1/search.
///
/// Validates that the query string `q` is non-empty, then calls
/// store::fts::search_messages with the sanitized query. Results are
/// ranked by BM25 relevance (lower/more-negative values indicate better
/// matches) and returned as JSON.
pub async fn search(
    State(state): State<SharedState>,
    Query(params): Query<SearchParams>,
) -> Result<Json<Vec<SearchResult>>, ApiError> {
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
            // C1.3: union FTS over message content AND attachment text.
            // The SearchResult shape carries a `source` discriminator;
            // older REST clients keyed off existing fields continue to
            // parse without modification (the new field is additive).
            claude_history_store::fts::search_messages_and_attachments(
                conn, &params.q, limit, offset,
            )
        })
        .await?;

    Ok(Json(results))
}
