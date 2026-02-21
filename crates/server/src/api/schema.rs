//! Schema introspection HTTP endpoint handlers.
//!
//! Provides 3 handlers for schema-related resources:
//! - GET /v1/schema — full schema introspection (tables, columns, FKs, views)
//! - GET /v1/schema/versions — Claude Code version history
//! - GET /v1/schema/drift — schema drift log entries with optional filters
//!
//! The drift endpoint supports optional `record_type` filtering and a `limit`
//! parameter. Filtering is applied in Rust after retrieval, matching the
//! existing CLI pattern from run_schema_drift.
//!
//! Requirement IDs: API-15, API-16, M2-P6

use axum::extract::{Query, State};
use axum::Json;
use serde::{Deserialize, Serialize};

use crate::api::error::ApiError;
use crate::state::SharedState;

// Re-export store query result types used as JSON response bodies.
use claude_history_store::query::{DriftEntry, VersionEntry};

// ---------------------------------------------------------------------------
// Schema introspection response structs
// ---------------------------------------------------------------------------

/// Full schema introspection response.
#[derive(Debug, Serialize)]
pub struct SchemaInfo {
    pub tables: Vec<TableInfo>,
    pub views: Vec<ViewInfo>,
}

/// Information about a database table.
#[derive(Debug, Serialize)]
pub struct TableInfo {
    pub name: String,
    pub columns: Vec<ColumnInfo>,
    pub foreign_keys: Vec<ForeignKeyInfo>,
}

/// Information about a view.
#[derive(Debug, Serialize)]
pub struct ViewInfo {
    pub name: String,
    pub sql: String,
}

/// Column metadata from PRAGMA table_info.
#[derive(Debug, Serialize)]
pub struct ColumnInfo {
    pub name: String,
    pub col_type: String,
    pub not_null: bool,
    pub primary_key: bool,
}

/// Foreign key relationship from PRAGMA foreign_key_list.
#[derive(Debug, Serialize)]
pub struct ForeignKeyInfo {
    pub from_column: String,
    pub to_table: String,
    pub to_column: String,
}

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

/// Handler for GET /v1/schema.
///
/// Returns comprehensive schema introspection: all user tables with their
/// columns, types, NOT NULL constraints, primary keys, and foreign keys;
/// plus all views with their defining SQL.
///
/// Internal SQLite tables (sqlite_*, schema_versions) are excluded.
pub async fn schema_full(
    State(state): State<SharedState>,
) -> Result<Json<SchemaInfo>, ApiError> {
    let info = state
        .conn
        .call(|conn| {
            // 1. Collect user table names
            let mut table_stmt = conn.prepare(
                "SELECT name FROM sqlite_master
                 WHERE type = 'table'
                   AND name NOT LIKE 'sqlite_%'
                   AND name != 'schema_versions'
                 ORDER BY name",
            )?;
            let table_names: Vec<String> = table_stmt
                .query_map([], |row| row.get(0))?
                .collect::<Result<Vec<_>, _>>()?;

            let mut tables = Vec::with_capacity(table_names.len());

            for table_name in &table_names {
                // 2. Column info via PRAGMA table_info
                let pragma_sql = format!("PRAGMA table_info('{}')", table_name.replace('\'', "''"));
                let mut col_stmt = conn.prepare(&pragma_sql)?;
                let columns: Vec<ColumnInfo> = col_stmt
                    .query_map([], |row| {
                        Ok(ColumnInfo {
                            name: row.get(1)?,
                            col_type: row.get::<_, String>(2)
                                .unwrap_or_default(),
                            not_null: row.get::<_, i32>(3)? != 0,
                            primary_key: row.get::<_, i32>(5)? != 0,
                        })
                    })?
                    .collect::<Result<Vec<_>, _>>()?;

                // 3. Foreign keys via PRAGMA foreign_key_list
                let fk_sql = format!("PRAGMA foreign_key_list('{}')", table_name.replace('\'', "''"));
                let mut fk_stmt = conn.prepare(&fk_sql)?;
                let foreign_keys: Vec<ForeignKeyInfo> = fk_stmt
                    .query_map([], |row| {
                        Ok(ForeignKeyInfo {
                            from_column: row.get(3)?,
                            to_table: row.get(2)?,
                            to_column: row.get(4)?,
                        })
                    })?
                    .collect::<Result<Vec<_>, _>>()?;

                tables.push(TableInfo {
                    name: table_name.clone(),
                    columns,
                    foreign_keys,
                });
            }

            // 4. Collect views
            let mut view_stmt = conn.prepare(
                "SELECT name, sql FROM sqlite_master
                 WHERE type = 'view'
                 ORDER BY name",
            )?;
            let views: Vec<ViewInfo> = view_stmt
                .query_map([], |row| {
                    Ok(ViewInfo {
                        name: row.get(0)?,
                        sql: row.get::<_, Option<String>>(1)?
                            .unwrap_or_default(),
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?;

            Ok(SchemaInfo { tables, views })
        })
        .await?;

    Ok(Json(info))
}
