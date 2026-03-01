//! MCP (Model Context Protocol) server integration.
//!
//! Provides claude-history capabilities as MCP tools via two transports:
//! - Streamable HTTP (nested into the axum daemon at /mcp)
//! - Stdio (for Claude Desktop integration via mcp-stdio subcommand)

pub mod tools;

use rmcp::model::{Implementation, ProtocolVersion, ServerCapabilities, ServerInfo};
use rmcp::{tool_handler, ServerHandler};

use crate::state::SharedState;

pub use tools::McpService;

// ---------------------------------------------------------------------------
// ServerHandler implementation
// ---------------------------------------------------------------------------

#[tool_handler]
impl ServerHandler for McpService {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            protocol_version: ProtocolVersion::V_2025_03_26,
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            server_info: Implementation {
                name: "claude-history".to_string(),
                version: env!("CARGO_PKG_VERSION").to_string(),
                ..Default::default()
            },
            instructions: Some(MCP_INSTRUCTIONS.to_string()),
        }
    }
}

/// Instructions text sent to MCP clients describing available capabilities.
const MCP_INSTRUCTIONS: &str = "\
claude-history provides queryable access to Claude Code session history stored in SQLite.

Available tools:
- search_messages: Full-text search (FTS5) across all message content
- list_sessions: Browse sessions filtered by project, date range
- query_messages: Filter messages by session, type, model, tool, date
- list_files: Files touched by Claude Code across sessions
- file_history: Chronological operations on a specific file
- git_log: Git operations extracted from Bash tool calls
- get_stats: Token usage, tool frequency, model breakdown
- execute_sql: Read-only SQL passthrough (any SELECT query)
- run_query: Execute named canned queries with parameter binding
- list_queries: Discover available canned queries
- list_bookmarks: List bookmarks from ClaudeHistoryBrowser (separate database, survives rebuilds)
- search_bookmarks: Search bookmarks by label or tag text
- get_bookmark: Retrieve a single bookmark by ID or assistant message UUID

For execute_sql, the database schema includes tables: sessions, messages, \
message_content, token_usage, tool_executions, files, file_operations, \
git_operations, projects, agents, version_history, schema_drift_log. \
FTS tables: fts_message_content, fts_file_operations. \
Views: v_file_token_cost, v_file_conversation_context, v_project_summary, \
v_file_provenance, v_git_commit_context, v_tool_errors, v_session_cost.";

// ---------------------------------------------------------------------------
// Transport builders
// ---------------------------------------------------------------------------

/// Build the StreamableHttpService for nesting into an axum Router.
pub fn build_streamable_http_service(
    state: SharedState,
) -> rmcp::transport::streamable_http_server::StreamableHttpService<McpService> {
    rmcp::transport::streamable_http_server::StreamableHttpService::new(
        move || Ok(McpService::new(state.clone())),
        rmcp::transport::streamable_http_server::session::local::LocalSessionManager::default()
            .into(),
        Default::default(),
    )
}
