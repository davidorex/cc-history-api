//! Schema introspection HTTP endpoint handlers.
//!
//! Provides 2 handlers for schema-related resources:
//! - GET /v1/schema/versions — Claude Code version history
//! - GET /v1/schema/drift — schema drift log entries with optional filters
//!
//! The drift endpoint supports optional `record_type` filtering and a `limit`
//! parameter. Filtering is applied in Rust after retrieval, matching the
//! existing CLI pattern from run_schema_drift.
//!
//! Requirement IDs: API-15, API-16

use axum::extract::{Query, State};
use axum::Json;
use serde::Deserialize;

use crate::api::error::ApiError;
use crate::state::SharedState;

// Re-export store query result types used as JSON response bodies.
use claude_history_store::query::{DriftEntry, VersionEntry};

// ---------------------------------------------------------------------------
// Query parameter structs
// ---------------------------------------------------------------------------

/// Query parameters for GET /v1/schema/drift.
#[derive(Debug, Deserialize)]
pub struct DriftParams {
    /// Filter drift entries by record_type (substring match).
    pub record_type: Option<String>,
    /// Maximum entries to return. If omitted, returns all matching entries.
    pub limit: Option<usize>,
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// Handler for GET /v1/schema/versions.
///
/// Returns distinct Claude Code versions observed across all ingested
/// sessions, with first_seen and last_seen timestamps. Ordered by first
/// appearance ascending.
pub async fn versions(
    State(state): State<SharedState>,
) -> Result<Json<Vec<VersionEntry>>, ApiError> {
    let results = state
        .conn
        .call(|conn| claude_history_store::query::version_history(conn))
        .await?;

    Ok(Json(results))
}

/// Handler for GET /v1/schema/drift.
///
/// Returns schema drift log entries ordered by first_seen_at descending.
/// Supports optional `record_type` filter (applied as substring match in
/// Rust post-retrieval, matching the CLI pattern) and a `limit` parameter.
pub async fn drift(
    State(state): State<SharedState>,
    Query(params): Query<DriftParams>,
) -> Result<Json<Vec<DriftEntry>>, ApiError> {
    let results = state
        .conn
        .call(|conn| claude_history_store::query::schema_drift_list(conn))
        .await?;

    // Apply record_type filter in Rust, matching CLI pattern from run_schema_drift
    let filtered: Vec<DriftEntry> = if let Some(ref record_type) = params.record_type {
        results
            .into_iter()
            .filter(|entry| entry.record_type.contains(record_type.as_str()))
            .collect()
    } else {
        results
    };

    // Apply limit if specified
    let limited = if let Some(limit) = params.limit {
        filtered.into_iter().take(limit).collect()
    } else {
        filtered
    };

    Ok(Json(limited))
}
