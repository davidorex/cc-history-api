//! Attachments-related HTTP endpoint handlers (C1.4).
//!
//! Provides the `/v1/attachments` list and `/v1/attachments/{uuid}` show
//! endpoints over the `attachments` table populated by C1.2's decomposer
//! routing. C1.3 added the FTS-search surface; these handlers add the
//! list-by-filter and fetch-by-uuid surface.
//!
//! Filters (list endpoint):
//! - `project` (optional) — substring match on `sessions.project_path`
//! - `inner_type` (optional) — exact match on `attachments.inner_type`
//! - `since` (optional) — lower bound on `attachments.timestamp`
//! - `limit` (optional) — cap on rows returned, defaults to 50

use axum::extract::{Path, Query, State};
use axum::Json;
use serde::Deserialize;

use crate::api::error::ApiError;
use crate::state::SharedState;

use claude_history_store::query::AttachmentRow;

/// Query parameters for GET /v1/attachments.
#[derive(Debug, Deserialize)]
pub struct AttachmentsParams {
    /// Filter by project path (substring match against `sessions.project_path`).
    pub project: Option<String>,
    /// Filter by exact `attachments.inner_type` (e.g. `hook_success`).
    pub inner_type: Option<String>,
    /// Lower bound on `attachments.timestamp` (ISO-8601 text).
    pub since: Option<String>,
    /// Maximum rows to return. Defaults to 50.
    pub limit: Option<usize>,
}

/// Handler for GET /v1/attachments.
///
/// Lists rows from the `attachments` table with optional project / inner_type
/// / since filters and a row-count cap. Returns a JSON array of
/// [`AttachmentRow`] ordered by `timestamp` DESC.
pub async fn list(
    State(state): State<SharedState>,
    Query(params): Query<AttachmentsParams>,
) -> Result<Json<Vec<AttachmentRow>>, ApiError> {
    let limit = params.limit.unwrap_or(50);
    let results = state
        .conn
        .call(move |conn| {
            claude_history_store::query::attachments_list(
                conn,
                params.project.as_deref(),
                params.inner_type.as_deref(),
                params.since.as_deref(),
                limit,
            )
        })
        .await?;
    Ok(Json(results))
}

/// Handler for GET /v1/attachments/{uuid}.
///
/// Returns a single attachment row by uuid. Returns 404 if no row matches.
pub async fn show(
    State(state): State<SharedState>,
    Path(uuid): Path<String>,
) -> Result<Json<AttachmentRow>, ApiError> {
    let result = state
        .conn
        .call(move |conn| claude_history_store::query::attachment_by_uuid(conn, &uuid))
        .await?;

    match result {
        Some(row) => Ok(Json(row)),
        None => Err(ApiError::NotFound("Attachment not found".to_string())),
    }
}
