//! Hook-executions HTTP endpoint handler (C1.4).
//!
//! Provides the `/v1/hook-executions` list endpoint over the
//! `hook_executions` table populated by C1.2's decomposer routing for the
//! `hook_success` and `hook_permission_decision` attachment subtypes.
//!
//! Filters:
//! - `tool_use_id` (optional) — exact match (joins to `tool_executions.tool_use_id`)
//! - `hook_event` (optional) — exact match (e.g. `PreToolUse`, `PostToolUse`)
//! - `exit_code` (optional) — exact match
//! - `limit` (optional) — cap on rows returned, defaults to 50

use axum::extract::{Query, State};
use axum::Json;
use serde::Deserialize;

use crate::api::error::ApiError;
use crate::state::SharedState;

use claude_history_store::query::HookExecutionRow;

/// Query parameters for GET /v1/hook-executions.
#[derive(Debug, Deserialize)]
pub struct HookExecutionsParams {
    /// Filter by exact `tool_use_id`.
    pub tool_use_id: Option<String>,
    /// Filter by exact `hook_event`.
    pub hook_event: Option<String>,
    /// Filter by exact `exit_code`.
    pub exit_code: Option<i64>,
    /// Maximum rows to return. Defaults to 50.
    pub limit: Option<usize>,
}

/// Handler for GET /v1/hook-executions.
///
/// Lists rows from the `hook_executions` table with optional filters.
/// Returns a JSON array of [`HookExecutionRow`] ordered by `id` DESC.
pub async fn list(
    State(state): State<SharedState>,
    Query(params): Query<HookExecutionsParams>,
) -> Result<Json<Vec<HookExecutionRow>>, ApiError> {
    let limit = params.limit.unwrap_or(50);
    let results = state
        .conn
        .call(move |conn| {
            claude_history_store::query::hook_executions_list(
                conn,
                params.tool_use_id.as_deref(),
                params.hook_event.as_deref(),
                params.exit_code,
                limit,
            )
        })
        .await?;
    Ok(Json(results))
}
