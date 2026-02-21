//! Read-only SQL passthrough for POST /v1/sql endpoint.
//!
//! Validates that SQL statements are SELECT-only (no mutations),
//! then executes with parameter binding and returns JSON rows.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum SqlPassthroughError {
    #[error("SQL validation failed: {0}")]
    Validation(String),
    // More variants will be added by P3
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
}
