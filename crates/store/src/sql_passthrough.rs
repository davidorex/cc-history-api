//! Read-only SQL passthrough for POST /v1/sql endpoint.
//!
//! Validates that SQL statements are SELECT-only (no mutations),
//! then executes with parameter binding and returns JSON rows.

use rusqlite::types::ValueRef;
use serde_json::Value;
use std::time::Instant;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum SqlPassthroughError {
    #[error("SQL validation failed: {0}")]
    Validation(String),
    #[error("SQL execution error: {0}")]
    Execution(#[from] rusqlite::Error),
    #[error("Query timeout after {0} seconds")]
    Timeout(u64),
    #[error("Parameter type error: {0}")]
    ParamType(String),
}

/// Mutating or otherwise unsafe SQL keywords that must not appear at word
/// boundaries in the statement body. Checked case-insensitively.
const FORBIDDEN_KEYWORDS: &[&str] = &[
    "INSERT", "UPDATE", "DELETE", "DROP", "ALTER", "CREATE", "ATTACH", "DETACH", "PRAGMA",
    "VACUUM", "REINDEX",
];

/// Validate that a SQL string is a read-only SELECT statement.
///
/// Rejects: INSERT, UPDATE, DELETE, DROP, ALTER, CREATE, ATTACH, DETACH,
///          PRAGMA, VACUUM, REINDEX, ANALYZE (write-capable statements).
/// Rejects: Multiple statements (semicolon-separated).
/// Accepts: SELECT, WITH...SELECT (CTEs), EXPLAIN SELECT.
///
/// Returns `Ok(())` if the statement is safe to execute read-only.
pub fn validate_sql(sql: &str) -> Result<(), SqlPassthroughError> {
    let trimmed = sql.trim();

    if trimmed.is_empty() {
        return Err(SqlPassthroughError::Validation(
            "empty SQL statement".into(),
        ));
    }

    // ---- reject multiple statements ----------------------------------------
    // Any semicolon is rejected unless everything after the *last* semicolon
    // is purely whitespace (no comments, no SQL). This is intentionally
    // conservative: even `SELECT 1; -- comment` is rejected for safety.
    if let Some(semi_pos) = trimmed.find(';') {
        let after_semi = &trimmed[semi_pos + 1..];
        if !after_semi.trim().is_empty() {
            return Err(SqlPassthroughError::Validation(
                "multiple statements are not allowed".into(),
            ));
        }
    }

    // Strip any trailing semicolon so downstream checks see bare SQL.
    let stmt = trimmed.trim_end_matches(';').trim().to_string();

    if stmt.is_empty() {
        return Err(SqlPassthroughError::Validation(
            "empty SQL statement".into(),
        ));
    }

    let upper = stmt.to_uppercase();

    // ---- check leading keyword ---------------------------------------------
    let first_word = upper
        .split_whitespace()
        .next()
        .unwrap_or("");

    match first_word {
        "SELECT" => {}
        "WITH" => {
            // CTE — verify there is a SELECT somewhere after WITH ...
            if !contains_keyword_at_word_boundary(&upper, "SELECT") {
                return Err(SqlPassthroughError::Validation(
                    "WITH clause must contain a SELECT".into(),
                ));
            }
        }
        "EXPLAIN" => {
            // EXPLAIN is read-only but we still need a SELECT after it.
            let rest = upper.trim_start_matches("EXPLAIN").trim();
            let next_kw = rest.split_whitespace().next().unwrap_or("");
            if next_kw != "SELECT" && next_kw != "WITH" {
                return Err(SqlPassthroughError::Validation(
                    "EXPLAIN must be followed by SELECT or WITH".into(),
                ));
            }
        }
        _ => {
            return Err(SqlPassthroughError::Validation(format!(
                "statement must begin with SELECT, WITH, or EXPLAIN — found `{first_word}`"
            )));
        }
    }

    // ---- reject forbidden keywords at word boundaries ----------------------
    for &kw in FORBIDDEN_KEYWORDS {
        if contains_keyword_at_word_boundary(&upper, kw) {
            return Err(SqlPassthroughError::Validation(format!(
                "forbidden keyword `{kw}` found in statement"
            )));
        }
    }

    Ok(())
}

/// Timeout in seconds for read-only SQL queries executed via passthrough.
const QUERY_TIMEOUT_SECS: u64 = 5;

/// Execute a validated read-only SQL statement with JSON parameter binding.
///
/// Calls [`validate_sql`] first, then maps each [`serde_json::Value`] param
/// to a rusqlite-compatible type, executes the query, and returns each result
/// row as a `serde_json::Map` with column names as keys.
///
/// A progress handler interrupts execution after [`QUERY_TIMEOUT_SECS`]
/// seconds to guard against runaway queries.
pub fn execute_sql(
    conn: &rusqlite::Connection,
    sql: &str,
    params: &[Value],
) -> Result<Vec<serde_json::Map<String, Value>>, SqlPassthroughError> {
    validate_sql(sql)?;

    // ---- convert JSON params to rusqlite-compatible boxed values -----------
    let boxed_params = json_params_to_sql(params)?;
    let param_refs: Vec<&dyn rusqlite::types::ToSql> =
        boxed_params.iter().map(|b| b.as_ref()).collect();

    // ---- install progress handler for timeout -----------------------------
    // Use sqlite3_progress_handler via FFI since the rusqlite `hooks` feature
    // is not enabled. The callback fires approximately every 1000 VM opcodes
    // and returns non-zero to interrupt the query if the deadline has passed.
    let start = Instant::now();
    let deadline_secs = QUERY_TIMEOUT_SECS;

    unsafe {
        let db_ptr = conn.handle();
        // Box a closure so we can pass it through the FFI void* context.
        let callback: Box<Box<dyn FnMut() -> bool>> = Box::new(Box::new(move || {
            start.elapsed().as_secs() >= deadline_secs
        }));
        let ctx = Box::into_raw(callback) as *mut std::ffi::c_void;

        unsafe extern "C" fn handler(ctx: *mut std::ffi::c_void) -> std::ffi::c_int {
            let cb = &mut *(ctx as *mut Box<dyn FnMut() -> bool>);
            if cb() { 1 } else { 0 }
        }

        rusqlite::ffi::sqlite3_progress_handler(db_ptr, 1000, Some(handler), ctx);

        let result = execute_query(conn, sql, &param_refs);

        // Remove the handler and free the closure.
        rusqlite::ffi::sqlite3_progress_handler(
            db_ptr,
            0,
            None,
            std::ptr::null_mut(),
        );
        drop(Box::from_raw(ctx as *mut Box<dyn FnMut() -> bool>));

        // Map an interrupted error to our Timeout variant.
        match result {
            Ok(rows) => Ok(rows),
            Err(SqlPassthroughError::Execution(ref e))
                if e.to_string().contains("interrupted") =>
            {
                Err(SqlPassthroughError::Timeout(QUERY_TIMEOUT_SECS))
            }
            Err(e) => Err(e),
        }
    }
}

/// Inner query execution — separated so the progress handler can be
/// cleaned up regardless of success or failure.
fn execute_query(
    conn: &rusqlite::Connection,
    sql: &str,
    params: &[&dyn rusqlite::types::ToSql],
) -> Result<Vec<serde_json::Map<String, Value>>, SqlPassthroughError> {
    let mut stmt = conn.prepare(sql)?;

    let col_names: Vec<String> = stmt
        .column_names()
        .into_iter()
        .map(|s| s.to_string())
        .collect();

    let mut rows_out = Vec::new();
    let mut rows = stmt.query(rusqlite::params_from_iter(params.iter()))?;

    while let Some(row) = rows.next()? {
        let mut map = serde_json::Map::new();
        for (i, name) in col_names.iter().enumerate() {
            let val = match row.get_ref(i)? {
                ValueRef::Null => Value::Null,
                ValueRef::Integer(n) => Value::Number(n.into()),
                ValueRef::Real(f) => {
                    serde_json::Number::from_f64(f)
                        .map(Value::Number)
                        .unwrap_or(Value::Null)
                }
                ValueRef::Text(bytes) => {
                    let s = String::from_utf8_lossy(bytes);
                    Value::String(s.into_owned())
                }
                ValueRef::Blob(bytes) => {
                    // Encode blobs as hex strings for JSON safety.
                    Value::String(hex_encode(bytes))
                }
            };
            map.insert(name.clone(), val);
        }
        rows_out.push(map);
    }

    Ok(rows_out)
}

/// Convert a slice of JSON values to boxed rusqlite ToSql parameters.
fn json_params_to_sql(
    params: &[Value],
) -> Result<Vec<Box<dyn rusqlite::types::ToSql>>, SqlPassthroughError> {
    params
        .iter()
        .enumerate()
        .map(|(i, v)| json_value_to_sql(i, v))
        .collect()
}

/// Map a single serde_json::Value to a boxed rusqlite ToSql.
fn json_value_to_sql(
    index: usize,
    value: &Value,
) -> Result<Box<dyn rusqlite::types::ToSql>, SqlPassthroughError> {
    match value {
        Value::Null => Ok(Box::new(rusqlite::types::Null)),
        Value::Bool(b) => Ok(Box::new(if *b { 1i64 } else { 0i64 })),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Ok(Box::new(i))
            } else if let Some(f) = n.as_f64() {
                Ok(Box::new(f))
            } else {
                Err(SqlPassthroughError::ParamType(format!(
                    "parameter {index}: unsupported number value"
                )))
            }
        }
        Value::String(s) => Ok(Box::new(s.clone())),
        Value::Array(_) => Err(SqlPassthroughError::ParamType(format!(
            "parameter {index}: arrays are not supported as SQL parameters"
        ))),
        Value::Object(_) => Err(SqlPassthroughError::ParamType(format!(
            "parameter {index}: objects are not supported as SQL parameters"
        ))),
    }
}

/// Hex-encode a byte slice (lowercase).
fn hex_encode(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        use std::fmt::Write;
        let _ = write!(s, "{b:02x}");
    }
    s
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Return true if `keyword` appears in `haystack` at a word boundary —
/// i.e. not embedded inside an identifier like `UPDATED_AT`.
///
/// Both `haystack` and `keyword` are expected to be uppercase already.
fn contains_keyword_at_word_boundary(haystack: &str, keyword: &str) -> bool {
    let bytes = haystack.as_bytes();
    let kw_bytes = keyword.as_bytes();
    let kw_len = kw_bytes.len();

    let mut start = 0;
    while let Some(pos) = haystack[start..].find(keyword) {
        let abs = start + pos;
        let before_ok = abs == 0 || !bytes[abs - 1].is_ascii_alphanumeric() && bytes[abs - 1] != b'_';
        let after_ok = abs + kw_len >= bytes.len()
            || !bytes[abs + kw_len].is_ascii_alphanumeric() && bytes[abs + kw_len] != b'_';

        if before_ok && after_ok {
            return true;
        }
        start = abs + 1;
    }
    false
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // ---- accepts ----------------------------------------------------------

    #[test]
    fn accepts_simple_select() {
        assert!(validate_sql("SELECT 1").is_ok());
    }

    #[test]
    fn accepts_select_with_where() {
        assert!(validate_sql("SELECT * FROM sessions WHERE id = 1").is_ok());
    }

    #[test]
    fn accepts_select_with_join() {
        assert!(
            validate_sql("SELECT s.id, t.content FROM sessions s JOIN turns t ON s.id = t.session_id")
                .is_ok()
        );
    }

    #[test]
    fn accepts_cte() {
        assert!(
            validate_sql("WITH cte AS (SELECT 1) SELECT * FROM cte").is_ok()
        );
    }

    #[test]
    fn accepts_explain_select() {
        assert!(validate_sql("EXPLAIN SELECT 1").is_ok());
    }

    #[test]
    fn accepts_lowercase() {
        assert!(validate_sql("select * from t").is_ok());
    }

    #[test]
    fn accepts_mixed_case() {
        assert!(validate_sql("Select * From sessions").is_ok());
    }

    #[test]
    fn accepts_leading_whitespace() {
        assert!(validate_sql("  SELECT 1  ").is_ok());
    }

    #[test]
    fn accepts_trailing_semicolon() {
        // A single trailing semicolon with nothing after it should be fine.
        assert!(validate_sql("SELECT 1;").is_ok());
    }

    #[test]
    fn accepts_select_with_updated_at_column() {
        // Column name containing "UPDATE" substring should not trigger rejection.
        assert!(validate_sql("SELECT updated_at FROM sessions").is_ok());
    }

    // ---- rejects ----------------------------------------------------------

    #[test]
    fn rejects_empty() {
        assert!(validate_sql("").is_err());
    }

    #[test]
    fn rejects_whitespace_only() {
        assert!(validate_sql("   ").is_err());
    }

    #[test]
    fn rejects_insert() {
        let err = validate_sql("INSERT INTO t VALUES (1)");
        assert!(err.is_err());
    }

    #[test]
    fn rejects_update() {
        assert!(validate_sql("UPDATE t SET x = 1").is_err());
    }

    #[test]
    fn rejects_delete() {
        assert!(validate_sql("DELETE FROM t").is_err());
    }

    #[test]
    fn rejects_drop() {
        assert!(validate_sql("DROP TABLE sessions").is_err());
    }

    #[test]
    fn rejects_alter() {
        assert!(validate_sql("ALTER TABLE sessions ADD COLUMN x TEXT").is_err());
    }

    #[test]
    fn rejects_create() {
        assert!(validate_sql("CREATE TABLE t (id INTEGER)").is_err());
    }

    #[test]
    fn rejects_attach() {
        assert!(validate_sql("ATTACH DATABASE '/tmp/x.db' AS y").is_err());
    }

    #[test]
    fn rejects_detach() {
        assert!(validate_sql("DETACH DATABASE y").is_err());
    }

    #[test]
    fn rejects_pragma() {
        assert!(validate_sql("PRAGMA table_info(sessions)").is_err());
    }

    #[test]
    fn rejects_vacuum() {
        assert!(validate_sql("VACUUM").is_err());
    }

    #[test]
    fn rejects_multiple_statements() {
        assert!(validate_sql("SELECT 1; SELECT 2").is_err());
    }

    #[test]
    fn rejects_select_then_drop() {
        assert!(validate_sql("SELECT 1; DROP TABLE sessions").is_err());
    }

    #[test]
    fn rejects_semicolon_before_comment() {
        // "SELECT 1; -- comment" has a semicolon before a comment — reject for safety.
        assert!(validate_sql("SELECT 1; -- comment").is_err());
    }

    // ---- execute_sql tests ------------------------------------------------

    /// Helper: create an in-memory SQLite connection for execute_sql tests.
    fn mem_conn() -> rusqlite::Connection {
        rusqlite::Connection::open_in_memory().expect("open in-memory db")
    }

    #[test]
    fn test_execute_simple_select() {
        let conn = mem_conn();
        let rows = execute_sql(&conn, "SELECT 1 AS val", &[]).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["val"], serde_json::json!(1));
    }

    #[test]
    fn test_execute_with_params() {
        let conn = mem_conn();
        let rows = execute_sql(
            &conn,
            "SELECT ?1 AS x",
            &[Value::String("hello".into())],
        )
        .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["x"], serde_json::json!("hello"));
    }

    #[test]
    fn test_execute_with_null_param() {
        let conn = mem_conn();
        let rows = execute_sql(&conn, "SELECT ?1 AS n", &[Value::Null]).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["n"], Value::Null);
    }

    #[test]
    fn test_execute_integer_mapping() {
        let conn = mem_conn();
        let rows = execute_sql(&conn, "SELECT 42 AS num", &[]).unwrap();
        assert_eq!(rows[0]["num"], serde_json::json!(42));
    }

    #[test]
    fn test_execute_text_mapping() {
        let conn = mem_conn();
        let rows = execute_sql(&conn, "SELECT 'abc' AS t", &[]).unwrap();
        assert_eq!(rows[0]["t"], serde_json::json!("abc"));
    }

    #[test]
    fn test_execute_null_mapping() {
        let conn = mem_conn();
        let rows = execute_sql(&conn, "SELECT NULL AS n", &[]).unwrap();
        assert_eq!(rows[0]["n"], Value::Null);
    }

    #[test]
    fn test_execute_rejects_mutation() {
        let conn = mem_conn();
        conn.execute_batch("CREATE TABLE t (id INTEGER)").unwrap();
        let err = execute_sql(&conn, "INSERT INTO t VALUES (1)", &[]);
        assert!(err.is_err());
        match err.unwrap_err() {
            SqlPassthroughError::Validation(_) => {}
            other => panic!("expected Validation error, got: {other}"),
        }
    }

    #[test]
    fn test_execute_invalid_sql() {
        let conn = mem_conn();
        let err = execute_sql(&conn, "SELECT * FROM nonexistent_table", &[]);
        assert!(err.is_err());
        match err.unwrap_err() {
            SqlPassthroughError::Execution(_) => {}
            other => panic!("expected Execution error, got: {other}"),
        }
    }

    #[test]
    fn test_execute_multiple_rows() {
        let conn = mem_conn();
        conn.execute_batch(
            "CREATE TABLE nums (v INTEGER);
             INSERT INTO nums VALUES (1);
             INSERT INTO nums VALUES (2);
             INSERT INTO nums VALUES (3);",
        )
        .unwrap();
        let rows = execute_sql(&conn, "SELECT v FROM nums ORDER BY v", &[]).unwrap();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0]["v"], serde_json::json!(1));
        assert_eq!(rows[1]["v"], serde_json::json!(2));
        assert_eq!(rows[2]["v"], serde_json::json!(3));
    }

    #[test]
    fn test_execute_empty_result() {
        let conn = mem_conn();
        conn.execute_batch("CREATE TABLE empty_t (id INTEGER)").unwrap();
        let rows = execute_sql(&conn, "SELECT id FROM empty_t", &[]).unwrap();
        assert!(rows.is_empty());
    }
}
