//! Health check endpoint handler.
//!
//! GET /v1/health returns a JSON response with server status, database size,
//! total message record count, and the server version string.
//!
//! The database size is computed from SQLite's page_count * page_size pragmas.
//! The record count is a COUNT(*) on the messages table.
//!
//! Requirement IDs: API-01

use axum::extract::State;
use axum::Json;
use serde::Serialize;

use crate::api::error::ApiError;
use crate::state::SharedState;

/// JSON response body for the health check endpoint.
#[derive(Debug, Serialize)]
pub struct HealthResponse {
    /// Server status indicator (e.g., "ok").
    pub status: String,
    /// Database file size in bytes, computed from SQLite page_count * page_size.
    pub db_size: i64,
    /// Total number of message records in the database.
    pub record_count: i64,
    /// Server version string (from AppState).
    pub version: String,
}

/// Handler for GET /v1/health.
///
/// Queries the SQLite database for page-based size and message count,
/// then returns a HealthResponse JSON body. Errors from the database
/// connection are converted to ApiError::Internal via the From impl.
pub async fn health(
    State(state): State<SharedState>,
) -> Result<Json<HealthResponse>, ApiError> {
    let version = state.version.clone();

    let (db_size, record_count) = state
        .conn
        .call(|conn| {
            // Compute database size from SQLite pragmas
            let db_size: i64 = conn.query_row(
                "SELECT page_count * page_size FROM pragma_page_count(), pragma_page_size()",
                [],
                |row| row.get(0),
            )?;

            // Count total message records
            let record_count: i64 =
                conn.query_row("SELECT COUNT(*) FROM messages", [], |row| row.get(0))?;

            Ok((db_size, record_count))
        })
        .await?;

    Ok(Json(HealthResponse {
        status: "ok".to_string(),
        db_size,
        record_count,
        version,
    }))
}
