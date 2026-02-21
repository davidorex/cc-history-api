//! POST /v1/sql — read-only parameterized SQL passthrough.
//!
//! Accepts a JSON body with a `query` string and optional `params` array,
//! validates that the query is a read-only SELECT, executes it against the
//! SQLite database, and returns the result rows as a JSON array of objects
//! keyed by column name.

use axum::extract::State;
use axum::Json;
use serde::Deserialize;
use tokio_rusqlite::rusqlite;

use crate::api::error::ApiError;
use crate::state::SharedState;

use claude_history_store::sql_passthrough::{self, SqlPassthroughError};

// ---------------------------------------------------------------------------
// Request body
// ---------------------------------------------------------------------------

/// JSON request body for POST /v1/sql.
#[derive(Deserialize)]
pub struct SqlQuery {
    /// The SQL query to execute (must be a read-only SELECT).
    pub query: String,
    /// Positional parameters for the query (?1, ?2, ...).
    /// Supports null, bool, number, and string values.
    pub params: Option<Vec<serde_json::Value>>,
}

// ---------------------------------------------------------------------------
// Handler
// ---------------------------------------------------------------------------

/// Handler for POST /v1/sql.
///
/// Validates the SQL statement on the request-handling thread (before
/// entering the database connection pool) so that validation failures
/// return 400 BadRequest without occupying a connection. Execution and
/// timeout errors propagate through the standard tokio_rusqlite error
/// path and become 500 Internal, except param-type errors which are
/// caught and returned as 400.
pub async fn execute(
    State(state): State<SharedState>,
    Json(body): Json<SqlQuery>,
) -> Result<Json<Vec<serde_json::Map<String, serde_json::Value>>>, ApiError> {
    // Validate before entering conn.call — returns 400 directly on failure.
    sql_passthrough::validate_sql(&body.query)
        .map_err(|e| ApiError::BadRequest(e.to_string()))?;

    let query = body.query;
    let params = body.params.unwrap_or_default();

    let results = state
        .conn
        .call(move |conn| {
            sql_passthrough::execute_sql(conn, &query, &params).map_err(|e| match e {
                // Validation is already done above, but execute_sql re-validates
                // internally — map it to rusqlite error for the tokio_rusqlite
                // boundary in the unlikely event it fires.
                SqlPassthroughError::Validation(msg) => {
                    rusqlite::Error::InvalidParameterName(msg)
                }
                SqlPassthroughError::Execution(re) => re,
                SqlPassthroughError::Timeout(secs) => rusqlite::Error::SqliteFailure(
                    rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_INTERRUPT),
                    Some(format!("Query timeout after {secs} seconds")),
                ),
                SqlPassthroughError::ParamType(msg) => {
                    rusqlite::Error::InvalidParameterName(msg)
                }
            })
        })
        .await?;

    Ok(Json(results))
}
