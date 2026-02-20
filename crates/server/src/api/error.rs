//! API error type that maps internal errors to HTTP responses.
//!
//! ApiError implements `axum::response::IntoResponse` so handlers can return
//! `Result<impl IntoResponse, ApiError>` and errors automatically become
//! JSON error bodies with appropriate HTTP status codes.
//!
//! Requirement IDs: API-05, API-11

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

// Re-export rusqlite through tokio_rusqlite to avoid adding a direct rusqlite
// dependency to the server crate (per decision [02-03]).
use tokio_rusqlite::rusqlite;

/// API error variants that map to HTTP status codes.
///
/// Each variant carries a human-readable message that is returned in the
/// JSON response body as `{ "error": "message" }`.
#[derive(Debug)]
pub enum ApiError {
    /// Resource not found (404).
    NotFound(String),

    /// Client request was malformed or invalid (400).
    BadRequest(String),

    /// Internal server error (500).
    Internal(String),
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ApiError::NotFound(msg) => write!(f, "Not found: {}", msg),
            ApiError::BadRequest(msg) => write!(f, "Bad request: {}", msg),
            ApiError::Internal(msg) => write!(f, "Internal error: {}", msg),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, message) = match &self {
            ApiError::NotFound(msg) => (StatusCode::NOT_FOUND, msg.clone()),
            ApiError::BadRequest(msg) => (StatusCode::BAD_REQUEST, msg.clone()),
            ApiError::Internal(msg) => (StatusCode::INTERNAL_SERVER_ERROR, msg.clone()),
        };

        let body = serde_json::json!({ "error": message });

        (status, axum::Json(body)).into_response()
    }
}

/// Convert tokio_rusqlite::Error into ApiError.
///
/// ConnectionClosed and Close variants map to Internal (the database connection
/// is unavailable). The Error(inner) variant is inspected: QueryReturnedNoRows
/// maps to NotFound, all others map to Internal.
impl From<tokio_rusqlite::Error> for ApiError {
    fn from(err: tokio_rusqlite::Error) -> Self {
        match err {
            tokio_rusqlite::Error::ConnectionClosed => {
                ApiError::Internal("Database connection closed".to_string())
            }
            tokio_rusqlite::Error::Close(_) => {
                ApiError::Internal("Database connection close error".to_string())
            }
            tokio_rusqlite::Error::Error(ref inner) => match inner {
                rusqlite::Error::QueryReturnedNoRows => {
                    ApiError::NotFound("Resource not found".to_string())
                }
                _ => ApiError::Internal(format!("Database error: {}", err)),
            },
            // tokio_rusqlite::Error is non_exhaustive; default to Internal
            _ => ApiError::Internal(format!("Database error: {}", err)),
        }
    }
}

/// Convert rusqlite::Error directly into ApiError (used inside conn.call closures
/// that return rusqlite::Error before it gets wrapped by tokio_rusqlite).
impl From<rusqlite::Error> for ApiError {
    fn from(err: rusqlite::Error) -> Self {
        match &err {
            rusqlite::Error::QueryReturnedNoRows => {
                ApiError::NotFound("Resource not found".to_string())
            }
            _ => ApiError::Internal(format!("Database error: {}", err)),
        }
    }
}
