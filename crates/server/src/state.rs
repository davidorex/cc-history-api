//! Shared application state for the HTTP API server.
//!
//! AppState holds the database connection, version string, and database file
//! path. It is wrapped in Arc for sharing across axum handler tasks.
//!
//! Requirement IDs: API-03, API-04

use std::path::PathBuf;
use std::sync::Arc;

/// Shared application state passed to every axum handler via Extension or State.
///
/// The `conn` field is a `tokio_rusqlite::Connection`, the same async wrapper
/// used by the CLI handlers. This allows API handlers to call `conn.call()`
/// to run synchronous rusqlite queries on a background thread.
pub struct AppState {
    /// Async SQLite connection (same type used by CLI handlers).
    pub conn: tokio_rusqlite::Connection,

    /// Server version string (e.g., from Cargo.toml or build info).
    pub version: String,

    /// Path to the SQLite database file on disk.
    pub db_path: PathBuf,
}

/// Type alias for the Arc-wrapped AppState used in handler signatures.
pub type SharedState = Arc<AppState>;
