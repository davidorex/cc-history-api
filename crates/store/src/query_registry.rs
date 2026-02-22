//! Canned query registry: loads .sql files with optional .toml sidecar metadata
//! from a configurable directory, and provides named-to-positional parameter
//! conversion for execution through sql_passthrough::execute_sql.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// A canned SQL query loaded from disk with metadata.
#[derive(Debug, Clone, Serialize)]
pub struct CannedQuery {
    /// Name derived from the .sql file stem.
    pub name: String,
    /// Raw SQL template with `:param` placeholders.
    pub sql: String,
    /// Human-readable description from .toml sidecar or "No description".
    pub description: String,
    /// Ordered parameter definitions.
    pub params: Vec<ParamDef>,
}

/// Definition of a named parameter for a canned query.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParamDef {
    /// Parameter name (without the leading colon).
    pub name: String,
    /// Human-readable description of the parameter.
    #[serde(default)]
    pub description: String,
    /// Optional default value. If None, the parameter is required.
    pub default: Option<String>,
}

/// TOML sidecar file format for query metadata.
#[derive(Debug, Deserialize)]
struct QueryMeta {
    description: Option<String>,
    #[serde(default)]
    params: Vec<ParamDef>,
}

/// Resolve the queries directory path.
///
/// Priority:
/// 1. $CLAUDE_HISTORY_QUERIES environment variable
/// 2. $HOME/.claude/claude-history/queries/ fallback
pub fn resolve_queries_dir() -> PathBuf {
    if let Ok(p) = std::env::var("CLAUDE_HISTORY_QUERIES") {
        if !p.is_empty() {
            return PathBuf::from(p);
        }
    }

    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home)
        .join(".claude")
        .join("claude-history")
        .join("queries")
}

/// Load all canned queries from a directory.
///
/// Reads every .sql file in `dir`. For each, looks for a matching .toml sidecar
/// for metadata (description, params). If no sidecar exists, auto-discovers
/// `:param` placeholders from the SQL.
///
/// Returns Ok(empty HashMap) with a tracing::warn if the directory does not exist.
pub fn load_queries(dir: &Path) -> Result<HashMap<String, CannedQuery>, Box<dyn std::error::Error>> {
    if !dir.exists() {
        tracing::warn!(
            path = %dir.display(),
            "Queries directory does not exist"
        );
        return Ok(HashMap::new());
    }

    let mut queries = HashMap::new();

    let entries = std::fs::read_dir(dir)?;
    for entry in entries {
        let entry = entry?;
        let path = entry.path();

        // Only process .sql files
        if path.extension().and_then(|e| e.to_str()) != Some("sql") {
            continue;
        }

        let name = match path.file_stem().and_then(|s| s.to_str()) {
            Some(s) => s.to_string(),
            None => continue,
        };

        let sql = std::fs::read_to_string(&path)?;

        // Look for matching .toml sidecar
        let toml_path = path.with_extension("toml");
        let (description, params) = if toml_path.exists() {
            let toml_content = std::fs::read_to_string(&toml_path)?;
            let meta: QueryMeta = toml::from_str(&toml_content)?;
            (
                meta.description.unwrap_or_else(|| "No description".to_string()),
                meta.params,
            )
        } else {
            // Auto-discover params from SQL
            let param_names = extract_named_params(&sql);
            let params = param_names
                .into_iter()
                .map(|name| ParamDef {
                    name,
                    description: String::new(),
                    default: None,
                })
                .collect();
            ("No description".to_string(), params)
        };

        queries.insert(
            name.clone(),
            CannedQuery {
                name,
                sql,
                description,
                params,
            },
        );
    }

    Ok(queries)
}

/// Extract named `:param` placeholders from SQL, skipping content inside
/// single-quoted strings.
///
/// Returns unique parameter names in order of first appearance (without the
/// leading colon).
pub fn extract_named_params(sql: &str) -> Vec<String> {
    let mut params = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let chars: Vec<char> = sql.chars().collect();
    let len = chars.len();
    let mut i = 0;
    let mut inside_quote = false;

    while i < len {
        let ch = chars[i];

        if inside_quote {
            if ch == '\'' {
                // Check for escaped quote ('')
                if i + 1 < len && chars[i + 1] == '\'' {
                    i += 2; // skip ''
                    continue;
                }
                inside_quote = false;
            }
            i += 1;
            continue;
        }

        if ch == '\'' {
            inside_quote = true;
            i += 1;
            continue;
        }

        if ch == ':' && i + 1 < len && (chars[i + 1].is_ascii_alphabetic() || chars[i + 1] == '_') {
            // Collect the param name
            let start = i + 1;
            let mut end = start;
            while end < len && (chars[end].is_ascii_alphanumeric() || chars[end] == '_') {
                end += 1;
            }
            let name: String = chars[start..end].iter().collect();
            if seen.insert(name.clone()) {
                params.push(name);
            }
            i = end;
            continue;
        }

        i += 1;
    }

    params
}

/// Convert named `:param` placeholders to positional `?N` parameters and build
/// the positional params vector for execute_sql.
///
/// Each unique parameter gets a 1-based positional index. The returned SQL has
/// all `:param_name` occurrences replaced with `?N`. The returned Vec contains
/// parameter values in positional order as serde_json::Value::String.
///
/// Returns an error if a required parameter (no default) is not provided in `params`.
pub fn prepare_sql(
    query: &CannedQuery,
    params: &HashMap<String, String>,
) -> Result<(String, Vec<serde_json::Value>), Box<dyn std::error::Error>> {
    // Discover params from the actual SQL to get positional ordering
    let sql_params = extract_named_params(&query.sql);

    // Build a lookup for defaults from the query definition
    let defaults: HashMap<&str, Option<&str>> = query
        .params
        .iter()
        .map(|p| (p.name.as_str(), p.default.as_deref()))
        .collect();

    // Assign positional indices (1-based) and resolve values
    let mut positional_values: Vec<serde_json::Value> = Vec::new();
    let mut name_to_position: HashMap<String, usize> = HashMap::new();

    for (idx, param_name) in sql_params.iter().enumerate() {
        let position = idx + 1;
        name_to_position.insert(param_name.clone(), position);

        // Resolve value: provided > default > error
        let value = if let Some(v) = params.get(param_name) {
            v.clone()
        } else if let Some(Some(default)) = defaults.get(param_name.as_str()) {
            default.to_string()
        } else {
            return Err(format!(
                "missing required parameter: '{}' (no default defined)",
                param_name
            )
            .into());
        };

        positional_values.push(serde_json::Value::String(value));
    }

    // Rewrite SQL: replace :param_name with ?N, skipping quoted strings
    let chars: Vec<char> = query.sql.chars().collect();
    let len = chars.len();
    let mut result = String::with_capacity(query.sql.len());
    let mut i = 0;
    let mut inside_quote = false;

    while i < len {
        let ch = chars[i];

        if inside_quote {
            result.push(ch);
            if ch == '\'' {
                if i + 1 < len && chars[i + 1] == '\'' {
                    result.push(chars[i + 1]);
                    i += 2;
                    continue;
                }
                inside_quote = false;
            }
            i += 1;
            continue;
        }

        if ch == '\'' {
            inside_quote = true;
            result.push(ch);
            i += 1;
            continue;
        }

        if ch == ':' && i + 1 < len && (chars[i + 1].is_ascii_alphabetic() || chars[i + 1] == '_') {
            let start = i + 1;
            let mut end = start;
            while end < len && (chars[end].is_ascii_alphanumeric() || chars[end] == '_') {
                end += 1;
            }
            let name: String = chars[start..end].iter().collect();
            if let Some(&pos) = name_to_position.get(&name) {
                result.push_str(&format!("?{}", pos));
            } else {
                // Unknown param -- pass through as-is (shouldn't happen if extract works correctly)
                result.push(':');
                result.push_str(&name);
            }
            i = end;
            continue;
        }

        result.push(ch);
        i += 1;
    }

    Ok((result, positional_values))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_finds_simple_params() {
        let sql = "SELECT * FROM t WHERE id = :id AND name = :name";
        let params = extract_named_params(sql);
        assert_eq!(params, vec!["id", "name"]);
    }

    #[test]
    fn extract_skips_quoted_strings() {
        let sql = "SELECT * FROM t WHERE label = ':not_a_param' AND id = :real_param";
        let params = extract_named_params(sql);
        assert_eq!(params, vec!["real_param"]);
    }

    #[test]
    fn extract_deduplicates() {
        let sql = "SELECT * FROM t WHERE a = :x AND b = :x";
        let params = extract_named_params(sql);
        assert_eq!(params, vec!["x"]);
    }

    #[test]
    fn extract_handles_underscore_params() {
        let sql = "SELECT * FROM t WHERE col = :my_param_2";
        let params = extract_named_params(sql);
        assert_eq!(params, vec!["my_param_2"]);
    }

    #[test]
    fn extract_handles_escaped_quotes() {
        let sql = "SELECT * FROM t WHERE label = 'it''s :not_param' AND id = :actual";
        let params = extract_named_params(sql);
        assert_eq!(params, vec!["actual"]);
    }

    #[test]
    fn prepare_sql_converts_named_to_positional() {
        let query = CannedQuery {
            name: "test".to_string(),
            sql: "SELECT * FROM t WHERE id = :id AND name = :name".to_string(),
            description: "test query".to_string(),
            params: vec![
                ParamDef {
                    name: "id".to_string(),
                    description: String::new(),
                    default: None,
                },
                ParamDef {
                    name: "name".to_string(),
                    description: String::new(),
                    default: None,
                },
            ],
        };

        let mut params = HashMap::new();
        params.insert("id".to_string(), "42".to_string());
        params.insert("name".to_string(), "alice".to_string());

        let (sql, values) = prepare_sql(&query, &params).unwrap();
        assert_eq!(sql, "SELECT * FROM t WHERE id = ?1 AND name = ?2");
        assert_eq!(values.len(), 2);
        assert_eq!(values[0], serde_json::Value::String("42".to_string()));
        assert_eq!(values[1], serde_json::Value::String("alice".to_string()));
    }

    #[test]
    fn prepare_sql_uses_defaults() {
        let query = CannedQuery {
            name: "test".to_string(),
            sql: "SELECT * FROM t LIMIT :limit".to_string(),
            description: "test".to_string(),
            params: vec![ParamDef {
                name: "limit".to_string(),
                description: "max rows".to_string(),
                default: Some("20".to_string()),
            }],
        };

        let params = HashMap::new(); // no params provided
        let (sql, values) = prepare_sql(&query, &params).unwrap();
        assert_eq!(sql, "SELECT * FROM t LIMIT ?1");
        assert_eq!(values[0], serde_json::Value::String("20".to_string()));
    }

    #[test]
    fn prepare_sql_errors_on_missing_required() {
        let query = CannedQuery {
            name: "test".to_string(),
            sql: "SELECT * FROM t WHERE id = :id".to_string(),
            description: "test".to_string(),
            params: vec![ParamDef {
                name: "id".to_string(),
                description: String::new(),
                default: None,
            }],
        };

        let params = HashMap::new();
        let result = prepare_sql(&query, &params);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("missing required parameter"));
        assert!(err.contains("id"));
    }

    #[test]
    fn prepare_sql_skips_quoted_params() {
        let query = CannedQuery {
            name: "test".to_string(),
            sql: "SELECT * FROM t WHERE label = ':fake' AND id = :real".to_string(),
            description: "test".to_string(),
            params: vec![ParamDef {
                name: "real".to_string(),
                description: String::new(),
                default: None,
            }],
        };

        let mut params = HashMap::new();
        params.insert("real".to_string(), "99".to_string());

        let (sql, values) = prepare_sql(&query, &params).unwrap();
        assert_eq!(
            sql,
            "SELECT * FROM t WHERE label = ':fake' AND id = ?1"
        );
        assert_eq!(values.len(), 1);
        assert_eq!(values[0], serde_json::Value::String("99".to_string()));
    }

    #[test]
    fn prepare_sql_handles_repeated_params() {
        let query = CannedQuery {
            name: "test".to_string(),
            sql: "SELECT * FROM t WHERE a = :x OR b = :x".to_string(),
            description: "test".to_string(),
            params: vec![ParamDef {
                name: "x".to_string(),
                description: String::new(),
                default: None,
            }],
        };

        let mut params = HashMap::new();
        params.insert("x".to_string(), "val".to_string());

        let (sql, values) = prepare_sql(&query, &params).unwrap();
        // Both :x should map to the same positional ?1
        assert_eq!(sql, "SELECT * FROM t WHERE a = ?1 OR b = ?1");
        assert_eq!(values.len(), 1);
    }

    #[test]
    fn load_queries_empty_for_nonexistent_dir() {
        let result = load_queries(Path::new("/nonexistent/path/to/queries"));
        assert!(result.is_ok());
        assert!(result.unwrap().is_empty());
    }
}
