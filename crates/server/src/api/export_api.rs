//! Session export HTTP endpoint handler.
//!
//! GET /v1/export/:session_id exports a complete session in the requested
//! format (json, markdown, or csv). The export is written to an in-memory
//! buffer inside conn.call, then returned as the response body with the
//! appropriate Content-Type header.
//!
//! This reuses the existing export::export_json, export_markdown, and
//! export_csv functions which take a &rusqlite::Connection (available
//! inside conn.call) and a &mut impl Write.
//!
//! Requirement ID: API-14

use axum::extract::{Path, Query, State};
use axum::http::header;
use axum::response::Response;
use serde::Deserialize;

use crate::api::error::ApiError;
use crate::state::SharedState;

// ---------------------------------------------------------------------------
// Query parameter structs
// ---------------------------------------------------------------------------

/// Query parameters for GET /v1/export/:session_id.
#[derive(Debug, Deserialize)]
pub struct ExportParams {
    /// Export format: "json" (default), "markdown", or "csv".
    pub format: Option<String>,
}

// ---------------------------------------------------------------------------
// Handler
// ---------------------------------------------------------------------------

/// Handler for GET /v1/export/:session_id.
///
/// Validates the format parameter, then calls the appropriate export function
/// inside conn.call to produce a Vec<u8> buffer. The buffer is returned as
/// the response body with the corresponding Content-Type header:
/// - json: application/json
/// - markdown: text/markdown
/// - csv: text/csv
///
/// Returns 400 Bad Request if format is not one of the valid values.
/// Returns 500 Internal Server Error if the export function fails (which
/// may occur if the session_id does not exist or the DB has issues).
pub async fn handler(
    State(state): State<SharedState>,
    Path(session_id): Path<String>,
    Query(params): Query<ExportParams>,
) -> Result<Response, ApiError> {
    let format = params.format.as_deref().unwrap_or("json");

    // Validate format before entering the DB closure
    let content_type = match format {
        "json" => "application/json",
        "markdown" => "text/markdown",
        "csv" => "text/csv",
        other => {
            return Err(ApiError::BadRequest(format!(
                "Invalid format '{}'. Valid values: json, markdown, csv",
                other
            )));
        }
    };

    let format_owned = format.to_string();
    let content_type_owned = content_type.to_string();

    // Run the export inside conn.call to access the synchronous rusqlite Connection.
    // The export functions write to a Vec<u8> buffer via the Write trait.
    let buffer = state
        .conn
        .call(move |conn| {
            let mut buf: Vec<u8> = Vec::new();

            let result = match format_owned.as_str() {
                "json" => crate::export::export_json(conn, &session_id, &mut buf),
                "markdown" => crate::export::export_markdown(conn, &session_id, &mut buf),
                "csv" => crate::export::export_csv(conn, &session_id, &mut buf),
                // Unreachable due to validation above, but match is exhaustive
                _ => unreachable!(),
            };

            // The export functions return Result<(), Box<dyn Error>>.
            // Map to rusqlite::Error for the conn.call return type.
            // Box<dyn Error> is not Send+Sync, so we convert to string
            // first and wrap in a rusqlite error variant.
            result.map_err(|e| {
                tokio_rusqlite::rusqlite::Error::ToSqlConversionFailure(
                    Box::from(e.to_string()),
                )
            })?;

            Ok(buf)
        })
        .await?;

    // Build the response with the appropriate Content-Type header.
    let response = Response::builder()
        .header(header::CONTENT_TYPE, content_type_owned)
        .body(axum::body::Body::from(buffer))
        .map_err(|e| ApiError::Internal(format!("Failed to build response: {}", e)))?;

    Ok(response)
}
