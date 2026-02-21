//! Schema introspection HTTP endpoint handlers.
//!
//! Provides 3 handlers for schema-related resources:
//! - GET /v1/schema — full schema introspection (tables, columns, FKs, views)
//! - GET /v1/schema/versions — Claude Code version history (enhanced timeline
//!   with session_count and new_fields_count; ?diff=true adds field diffs)
//! - GET /v1/schema/drift — schema drift entries grouped by version then
//!   record_type, with promotion status and occurrence counts
//!
//! The versions endpoint supports an optional `diff` query parameter. When
//! `diff=true`, per-version field diffs (new_fields, disappeared_fields) are
//! included in the response.
//!
//! The drift endpoint supports optional `record_type` filtering and a `limit`
//! parameter. Drift entries are grouped by version, then by record_type within
//! each version. Filtering is applied in Rust post-retrieval.
//!
//! Requirement IDs: API-15, API-16, M2-P6, VER-01, VER-02, VER-03

use axum::extract::{Query, State};
use axum::Json;
use serde::{Deserialize, Serialize};

use crate::api::error::ApiError;
use crate::state::SharedState;

// Re-export store query result types used as JSON response bodies.
use claude_history_store::query::{
    VersionDiffEntry, VersionDriftGroup, VersionHistoryEntry,
};

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

/// Query parameters for GET /v1/schema/versions.
#[derive(Debug, Deserialize)]
pub struct VersionsParams {
    /// When true, includes per-version field diffs (new fields, disappeared fields).
    pub diff: Option<bool>,
}

/// Query parameters for GET /v1/schema/drift.
#[derive(Debug, Deserialize)]
pub struct DriftParams {
    /// Filter drift entries by record_type (substring match).
    pub record_type: Option<String>,
    /// Maximum entries to return. If omitted, returns all matching entries.
    pub limit: Option<usize>,
}

/// Response enum for GET /v1/schema/versions.
///
/// Uses `#[serde(untagged)]` so the JSON output is a flat array of either
/// `VersionHistoryEntry` or `VersionDiffEntry` objects, with no wrapping tag.
#[derive(Debug, Serialize)]
#[serde(untagged)]
pub enum VersionsResponse {
    /// Default timeline: version history with session_count and new_fields_count.
    Timeline(Vec<VersionHistoryEntry>),
    /// Diff view: includes new_fields and disappeared_fields per version.
    Diff(Vec<VersionDiffEntry>),
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// Handler for GET /v1/schema/versions.
///
/// Returns Claude Code version history with enhanced metadata. By default,
/// returns a timeline with session_count and new_fields_count per version.
/// With `?diff=true`, includes per-version field diffs (new_fields,
/// disappeared_fields). Ordered by first appearance ascending.
pub async fn versions(
    State(state): State<SharedState>,
    Query(params): Query<VersionsParams>,
) -> Result<Json<VersionsResponse>, ApiError> {
    let diff = params.diff.unwrap_or(false);

    if diff {
        let results = state
            .conn
            .call(|conn| claude_history_store::query::version_history_with_diff(conn))
            .await?;
        Ok(Json(VersionsResponse::Diff(results)))
    } else {
        let results = state
            .conn
            .call(|conn| claude_history_store::query::version_history_enhanced(conn))
            .await?;
        Ok(Json(VersionsResponse::Timeline(results)))
    }
}

/// Handler for GET /v1/schema/drift.
///
/// Returns schema drift entries grouped by version, then by record_type within
/// each version. Each field includes occurrence_count, promotion_status, and a
/// sample_value. Supports optional `record_type` filter (applied in Rust
/// post-retrieval by filtering each group's record_types) and a `limit`
/// parameter (applied by counting total fields across all groups).
pub async fn drift(
    State(state): State<SharedState>,
    Query(params): Query<DriftParams>,
) -> Result<Json<Vec<VersionDriftGroup>>, ApiError> {
    let mut groups = state
        .conn
        .call(|conn| claude_history_store::query::drift_by_version(conn))
        .await?;

    // Apply record_type filter in Rust post-retrieval: keep only record_types
    // that match the filter within each version group.
    if let Some(ref record_type) = params.record_type {
        for group in &mut groups {
            group
                .record_types
                .retain(|rt| rt.record_type.contains(record_type.as_str()));
        }
        // Remove version groups that have no record_types left after filtering.
        groups.retain(|g| !g.record_types.is_empty());
    }

    // Apply limit by counting total fields across all groups and truncating
    // when the limit is reached.
    if let Some(limit) = params.limit {
        let mut total_fields = 0usize;
        let mut truncated_groups = Vec::new();

        for group in groups {
            if total_fields >= limit {
                break;
            }
            let mut truncated_rts = Vec::new();
            for mut rt in group.record_types {
                if total_fields >= limit {
                    break;
                }
                let remaining = limit - total_fields;
                if rt.fields.len() > remaining {
                    rt.fields.truncate(remaining);
                }
                total_fields += rt.fields.len();
                truncated_rts.push(rt);
            }
            truncated_groups.push(VersionDriftGroup {
                version: group.version,
                record_types: truncated_rts,
            });
        }

        groups = truncated_groups;
    }

    Ok(Json(groups))
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
