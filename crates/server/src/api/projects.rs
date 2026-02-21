//! Projects HTTP endpoint handlers.
//!
//! Provides 2 handlers for project resources:
//! - GET /v1/projects — list projects with optional limit
//! - GET /v1/projects/{path} — get single project detail
//!
//! Each handler takes State<SharedState> and calls conn.call() to run
//! synchronous store query functions on a background thread.

use axum::extract::{Path, Query, State};
use axum::Json;
use serde::Deserialize;

use crate::api::error::ApiError;
use crate::state::SharedState;

use claude_history_store::query::{ProjectDetail, ProjectEntry};

// ---------------------------------------------------------------------------
// Query parameter structs
// ---------------------------------------------------------------------------

/// Query parameters for GET /v1/projects.
#[derive(Debug, Deserialize)]
pub struct ProjectsParams {
    /// Maximum projects to return. Defaults to 100.
    pub limit: Option<usize>,
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// Handler for GET /v1/projects.
///
/// Lists projects ordered by session_count descending. Reads from the
/// projects table populated by migration 004.
pub async fn list(
    State(state): State<SharedState>,
    Query(params): Query<ProjectsParams>,
) -> Result<Json<Vec<ProjectEntry>>, ApiError> {
    let limit = params.limit.unwrap_or(100);

    let results = state
        .conn
        .call(move |conn| claude_history_store::query::list_projects(conn, limit))
        .await?;

    Ok(Json(results))
}

/// Handler for GET /v1/projects/{path}.
///
/// Returns detailed project information from the v_project_summary view.
/// The path parameter is URL-encoded (slashes become %2F); axum
/// automatically decodes it. Returns 404 if the project_path does not
/// exist.
pub async fn detail(
    State(state): State<SharedState>,
    Path(path): Path<String>,
) -> Result<Json<ProjectDetail>, ApiError> {
    let result = state
        .conn
        .call(move |conn| claude_history_store::query::get_project(conn, &path))
        .await?;

    match result {
        Some(project) => Ok(Json(project)),
        None => Err(ApiError::NotFound("Project not found".into())),
    }
}
