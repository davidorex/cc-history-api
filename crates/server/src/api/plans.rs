//! Plans HTTP endpoint handlers (C2.6).
//!
//! Surfaces the plan-content data model introduced by C2.1 (real
//! `messages.plan_content` column via migration 010) and made
//! FTS-searchable by C2.3 (synthetic `block_type = 'plan_content'`
//! rows in `message_content` via migration 011) over HTTP.
//!
//! Three endpoints:
//!
//!   - `GET /v1/plans` — list plan-bearing messages with optional
//!     `project` (substring), `since` (ISO-8601 lower bound on
//!     `messages.timestamp`), and `limit` (default 50) filters.
//!     Returns previews (~200 chars) of the plan markdown; the full
//!     bodies are intentionally not in list-view to keep payload sizes
//!     reasonable. Mirrors the `claude-history plans list` CLI surface
//!     from C2.4.
//!   - `GET /v1/plans/{session_id}` — full plan markdown bodies for
//!     every plan-bearing message in a session, ordered by timestamp
//!     ascending. Returns 404 when the session has no plan-bearing
//!     messages (which collapses "session not found" and "session
//!     exists but no plans" into one response — callers needing the
//!     distinction query `/v1/sessions/{id}` separately).
//!   - `GET /v1/plans/search?q=...` — FTS5 full-text search restricted
//!     to plan-content rows via the synthetic `block_type =
//!     'plan_content'` rows from migration 011. Complements `/v1/search`
//!     (which unions message content + attachment text) by narrowing
//!     to plan markdown only.
//!
//! All three handlers route via the shared `tokio_rusqlite` pool on
//! `SharedState` and reuse the same store-layer functions exercised by
//! the CLI (`plans_list`, `plan_show`, `search_plans`), so REST and
//! CLI render the same data even when both run concurrently against
//! the same DB.

use axum::extract::{Path, Query, State};
use axum::Json;
use serde::Deserialize;

use crate::api::error::ApiError;
use crate::state::SharedState;

use claude_history_store::fts::SearchResult;
use claude_history_store::query::{PlanFullRow, PlanRow};

/// Query parameters for `GET /v1/plans`.
///
/// Mirrors the CLI `plans list` flags. `limit` defaults to 50 to match
/// the convention used by `/v1/attachments` and the CLI surface.
#[derive(Debug, Deserialize)]
pub struct PlansListParams {
    /// Filter by project path (substring match against `sessions.project_path`).
    pub project: Option<String>,
    /// Lower bound (inclusive) on `messages.timestamp` (ISO-8601 text).
    pub since: Option<String>,
    /// Maximum rows to return. Defaults to 50.
    pub limit: Option<usize>,
}

/// Query parameters for `GET /v1/plans/search`.
#[derive(Debug, Deserialize)]
pub struct PlansSearchParams {
    /// FTS5 query string. Phrase-wrapped + double-quote-escaped by the
    /// store layer's sanitizer; treated as a phrase by default.
    pub q: String,
    /// Maximum results to return. Defaults to 20 (matches `/v1/search`).
    pub limit: Option<usize>,
    /// Pagination offset. Defaults to 0.
    pub offset: Option<usize>,
}

/// Handler for `GET /v1/plans`.
///
/// Returns rows from `messages` where `plan_content IS NOT NULL` joined
/// to `sessions` for project context, ordered by `timestamp DESC`.
/// `plan_content_preview` carries the first ~200 chars of the markdown;
/// callers wanting the full body fetch `/v1/plans/{session_id}`.
pub async fn list(
    State(state): State<SharedState>,
    Query(params): Query<PlansListParams>,
) -> Result<Json<Vec<PlanRow>>, ApiError> {
    let limit = params.limit.unwrap_or(50);
    let results = state
        .conn
        .call(move |conn| {
            claude_history_store::query::plans_list(
                conn,
                params.project.as_deref(),
                params.since.as_deref(),
                limit,
            )
        })
        .await?;
    Ok(Json(results))
}

/// Handler for `GET /v1/plans/{session_id}`.
///
/// Returns every plan-bearing message in the named session, ordered by
/// timestamp ascending. Returns 404 when no plan-bearing messages exist
/// for the session (which conflates "no such session" with "session has
/// no plans"; callers needing the distinction query `/v1/sessions/{id}`
/// separately, mirroring the CLI's behavior at `run_plans_show`).
pub async fn show(
    State(state): State<SharedState>,
    Path(session_id): Path<String>,
) -> Result<Json<Vec<PlanFullRow>>, ApiError> {
    let results = state
        .conn
        .call(move |conn| claude_history_store::query::plan_show(conn, &session_id))
        .await?;

    if results.is_empty() {
        return Err(ApiError::NotFound(
            "No plan-bearing messages for session".to_string(),
        ));
    }

    Ok(Json(results))
}

/// Handler for `GET /v1/plans/search`.
///
/// FTS5 full-text search restricted to the synthetic `block_type =
/// 'plan_content'` rows. Validates that `q` is non-empty (matches the
/// `/v1/search` precedent) before invoking the store-layer
/// `search_plans` function.
pub async fn search(
    State(state): State<SharedState>,
    Query(params): Query<PlansSearchParams>,
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
            claude_history_store::fts::search_plans(conn, &params.q, limit, offset)
        })
        .await?;

    Ok(Json(results))
}
