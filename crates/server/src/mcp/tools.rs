//! MCP tool definitions for claude-history.
//!
//! Each #[tool]-annotated method maps to an existing store function, providing
//! typed MCP tool interfaces over the same data the REST API and CLI expose.

use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, Content};
use rmcp::{tool, tool_router, ErrorData as McpError};
use schemars::JsonSchema;
use serde::Deserialize;

use crate::state::SharedState;

// ---------------------------------------------------------------------------
// Parameter structs
// ---------------------------------------------------------------------------

#[derive(Deserialize, JsonSchema)]
pub struct SearchParams {
    /// FTS5 search query (supports AND, OR, NOT, "phrase match", prefix*)
    pub query: String,
    /// Maximum results to return
    pub limit: Option<usize>,
}

#[derive(Deserialize, JsonSchema)]
pub struct ListSessionsParams {
    /// Filter by project path (substring match)
    pub project: Option<String>,
    /// Show sessions after this date (YYYY-MM-DD or ISO8601)
    pub after: Option<String>,
    /// Show sessions before this date (YYYY-MM-DD or ISO8601)
    pub before: Option<String>,
    /// Maximum sessions to return
    pub limit: Option<usize>,
}

#[derive(Deserialize, JsonSchema)]
pub struct QueryMessagesParams {
    /// Filter by session ID
    pub session_id: Option<String>,
    /// Filter by message type (user, assistant)
    pub message_type: Option<String>,
    /// Filter by model name
    pub model: Option<String>,
    /// Filter by tool name used
    pub tool: Option<String>,
    /// Show messages after this date
    pub after: Option<String>,
    /// Show messages before this date
    pub before: Option<String>,
    /// Maximum results
    pub limit: Option<usize>,
}

#[derive(Deserialize, JsonSchema)]
pub struct ListFilesParams {
    /// Filter by session ID
    pub session_id: Option<String>,
    /// Filter by path substring
    pub path: Option<String>,
    /// Maximum results
    pub limit: Option<usize>,
}

#[derive(Deserialize, JsonSchema)]
pub struct FileHistoryParams {
    /// File path to show history for
    pub path: String,
    /// Filter by session ID
    pub session_id: Option<String>,
    /// Maximum operations to show
    pub limit: Option<usize>,
}

#[derive(Deserialize, JsonSchema)]
pub struct GitLogParams {
    /// Filter by session ID
    pub session_id: Option<String>,
    /// Filter by git operation type (commit, push, checkout, etc.)
    pub operation_type: Option<String>,
    /// Maximum operations to show
    pub limit: Option<usize>,
}

#[derive(Deserialize, JsonSchema)]
pub struct ExecuteSqlParams {
    /// Read-only SQL SELECT query to execute against the database
    pub query: String,
    /// Positional parameters for the query (?1, ?2, ...)
    #[serde(default)]
    pub params: Vec<serde_json::Value>,
}

#[derive(Deserialize, JsonSchema)]
pub struct RunQueryParams {
    /// Name of the canned query (filename without .sql extension)
    pub name: String,
    /// Named parameters as key-value pairs
    #[serde(default)]
    pub params: std::collections::HashMap<String, String>,
}

#[derive(Deserialize, JsonSchema)]
pub struct ListBookmarksParams {
    /// Filter by project path (substring match) or encoded dir name. Omit for all projects.
    pub project: Option<String>,
    /// Maximum results to return (default 50)
    pub limit: Option<usize>,
}

#[derive(Deserialize, JsonSchema)]
pub struct SearchBookmarksParams {
    /// Search text matched against bookmark labels and tags
    pub query: String,
    /// Filter by project path (substring match) or encoded dir name. Omit for all projects.
    pub project: Option<String>,
    /// Maximum results to return (default 50)
    pub limit: Option<usize>,
}

#[derive(Deserialize, JsonSchema)]
pub struct GetBookmarkParams {
    /// Bookmark UUID
    pub id: Option<String>,
    /// Assistant message UUID
    pub assistant_uuid: Option<String>,
    /// Project scope (recommended when using assistant_uuid)
    pub project: Option<String>,
}

// ---------------------------------------------------------------------------
// McpService
// ---------------------------------------------------------------------------

/// MCP tool service backed by the shared AppState.
#[derive(Clone)]
pub struct McpService {
    state: SharedState,
    pub(crate) tool_router: ToolRouter<Self>,
}

impl McpService {
    pub fn new(state: SharedState) -> Self {
        let tool_router = Self::tool_router();
        Self { state, tool_router }
    }
}

/// Serialize a value to pretty JSON and wrap in a CallToolResult.
fn json_result<T: serde::Serialize>(value: &T) -> Result<CallToolResult, McpError> {
    let json = serde_json::to_string_pretty(value)
        .map_err(|e| McpError::internal_error(format!("JSON serialization failed: {e}"), None))?;
    Ok(CallToolResult::success(vec![Content::text(json)]))
}

/// Convert a tokio_rusqlite error to an MCP error.
fn db_error(e: impl std::fmt::Display) -> McpError {
    McpError::internal_error(format!("Database error: {e}"), None)
}

// ---------------------------------------------------------------------------
// Tool implementations
// ---------------------------------------------------------------------------

#[tool_router]
impl McpService {
    #[tool(description = "Search message content using FTS5 full-text search. Supports AND, OR, NOT, \"phrase match\", and prefix* syntax. Results ranked by bm25 relevance.")]
    async fn search_messages(
        &self,
        Parameters(params): Parameters<SearchParams>,
    ) -> Result<CallToolResult, McpError> {
        let limit = params.limit.unwrap_or(20);
        let query = params.query;
        let results = self
            .state
            .conn
            .call(move |conn| {
                claude_history_store::fts::search_messages(conn, &query, limit, 0)
            })
            .await
            .map_err(db_error)?;
        json_result(&results)
    }

    #[tool(description = "List sessions with optional project, date, and limit filters.")]
    async fn list_sessions(
        &self,
        Parameters(params): Parameters<ListSessionsParams>,
    ) -> Result<CallToolResult, McpError> {
        let limit = params.limit.unwrap_or(50);
        let project = params.project;
        let after = params.after;
        let before = params.before;
        let results = self
            .state
            .conn
            .call(move |conn| {
                claude_history_store::query::list_sessions(
                    conn,
                    project.as_deref(),
                    after.as_deref(),
                    before.as_deref(),
                    limit,
                )
            })
            .await
            .map_err(db_error)?;
        json_result(&results)
    }

    #[tool(description = "Query messages with filters for session, type, model, tool, and date range.")]
    async fn query_messages(
        &self,
        Parameters(params): Parameters<QueryMessagesParams>,
    ) -> Result<CallToolResult, McpError> {
        let limit = params.limit.unwrap_or(100);
        let session_id = params.session_id;
        let message_type = params.message_type;
        let model = params.model;
        let tool = params.tool;
        let after = params.after;
        let before = params.before;
        let results = self
            .state
            .conn
            .call(move |conn| {
                claude_history_store::query::query_messages(
                    conn,
                    session_id.as_deref(),
                    message_type.as_deref(),
                    model.as_deref(),
                    tool.as_deref(),
                    after.as_deref(),
                    before.as_deref(),
                    limit,
                )
            })
            .await
            .map_err(db_error)?;
        json_result(&results)
    }

    #[tool(description = "List files touched by Claude Code across sessions, with optional session and path filters.")]
    async fn list_files(
        &self,
        Parameters(params): Parameters<ListFilesParams>,
    ) -> Result<CallToolResult, McpError> {
        let limit = params.limit.unwrap_or(100);
        let session_id = params.session_id;
        let path = params.path;
        let results = self
            .state
            .conn
            .call(move |conn| {
                claude_history_store::artifact_queries::list_files(
                    conn,
                    session_id.as_deref(),
                    path.as_deref(),
                    limit,
                )
            })
            .await
            .map_err(db_error)?;
        json_result(&results)
    }

    #[tool(description = "Show chronological file operations (read, write, edit) on a specific file path.")]
    async fn file_history(
        &self,
        Parameters(params): Parameters<FileHistoryParams>,
    ) -> Result<CallToolResult, McpError> {
        let limit = params.limit.unwrap_or(50);
        let path = params.path;
        let session_id = params.session_id;
        let results = self
            .state
            .conn
            .call(move |conn| {
                claude_history_store::artifact_queries::query_file_operations(
                    conn,
                    &path,
                    session_id.as_deref(),
                    limit,
                )
            })
            .await
            .map_err(db_error)?;
        json_result(&results)
    }

    #[tool(description = "Show git operations (commit, push, checkout, etc.) extracted from Bash tool calls.")]
    async fn git_log(
        &self,
        Parameters(params): Parameters<GitLogParams>,
    ) -> Result<CallToolResult, McpError> {
        let limit = params.limit.unwrap_or(50);
        let session_id = params.session_id;
        let operation_type = params.operation_type;
        let results = self
            .state
            .conn
            .call(move |conn| {
                claude_history_store::artifact_queries::list_git_operations(
                    conn,
                    session_id.as_deref(),
                    operation_type.as_deref(),
                    limit,
                )
            })
            .await
            .map_err(db_error)?;
        json_result(&results)
    }

    #[tool(description = "Show token usage, tool frequency, and model breakdown statistics.")]
    async fn get_stats(&self) -> Result<CallToolResult, McpError> {
        let results = self
            .state
            .conn
            .call(move |conn| {
                let tokens = claude_history_store::query::token_stats_by_model(conn)?;
                let tools = claude_history_store::query::tool_frequency(conn)?;
                let models = claude_history_store::query::model_breakdown(conn)?;
                Ok::<_, tokio_rusqlite::rusqlite::Error>((tokens, tools, models))
            })
            .await
            .map_err(db_error)?;

        let combined = serde_json::json!({
            "token_usage": results.0,
            "tool_frequency": results.1,
            "model_breakdown": results.2,
        });
        json_result(&combined)
    }

    #[tool(description = "Execute a read-only SQL SELECT query against the claude-history database. Use list_queries or the schema reference for table/column info.")]
    async fn execute_sql(
        &self,
        Parameters(params): Parameters<ExecuteSqlParams>,
    ) -> Result<CallToolResult, McpError> {
        claude_history_store::sql_passthrough::validate_sql(&params.query)
            .map_err(|e| McpError::invalid_params(format!("SQL validation failed: {e}"), None))?;

        let query = params.query;
        let positional = params.params;
        let results = self
            .state
            .conn
            .call(move |conn| {
                claude_history_store::sql_passthrough::execute_sql(conn, &query, &positional)
                    .map_err(|e| {
                        tokio_rusqlite::rusqlite::Error::ToSqlConversionFailure(Box::new(
                            std::io::Error::new(std::io::ErrorKind::Other, e.to_string()),
                        ))
                    })
            })
            .await
            .map_err(db_error)?;
        json_result(&results)
    }

    #[tool(description = "Execute a named canned query with parameter binding. Use list_queries to discover available queries.")]
    async fn run_query(
        &self,
        Parameters(params): Parameters<RunQueryParams>,
    ) -> Result<CallToolResult, McpError> {
        let dir = claude_history_store::query_registry::resolve_queries_dir();
        let queries = claude_history_store::query_registry::load_queries(&dir)
            .map_err(|e| McpError::internal_error(format!("Failed to load queries: {e}"), None))?;

        let query = queries.get(&params.name).ok_or_else(|| {
            let available: Vec<&str> = queries.keys().map(|k| k.as_str()).collect();
            McpError::invalid_params(
                format!(
                    "Query '{}' not found. Available: {}",
                    params.name,
                    available.join(", ")
                ),
                None,
            )
        })?;

        let (sql, positional) =
            claude_history_store::query_registry::prepare_sql(query, &params.params).map_err(
                |e| McpError::invalid_params(format!("Parameter binding failed: {e}"), None),
            )?;

        let results = self
            .state
            .conn
            .call(move |conn| {
                claude_history_store::sql_passthrough::execute_sql(conn, &sql, &positional)
                    .map_err(|e| {
                        tokio_rusqlite::rusqlite::Error::ToSqlConversionFailure(Box::new(
                            std::io::Error::new(std::io::ErrorKind::Other, e.to_string()),
                        ))
                    })
            })
            .await
            .map_err(db_error)?;
        json_result(&results)
    }

    #[tool(description = "List all available canned queries. Each query has a name, description, SQL template, and parameter definitions.")]
    async fn list_queries(&self) -> Result<CallToolResult, McpError> {
        let dir = claude_history_store::query_registry::resolve_queries_dir();
        let queries = claude_history_store::query_registry::load_queries(&dir)
            .map_err(|e| McpError::internal_error(format!("Failed to load queries: {e}"), None))?;

        let mut list: Vec<&claude_history_store::query_registry::CannedQuery> =
            queries.values().collect();
        list.sort_by(|a, b| a.name.cmp(&b.name));
        json_result(&list)
    }

    #[tool(description = "List bookmarks from ClaudeHistoryBrowser, sorted by creation date (newest first). Bookmarks are stored in a separate database and survive session history rebuilds.")]
    async fn list_bookmarks(
        &self,
        Parameters(params): Parameters<ListBookmarksParams>,
    ) -> Result<CallToolResult, McpError> {
        let project = params.project;
        let limit = params.limit.unwrap_or(50);
        let results = tokio::task::spawn_blocking(move || {
            claude_history_store::bookmarks::list_bookmarks(project.as_deref(), limit)
        })
        .await
        .map_err(|e| McpError::internal_error(format!("Task join error: {e}"), None))?
        .map_err(db_error)?;
        json_result(&results)
    }

    #[tool(description = "Search bookmarks by label or tag text (substring match). Bookmarks are stored in a separate database and survive session history rebuilds.")]
    async fn search_bookmarks(
        &self,
        Parameters(params): Parameters<SearchBookmarksParams>,
    ) -> Result<CallToolResult, McpError> {
        let query = params.query;
        let project = params.project;
        let limit = params.limit.unwrap_or(50);
        let results = tokio::task::spawn_blocking(move || {
            claude_history_store::bookmarks::search_bookmarks(&query, project.as_deref(), limit)
        })
        .await
        .map_err(|e| McpError::internal_error(format!("Task join error: {e}"), None))?
        .map_err(db_error)?;
        json_result(&results)
    }

    #[tool(description = "Retrieve a single bookmark by its ID or by assistant message UUID. At least one of 'id' or 'assistant_uuid' must be provided.")]
    async fn get_bookmark(
        &self,
        Parameters(params): Parameters<GetBookmarkParams>,
    ) -> Result<CallToolResult, McpError> {
        let id = params.id;
        let assistant_uuid = params.assistant_uuid;
        let project = params.project;
        let result = tokio::task::spawn_blocking(move || {
            claude_history_store::bookmarks::get_bookmark(
                id.as_deref(),
                assistant_uuid.as_deref(),
                project.as_deref(),
            )
        })
        .await
        .map_err(|e| McpError::internal_error(format!("Task join error: {e}"), None))?
        .map_err(db_error)?;
        match result {
            Some(bookmark) => json_result(&bookmark),
            None => Ok(CallToolResult::success(vec![Content::text(
                "No bookmark found matching the given criteria.",
            )])),
        }
    }
}
