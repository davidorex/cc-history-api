//! claude-history CLI binary.
//!
//! Provides the `claude-history` command with 21 subcommands for syncing JSONL
//! session files into a local SQLite database, starting the HTTP/UDS daemon,
//! searching message content, browsing sessions, querying messages, viewing
//! usage statistics, exporting sessions, checking Claude Code versions,
//! inspecting schema drift, interacting with the artifact layer, and running
//! canned SQL queries with named parameter binding.
//!
//! Usage:
//!   claude-history serve [--port 7424] [--socket /tmp/claude-history.sock]
//!   claude-history sync [--projects-dir <path>] [--db-path <path>]
//!   claude-history search <query> [--limit N] [--json]
//!   claude-history sessions [--project <path>] [--after <date>] [--before <date>] [--limit N] [--json]
//!   claude-history query [--session-id <id>] [--type <type>] [--model <m>] [--tool <t>] [--limit N]
//!   claude-history stats [--session-id <id>] [--json]
//!   claude-history export <session-id> [--format json|markdown|csv]
//!   claude-history version-check [--json]
//!   claude-history schema-drift [--record-type <type>] [--limit N] [--json]
//!   claude-history record-type-drift [--type-name <name>] [--version <v>] [--since <date>] [--limit N] [--json]
//!   claude-history files [--session-id <id>] [--path <substr>] [--limit N] [--json]
//!   claude-history file-history <path> [--session-id <id>] [--limit N] [--json]
//!   claude-history reconstruct <path> --session-id <id> [--at <uuid>]
//!   claude-history git-log [--session-id <id>] [--type <type>] [--limit N] [--json]
//!   claude-history artifacts <session-id> [--json]
//!   claude-history queries list [--json] [--queries-dir <path>]
//!   claude-history queries show <name> [--queries-dir <path>]
//!   claude-history queries run <name> [--param key=value]... [--json] [--queries-dir <path>]
//!   claude-history mcp-stdio
//!
//! All subcommands share a global --db-path option. Logs go to stderr,
//! structured output to stdout (PAT-020).

use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

mod api;
pub mod daemon_client;
pub mod events;
mod export;
pub mod mcp;
mod output;
mod serve;
mod state;
mod watcher;

use daemon_client::{ConnectionMode, detect_connection_mode, resolve_socket_path};

#[derive(Parser)]
#[command(name = "claude-history", about = "Claude Code session history API", after_long_help = MAIN_HELP)]
struct Cli {
    /// Path to the database file.
    /// Defaults to $CLAUDE_HISTORY_DB_PATH or ~/.claude/.claude-history.db
    #[arg(long, global = true)]
    db_path: Option<PathBuf>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Start the HTTP API daemon (TCP + Unix domain socket)
    Serve {
        /// TCP port to listen on. Defaults to 7424.
        #[arg(long, default_value = "7424")]
        port: u16,
        /// Unix domain socket path. Defaults to $CLAUDE_HISTORY_SOCKET or /tmp/claude-history.sock.
        #[arg(long)]
        socket: Option<PathBuf>,
        /// Path to Claude Code projects directory for live file watching.
        /// Defaults to $CLAUDE_PROJECTS_DIR or ~/.claude/projects/
        #[arg(long)]
        projects_dir: Option<PathBuf>,
    },
    /// Sync JSONL session files into the database
    Sync {
        /// Path to Claude Code projects directory.
        /// Defaults to ~/.claude/projects/
        #[arg(long)]
        projects_dir: Option<PathBuf>,
    },
    /// Search message content using full-text search
    #[command(after_long_help = SEARCH_HELP)]
    Search {
        /// Search query (FTS5 phrase matching)
        query: String,
        /// Maximum results to return
        #[arg(long, default_value = "20")]
        limit: usize,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// List sessions with optional filters
    #[command(after_long_help = SESSIONS_HELP)]
    Sessions {
        /// Filter by project path (substring match)
        #[arg(long)]
        project: Option<String>,
        /// Show sessions after this date (YYYY-MM-DD or ISO8601)
        #[arg(long)]
        after: Option<String>,
        /// Show sessions before this date (YYYY-MM-DD or ISO8601)
        #[arg(long)]
        before: Option<String>,
        /// (C2.4) Restrict to sessions by plan-bearing-message presence
        /// (`plan_content IS NOT NULL`). Tri-state: omit the flag for no
        /// filter, pass `--has-plan` or `--has-plan=true` for sessions
        /// WITH at least one plan, and `--has-plan=false` for sessions
        /// WITHOUT any plans. There is no separate `--no-has-plan` flag.
        #[arg(long, action = clap::ArgAction::Set, num_args = 0..=1, default_missing_value = "true", require_equals = false)]
        has_plan: Option<bool>,
        /// Maximum sessions to return
        #[arg(long, default_value = "50")]
        limit: usize,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// Query messages with filters (always outputs JSON)
    Query {
        /// Filter by session ID
        #[arg(long)]
        session_id: Option<String>,
        /// Filter by message type (user, assistant)
        #[arg(long = "type")]
        message_type: Option<String>,
        /// Filter by model name
        #[arg(long)]
        model: Option<String>,
        /// Filter by tool name used
        #[arg(long)]
        tool: Option<String>,
        /// Show messages after this date
        #[arg(long)]
        after: Option<String>,
        /// Show messages before this date
        #[arg(long)]
        before: Option<String>,
        /// Maximum results
        #[arg(long, default_value = "100")]
        limit: usize,
    },
    /// Show token usage, tool frequency, and model breakdown statistics
    Stats {
        /// Filter stats to a specific session
        #[arg(long)]
        session_id: Option<String>,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// Export a complete session conversation
    #[command(after_long_help = EXPORT_HELP)]
    Export {
        /// Session ID to export
        session_id: String,
        /// Output format: json, markdown, csv
        #[arg(long, default_value = "json")]
        format: String,
    },
    /// Show Claude Code version history detected from ingested data
    VersionCheck {
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// Show schema drift events (unknown fields captured during ingestion)
    SchemaDrift {
        /// Filter by record type
        #[arg(long)]
        record_type: Option<String>,
        /// Maximum entries to show
        #[arg(long, default_value = "50")]
        limit: usize,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// Show record-type drift events (unknown JSONL discriminators)
    ///
    /// Companion to schema-drift. Where schema-drift surfaces unknown fields on
    /// a known record type, this surfaces records whose top-level `type` value
    /// did not match any of the seven modeled JSONLRecord variants and was
    /// captured to record_type_drift_log via the JSONLRecord::Unknown path
    /// introduced in B1.1.
    RecordTypeDrift {
        /// Filter by type_name (substring match)
        #[arg(long)]
        type_name: Option<String>,
        /// Filter by exact Claude Code version
        #[arg(long)]
        version: Option<String>,
        /// Lower bound on last_seen_at (ISO-8601 text, e.g. 2026-04-01)
        #[arg(long)]
        since: Option<String>,
        /// Maximum entries to show
        #[arg(long, default_value = "50")]
        limit: usize,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// List files touched by Claude Code across sessions
    #[command(after_long_help = FILES_HELP)]
    Files {
        /// Filter by session ID
        #[arg(long)]
        session_id: Option<String>,
        /// Filter by path substring
        #[arg(long)]
        path: Option<String>,
        /// Filter by project path substring
        #[arg(long)]
        project: Option<String>,
        /// Maximum results
        #[arg(long, default_value = "100")]
        limit: usize,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// Show chronological operations on a file
    #[command(after_long_help = FILE_HISTORY_HELP)]
    FileHistory {
        /// File path to show history for
        path: String,
        /// Filter by session ID
        #[arg(long)]
        session_id: Option<String>,
        /// Filter by project path substring
        #[arg(long)]
        project: Option<String>,
        /// Maximum operations to show
        #[arg(long, default_value = "50")]
        limit: usize,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// Reconstruct file content at a point in time
    #[command(after_long_help = RECONSTRUCT_HELP)]
    Reconstruct {
        /// File path to reconstruct
        path: String,
        /// Session ID (required -- reconstruction is per-session)
        #[arg(long)]
        session_id: String,
        /// Stop reconstruction at this message UUID
        #[arg(long)]
        at: Option<String>,
    },
    /// Show git operations extracted from Bash tool calls
    #[command(after_long_help = GIT_LOG_HELP)]
    GitLog {
        /// Filter by session ID
        #[arg(long)]
        session_id: Option<String>,
        /// Filter by git operation type (commit, push, checkout, etc.)
        #[arg(long = "type")]
        operation_type: Option<String>,
        /// Maximum operations to show
        #[arg(long, default_value = "50")]
        limit: usize,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// Show combined file and git artifacts for a session
    Artifacts {
        /// Session ID to show artifacts for
        session_id: String,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// List or show attachment rows from the attachments table (C1.4)
    Attachments {
        #[command(subcommand)]
        action: AttachmentsAction,
    },
    /// List or show user-typed plan markdown (C2.4)
    Plans {
        #[command(subcommand)]
        action: PlansAction,
    },
    /// List hook_executions rows (C1.4)
    HookExecutions {
        /// Filter by exact tool_use_id
        #[arg(long)]
        tool_use_id: Option<String>,
        /// Filter by exact hook_event (PreToolUse, PostToolUse, UserPromptSubmit, Stop)
        #[arg(long)]
        hook_event: Option<String>,
        /// Filter by exact exit_code
        #[arg(long)]
        exit_code: Option<i64>,
        /// Maximum rows to return
        #[arg(long, default_value = "50")]
        limit: usize,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// Manage and run canned SQL queries
    #[command(after_long_help = QUERIES_SCHEMA_HELP)]
    Queries {
        #[command(subcommand)]
        action: QueriesAction,
    },
    /// Run as MCP server over stdin/stdout (for Claude Desktop integration)
    #[command(after_long_help = MCP_STDIO_HELP)]
    McpStdio,
    /// Print MCP client configuration snippets for connecting to claude-history
    McpConfig {
        /// Which client to generate config for
        #[arg(value_enum, default_value = "all")]
        client: McpClient,
    },
}

#[derive(Clone, clap::ValueEnum)]
enum McpClient {
    /// Show all client configurations
    All,
    /// Claude Code (.mcp.json or `claude mcp add`)
    ClaudeCode,
    /// Claude Desktop (claude_desktop_config.json)
    ClaudeDesktop,
    /// Cursor (.cursor/mcp.json)
    Cursor,
    /// VS Code / GitHub Copilot (.vscode/mcp.json)
    Vscode,
    /// Windsurf / Codeium (~/.codeium/windsurf/mcp_config.json)
    Windsurf,
    /// Zed (~/.config/zed/settings.json)
    Zed,
}

#[derive(Subcommand)]
enum AttachmentsAction {
    /// List attachment rows with optional filters
    List {
        /// Filter by project path (substring match)
        #[arg(long)]
        project: Option<String>,
        /// Filter by exact attachment inner_type (e.g. hook_success)
        #[arg(long)]
        inner_type: Option<String>,
        /// Show attachments after this timestamp (ISO-8601)
        #[arg(long)]
        since: Option<String>,
        /// Maximum rows to return
        #[arg(long, default_value = "50")]
        limit: usize,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// Show full envelope + body_json for a single attachment by uuid
    Show {
        /// Attachment UUID
        uuid: String,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum PlansAction {
    /// List plan-bearing messages with optional filters
    List {
        /// Filter by project path (substring match)
        #[arg(long)]
        project: Option<String>,
        /// Show plans after this timestamp (ISO-8601)
        #[arg(long)]
        since: Option<String>,
        /// Maximum rows to return
        #[arg(long, default_value = "50")]
        limit: usize,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// Show full plan markdown for every plan-bearing message in a session
    Show {
        /// Session ID
        session_id: String,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum QueriesAction {
    /// List all available canned queries
    List {
        /// Output as JSON
        #[arg(long)]
        json: bool,
        /// Path to queries directory
        #[arg(long)]
        queries_dir: Option<PathBuf>,
    },
    /// Show SQL and metadata for a specific query
    Show {
        /// Query name (filename without .sql extension)
        name: String,
        /// Path to queries directory
        #[arg(long)]
        queries_dir: Option<PathBuf>,
    },
    /// Execute a canned query with parameter binding
    Run {
        /// Query name (filename without .sql extension)
        name: String,
        /// Parameters as key=value pairs (repeatable)
        #[arg(long = "param", value_parser = parse_key_val)]
        params: Vec<(String, String)>,
        /// Output as JSON
        #[arg(long)]
        json: bool,
        /// Path to queries directory
        #[arg(long)]
        queries_dir: Option<PathBuf>,
    },
}

fn parse_key_val(s: &str) -> Result<(String, String), String> {
    let pos = s
        .find('=')
        .ok_or_else(|| format!("invalid KEY=value: no `=` found in `{s}`"))?;
    Ok((s[..pos].to_string(), s[pos + 1..].to_string()))
}

/// Schema reference for `queries --help`. Gives LLMs and humans the information
/// needed to author new canned queries without reading migration files.
const SEARCH_HELP: &str = r#"FTS5 QUERY SYNTAX:

  Uses SQLite FTS5 full-text search. Supports:
    AND, OR, NOT     Boolean operators (implicit AND between terms)
    "quoted phrase"   Exact phrase match
    prefix*           Prefix matching (e.g. refact* matches refactor, refactoring)
    -term             Exclude term (same as NOT)

  Hyphenated terms must be quoted: '"gsd-tools"' not gsd-tools

EXAMPLES:

  claude-history search 'error handling'             # Both words (implicit AND)
  claude-history search '"error handling"'           # Exact phrase
  claude-history search 'tokio AND spawn'            # Explicit AND
  claude-history search 'refactor OR restructure'    # Either word
  claude-history search 'database NOT migration'     # Exclude
  claude-history search 'deploy*'                    # Prefix match
  claude-history search '"gsd-tools"'                # Hyphenated term
  claude-history search 'error handling' --json      # Machine-readable output

  Results are ranked by BM25 relevance (best matches first).
  Searches across all message content: user prompts, assistant responses,
  tool results, and thinking blocks."#;

const SESSIONS_HELP: &str = r#"EXAMPLES:

  claude-history sessions                                # Recent 50 sessions
  claude-history sessions --project myproject            # Filter by project
  claude-history sessions --after 2026-03-01             # Since a date
  claude-history sessions --after 2026-03-01 --before 2026-03-08
  claude-history sessions --project smak --limit 5 --json

  Project filter is substring match: --project smak matches
  /Users/david/Projects/smak, /tmp/smak-test, etc.

  Session IDs from this output can be used with --session-id in other commands
  to drill into a specific session's messages, files, git ops, or artifacts."#;

const FILES_HELP: &str = r#"EXAMPLES:

  claude-history files --project myproject               # All files in a project
  claude-history files --path main.rs                    # Find main.rs across all projects
  claude-history files --path Cargo.toml --project smak  # Scoped to one project
  claude-history files --session-id abc123               # Files in one session
  claude-history files --path .rs --limit 20 --json      # Rust files, JSON output

  Path and project filters are substring matches (LIKE '%term%').
  Results include operation_count showing how many read/write/edit ops
  touched each file. Use file-history to drill into specific operations.

WORKFLOW:

  1. Find files:    claude-history files --project myproject --path config
  2. See operations: claude-history file-history config.rs --project myproject
  3. Reconstruct:   claude-history reconstruct /full/path/config.rs --session-id <id>"#;

const FILE_HISTORY_HELP: &str = r#"EXAMPLES:

  claude-history file-history main.rs                        # Partial path match
  claude-history file-history main.rs --project myproject    # Scoped to project
  claude-history file-history tools.rs --session-id abc123   # Scoped to session
  claude-history file-history .rs --limit 100 --json         # All Rust file ops

  The path argument is a substring match (LIKE '%path%'), not an exact path.
  This means 'main.rs' matches /src/main.rs, /tests/main.rs, etc.
  Use --project to narrow results to a specific project.

  Operations returned: write (full file creation), edit (string replacement),
  read (file was read), bash_cp, bash_mv, bash_rm, bash_mkdir, bash_touch.

  Each operation includes session_id, timestamp, content (for writes),
  old_content (for edits), and the message_uuid that triggered it."#;

const GIT_LOG_HELP: &str = r#"EXAMPLES:

  claude-history git-log                                     # Recent git ops
  claude-history git-log --type commit                       # Only commits
  claude-history git-log --type push                         # Only pushes
  claude-history git-log --session-id abc123                  # One session
  claude-history git-log --type commit --limit 10 --json     # JSON output

  Operation types extracted from Bash commands: commit, push, pull, checkout,
  branch, merge, rebase, stash, reset, diff, log, show, grep, tag, fetch, add.

  Commit operations include the commit message and branch when detected."#;

const RECONSTRUCT_HELP: &str = r#"Replays Write and Edit operations in timestamp order to reproduce file
content at any point during a session.

EXAMPLES:

  claude-history reconstruct /src/main.rs --session-id abc123
  claude-history reconstruct /src/main.rs --session-id abc123 --at <msg-uuid>

  The --at flag stops reconstruction at a specific message, giving you the
  file state at that point in the conversation. Without it, you get the final
  state after all operations in the session.

  The path must be the full absolute path as stored in the database.
  Use `claude-history files --session-id <id>` to discover exact paths."#;

const EXPORT_HELP: &str = r#"EXAMPLES:

  claude-history export abc123                      # JSON (default)
  claude-history export abc123 --format markdown    # Human-readable
  claude-history export abc123 --format csv         # Spreadsheet-friendly

  Exports the complete conversation: all messages with their content blocks,
  tool calls, results, and metadata. Useful for archival, analysis, or
  feeding into other tools.

  Find session IDs with: claude-history sessions --project myproject"#;

const MCP_STDIO_HELP: &str = r#"Runs claude-history as an MCP (Model Context Protocol) server communicating
over stdin/stdout. This is how Claude Desktop and Claude Code connect.

15 TOOLS AVAILABLE:

  Discovery:
    list_sessions       Browse sessions by project, date range
    list_files          Find files by path substring and/or project
    list_queries        Discover available canned SQL queries
    list_bookmarks      Browse bookmarks from ClaudeHistoryBrowser
    list_attachments    List attachment rows with project/inner_type/since filters
    get_stats           Token usage, tool frequency, model breakdown

  Deep queries:
    search_messages     FTS5 full-text search across all message + attachment content
    query_messages      Filter messages by session, type, model, tool, date
    file_history        Chronological operations on a file (substring path match)
    git_log             Git operations extracted from Bash tool calls
    get_bookmark        Retrieve a bookmark by ID or assistant message UUID
    search_bookmarks    Search bookmarks by label or tag text
    get_hook_executions List hook_executions rows by tool_use_id/hook_event/exit_code

  Power tools:
    execute_sql         Read-only SQL passthrough (any SELECT query)
    run_query           Execute named canned queries with parameter binding

CONFIGURATION:

  For Claude Code (.mcp.json):
    { "mcpServers": { "claude-history": { "command": "claude-history", "args": ["mcp-stdio"] } } }

  For Claude Desktop (claude_desktop_config.json):
    { "mcpServers": { "claude-history": { "command": "/path/to/claude-history", "args": ["mcp-stdio"] } } }

  Or run: claude-history mcp-config"#;

const MAIN_HELP: &str = r#"WHAT THIS IS:

  claude-history ingests Claude Code JSONL session files into a normalized
  SQLite database and serves them via CLI, HTTP API, and MCP tools.

  It captures: conversations, tool calls, file operations (read/write/edit),
  git operations, token usage, model info, and schema drift events.

  Data source: ~/.claude/projects/<encoded-dir>/<session-id>/...jsonl

DAEMON:

  The daemon (`claude-history serve`) runs the file watcher for live ingestion
  and serves the HTTP/UDS API. Without it, no new sessions are ingested.

    claude-history serve &            # Start daemon (background)
    pgrep -f 'claude-history serve'   # Check if running

  On startup the daemon runs sync_all to catch up any sessions missed while
  it was down. The file watcher then handles live ingestion going forward.

DISCOVERY WORKFLOW:

  1. Find sessions:
       claude-history sessions --project myproject --limit 10

  2. Search across all conversations:
       claude-history search "error handling"
       claude-history search '"exact phrase"'           # FTS5 phrase match
       claude-history search 'tokio AND spawn'          # Boolean operators
       claude-history search 'refactor*'                # Prefix match

  3. Find files touched in a project:
       claude-history files --project myproject --path main.rs

  4. See what happened to a file:
       claude-history file-history main.rs --project myproject

  5. Reconstruct file content at a point in time:
       claude-history reconstruct /path/to/file.rs --session-id <id>

  6. See git operations:
       claude-history git-log --type commit --limit 20

  7. Run custom SQL:
       claude-history queries run recent-sessions --param limit=5

  8. Export a session:
       claude-history export <session-id> --format markdown

MCP TOOLS (15):

  The same data is available via MCP for LLM tool use. Connect via:
    - Streamable HTTP: http://127.0.0.1:7424/mcp (when daemon is running)
    - Stdio: claude-history mcp-stdio (for Claude Desktop / Claude Code)

  Run `claude-history mcp-config` for client configuration snippets.

OUTPUT:

  All subcommands default to human-readable table output. Add --json for
  machine-readable JSON. Logs go to stderr, data to stdout.
  Timestamps display in local timezone (stored as UTC internally)."#;

const QUERIES_SCHEMA_HELP: &str = r#"DATABASE SCHEMA:

  Tables:
    sessions          session_id*, project_path, first_seen_at, last_seen_at, version, slug, git_branch
    messages          uuid*, session_id→sessions, type, timestamp, parent_uuid, model, stop_reason, is_compact_summary, agent_id, subtype
    message_content   id*, message_uuid→messages, block_index, block_type(text|thinking|tool_use|tool_result|plan_content), text_content, tool_name, tool_input
                      Note: block_type='plan_content' rows are FTS-only synthetic projections of messages.plan_content (migration 011)
                      and use sentinel block_index = -1. Ad-hoc SQL targeting real message blocks should add
                      `WHERE block_type != 'plan_content'` (or `WHERE block_index >= 0`) to exclude them.
    token_usage       message_uuid*→messages, input_tokens, output_tokens, cache_creation_input_tokens, cache_read_input_tokens
    tool_executions   id*, message_uuid→messages, tool_use_id, tool_name, input_json, result_content, is_error
    files             id*, session_id→sessions, file_path, first_seen, last_modified, operation_count
    file_operations   id*, session_id→sessions, file_path, operation_type(write|edit|read|bash_*), content, old_content, command, message_uuid→messages, timestamp, result_summary, is_error
    git_operations    id*, session_id→sessions, operation_type(commit|push|checkout|...), command, commit_message, branch, message_uuid→messages, timestamp, result_summary, is_error
    projects          project_path*, display_name, first_seen, last_seen, session_count
    agents            agent_id*, session_id→sessions, first_seen_at, last_seen_at
    version_history   version*, first_seen_at, last_seen_at, session_count, new_fields_count
    schema_drift_log  id*, field_name, record_type, version, sample_value, first_seen_at, occurrence_count, last_seen_at

  Views (prebuilt cross-domain queries):
    v_file_token_cost             Per-file token cost by project, file, operation_type
    v_file_conversation_context   Assistant reasoning within 60s before file write/edit
    v_project_summary             Project stats: sessions, messages, tokens, file/git ops
    v_file_provenance             Complete file operation history across sessions
    v_git_commit_context          Commit messages with assistant reasoning within 120s
    v_tool_errors                 Tool error patterns with session/project context
    v_session_cost                Session cost: tokens, cache, file ops, git ops

  FTS (full-text search — use MATCH, not LIKE):
    fts_message_content           FTS5 on message_content.text_content
    fts_file_operations           FTS5 on file_operations content, old_content, command

    Query:   WHERE fts_message_content MATCH :query
    Syntax:  AND, OR, NOT, "phrase match", prefix*
    Ranking: ORDER BY bm25(fts_message_content)  — lower = better match
    Join:    FROM fts_message_content fts JOIN message_content mc ON fts.rowid = mc.rowid

  Key relationships:
    sessions.session_id  →  messages, files, file_operations, git_operations, agents
    messages.uuid        →  message_content, token_usage, tool_executions
    messages.uuid        →  file_operations.message_uuid, git_operations.message_uuid

  Timestamps are stored as UTC ISO 8601 strings. CLI display converts to local time.

AUTHORING QUERIES:

  Queries are .sql files in ~/.claude/claude-history/queries/ (override with $CLAUDE_HISTORY_QUERIES).
  Use :param_name for parameters. Optional .toml sidecar for metadata.

  Param types control SQLite binding: integer/real bind as numbers, text as strings.
  Without type hints, use CAST(:param AS INTEGER) for numeric comparisons.

  Example — file-edits-by-project.sql:

    SELECT fo.file_path, fo.operation_type, fo.timestamp
    FROM file_operations fo
    JOIN sessions s ON fo.session_id = s.session_id
    WHERE s.project_path LIKE '%' || :project || '%'
      AND fo.operation_type IN ('write', 'edit')
    ORDER BY fo.timestamp DESC
    LIMIT :limit

  Example — file-edits-by-project.toml:

    description = "Recent file writes and edits for a project"

    [[params]]
    name = "project"
    description = "Project path substring to match"

    [[params]]
    name = "limit"
    description = "Maximum results to return"
    default = "20"
    type = "integer""#;

/// Resolve the database file path.
///
/// Priority:
/// 1. Explicit CLI argument (if provided)
/// 2. CLAUDE_HISTORY_DB_PATH environment variable (if set and non-empty)
/// 3. $HOME/.claude/.claude-history.db (fallback default)
fn resolve_db_path(cli_arg: Option<PathBuf>) -> Option<PathBuf> {
    if let Some(p) = cli_arg {
        return Some(p);
    }

    if let Ok(p) = std::env::var("CLAUDE_HISTORY_DB_PATH") {
        if !p.is_empty() {
            return Some(PathBuf::from(p));
        }
    }

    std::env::var("HOME").ok().map(|home| {
        PathBuf::from(home)
            .join(".claude")
            .join(".claude-history.db")
    })
}

/// Resolve the projects directory path.
///
/// Priority:
/// 1. Explicit CLI argument (if provided)
/// 2. CLAUDE_PROJECTS_DIR environment variable (if set and non-empty)
/// 3. $HOME/.claude/projects/ (fallback default)
fn resolve_projects_dir(cli_arg: Option<PathBuf>) -> Option<PathBuf> {
    if let Some(p) = cli_arg {
        return Some(p);
    }

    if let Ok(p) = std::env::var("CLAUDE_PROJECTS_DIR") {
        if !p.is_empty() {
            return Some(PathBuf::from(p));
        }
    }

    std::env::var("HOME").ok().map(|home| {
        PathBuf::from(home).join(".claude").join("projects")
    })
}

/// Resolve the ConnectionMode for read-only subcommands.
///
/// Probes the daemon socket (resolved via CLI arg > env var > default). If the
/// daemon is running and healthy, returns `ConnectionMode::Daemon`. Otherwise
/// falls back to `ConnectionMode::Direct` with an open DB connection.
///
/// This is called once at startup so all read subcommands share the same mode.
async fn resolve_connection_mode(db_path_arg: Option<PathBuf>) -> Result<ConnectionMode, String> {
    let socket_path = resolve_socket_path(None);

    let db_path = resolve_db_path(db_path_arg).ok_or_else(|| {
        "Could not determine database path. Set CLAUDE_HISTORY_DB_PATH or HOME environment variable, or pass --db-path.".to_string()
    })?;

    tracing::debug!(
        socket = %socket_path.display(),
        db = %db_path.display(),
        "Detecting connection mode"
    );

    detect_connection_mode(&socket_path, db_path).await
}

#[tokio::main]
async fn main() -> ExitCode {
    // Initialize tracing with env-filter; defaults to INFO if RUST_LOG is unset.
    // Logs go to stderr (tracing-subscriber default), leaving stdout clean for
    // structured output.
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();

    match cli.command {
        // Serve and Sync bypass ConnectionMode — Serve IS the daemon, Sync is a
        // write operation that must always open the DB directly.
        Commands::Serve { port, socket, projects_dir } => run_serve(cli.db_path, port, socket, projects_dir).await,
        Commands::Sync { projects_dir } => run_sync(projects_dir, cli.db_path).await,

        // Queries subcommand: list/show are filesystem-only, run needs DB.
        Commands::Queries { action } => run_queries(action, cli.db_path).await,

        // MCP stdio: opens DB directly and serves MCP protocol on stdin/stdout.
        Commands::McpStdio => run_mcp_stdio(cli.db_path).await,

        // MCP config: print client configuration snippets (no DB needed).
        Commands::McpConfig { client } => {
            print_mcp_config(client);
            ExitCode::SUCCESS
        }

        // All read-only subcommands: detect daemon vs direct DB once at startup.
        read_cmd => {
            let mode = match resolve_connection_mode(cli.db_path).await {
                Ok(m) => m,
                Err(msg) => {
                    eprintln!("Error: {}", msg);
                    return ExitCode::FAILURE;
                }
            };

            match read_cmd {
                Commands::Search { query, limit, json } => {
                    run_search(mode, query, limit, json).await
                }
                Commands::Sessions {
                    project,
                    after,
                    before,
                    has_plan,
                    limit,
                    json,
                } => run_sessions(mode, project, after, before, has_plan, limit, json).await,
                Commands::Query {
                    session_id,
                    message_type,
                    model,
                    tool,
                    after,
                    before,
                    limit,
                } => run_query(mode, session_id, message_type, model, tool, after, before, limit).await,
                Commands::Stats { session_id, json } => run_stats(mode, session_id, json).await,
                Commands::Export { session_id, format } => {
                    run_export(mode, session_id, format).await
                }
                Commands::VersionCheck { json } => run_version_check(mode, json).await,
                Commands::SchemaDrift {
                    record_type,
                    limit,
                    json,
                } => run_schema_drift(mode, record_type, limit, json).await,
                Commands::RecordTypeDrift {
                    type_name,
                    version,
                    since,
                    limit,
                    json,
                } => run_record_type_drift(mode, type_name, version, since, limit, json).await,
                Commands::Files {
                    session_id,
                    path,
                    project,
                    limit,
                    json,
                } => run_files(mode, session_id, path, project, limit, json).await,
                Commands::FileHistory {
                    path,
                    session_id,
                    project,
                    limit,
                    json,
                } => run_file_history(mode, path, session_id, project, limit, json).await,
                Commands::Reconstruct {
                    path,
                    session_id,
                    at,
                } => run_reconstruct(mode, path, session_id, at).await,
                Commands::GitLog {
                    session_id,
                    operation_type,
                    limit,
                    json,
                } => run_git_log(mode, session_id, operation_type, limit, json).await,
                Commands::Artifacts { session_id, json } => {
                    run_artifacts(mode, session_id, json).await
                }
                Commands::Attachments { action } => run_attachments(mode, action).await,
                Commands::Plans { action } => run_plans(mode, action).await,
                Commands::HookExecutions {
                    tool_use_id,
                    hook_event,
                    exit_code,
                    limit,
                    json,
                } => {
                    run_hook_executions(mode, tool_use_id, hook_event, exit_code, limit, json).await
                }
                // Serve, Sync, Queries, McpStdio, and McpConfig already handled above.
                Commands::Serve { .. } | Commands::Sync { .. } | Commands::Queries { .. } | Commands::McpStdio | Commands::McpConfig { .. } => unreachable!(),
            }
        }
    }
}

async fn run_sync(projects_dir_arg: Option<PathBuf>, db_path_arg: Option<PathBuf>) -> ExitCode {
    // Resolve paths
    let db_path = match resolve_db_path(db_path_arg) {
        Some(p) => p,
        None => {
            eprintln!("Error: Could not determine database path. Set CLAUDE_HISTORY_DB_PATH or HOME environment variable, or pass --db-path.");
            return ExitCode::FAILURE;
        }
    };

    let projects_dir = match resolve_projects_dir(projects_dir_arg) {
        Some(p) => p,
        None => {
            eprintln!("Error: Could not determine projects directory. Set HOME environment variable, or pass --projects-dir.");
            return ExitCode::FAILURE;
        }
    };

    // Validate projects directory exists
    if !projects_dir.exists() {
        eprintln!(
            "Error: Projects directory does not exist: {}\n\
             This directory should contain Claude Code session JSONL files.\n\
             If Claude Code is installed, it is typically at ~/.claude/projects/\n\
             You can specify a different path with --projects-dir.",
            projects_dir.display()
        );
        return ExitCode::FAILURE;
    }

    if !projects_dir.is_dir() {
        eprintln!(
            "Error: Projects path is not a directory: {}",
            projects_dir.display()
        );
        return ExitCode::FAILURE;
    }

    tracing::info!(
        db_path = %db_path.display(),
        projects_dir = %projects_dir.display(),
        "Starting sync"
    );

    // Initialize database (creates parent dirs, opens connection, runs migrations)
    let conn = match claude_history_store::db::init_db(&db_path).await {
        Ok(conn) => conn,
        Err(e) => {
            eprintln!("Error: Failed to initialize database at {}: {}", db_path.display(), e);
            return ExitCode::FAILURE;
        }
    };

    // Run sync
    let result = match claude_history_store::sync::sync_all(&conn, &projects_dir).await {
        Ok(r) => r,
        Err(e) => {
            eprintln!("Error: Sync failed: {}", e);
            return ExitCode::FAILURE;
        }
    };

    // Print human-readable summary to stdout
    println!("Sync complete:");
    println!("  Files discovered: {}", result.files_discovered);
    println!("  Files synced:     {}", result.files_synced);
    println!("  Files skipped:    {} (no new data)", result.files_skipped);
    println!("  Files errored:    {}", result.files_errored);
    println!("  Records ingested: {}", result.total_records);
    println!("  Warnings:         {}", result.total_warnings);
    println!("  Drift fields:     {}", result.total_overflow_fields);

    if result.files_errored > 0 {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

/// Start the HTTP API daemon on TCP and Unix domain socket.
///
/// [CLI-01] Opens the database, builds SharedState, resolves the socket path
/// (from --socket arg, then CLAUDE_HISTORY_SOCKET env, then default), resolves
/// the projects directory for live file watching (from --projects-dir arg,
/// then CLAUDE_PROJECTS_DIR env, then ~/.claude/projects/), and calls
/// serve::run_server which blocks until shutdown signal (SIGINT/SIGTERM).
async fn run_serve(
    db_path_arg: Option<PathBuf>,
    port: u16,
    socket_arg: Option<PathBuf>,
    projects_dir_arg: Option<PathBuf>,
) -> ExitCode {
    // Resolve database path and open connection
    let db_path = match resolve_db_path(db_path_arg) {
        Some(p) => p,
        None => {
            eprintln!("Error: Could not determine database path. Set CLAUDE_HISTORY_DB_PATH or HOME environment variable, or pass --db-path.");
            return ExitCode::FAILURE;
        }
    };

    if !db_path.exists() {
        eprintln!(
            "Error: Database file does not exist: {}\n\
             Run `claude-history sync` first to create the database.",
            db_path.display()
        );
        return ExitCode::FAILURE;
    }

    let conn = match claude_history_store::db::init_db(&db_path).await {
        Ok(c) => c,
        Err(e) => {
            eprintln!(
                "Error: Failed to open database at {}: {}",
                db_path.display(),
                e
            );
            return ExitCode::FAILURE;
        }
    };

    // Build shared application state.
    // The broadcast channel capacity of 1024 provides ~100 seconds of buffer at
    // 10 events/second. The initial receiver (_rx) is immediately dropped — it
    // exists only to satisfy channel construction. Each SSE client handler creates
    // its own Receiver via event_tx.subscribe().
    let (event_tx, _rx) = tokio::sync::broadcast::channel::<crate::events::SseEvent>(1024);

    let state = Arc::new(state::AppState {
        conn,
        version: env!("CARGO_PKG_VERSION").to_string(),
        db_path: db_path.clone(),
        event_tx,
    });

    // Resolve socket path: --socket arg > $CLAUDE_HISTORY_SOCKET > default
    let socket_path = socket_arg
        .or_else(|| {
            std::env::var("CLAUDE_HISTORY_SOCKET")
                .ok()
                .filter(|s| !s.is_empty())
                .map(PathBuf::from)
        })
        .unwrap_or_else(|| PathBuf::from("/tmp/claude-history.sock"));

    // Resolve projects directory: --projects-dir arg > CLAUDE_PROJECTS_DIR env
    // > $HOME/.claude/projects/ default. Uses the same resolution function as
    // the Sync subcommand for consistency (PAT-020 pattern).
    // If the directory does not exist, log a warning but proceed — Claude Code
    // may not have created sessions yet, and the watcher will handle the missing
    // directory gracefully (notify returns an error, which serve.rs logs and
    // continues without live ingestion).
    let projects_dir = match resolve_projects_dir(projects_dir_arg) {
        Some(p) => p,
        None => {
            eprintln!("Error: Could not determine projects directory. Set CLAUDE_PROJECTS_DIR or HOME environment variable, or pass --projects-dir.");
            return ExitCode::FAILURE;
        }
    };

    if !projects_dir.exists() {
        tracing::warn!(
            path = %projects_dir.display(),
            "Projects directory does not exist — file watcher may fail to start. \
             Claude Code may not have created sessions yet."
        );
    }

    tracing::info!(
        port = port,
        socket = %socket_path.display(),
        db = %db_path.display(),
        projects_dir = %projects_dir.display(),
        version = %env!("CARGO_PKG_VERSION"),
        "Starting claude-history daemon"
    );

    match serve::run_server(state, port, socket_path, projects_dir).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("Error: Server failed: {}", e);
            ExitCode::FAILURE
        }
    }
}

/// Search message content using FTS5 full-text search.
///
/// [CLI-05, CLI-15] Routes through daemon HTTP API when available, otherwise
/// uses direct DB access via store::fts::search_messages. Output formatting
/// is identical regardless of data source.
async fn run_search(
    mode: ConnectionMode,
    query: String,
    limit: usize,
    json: bool,
) -> ExitCode {
    let results = match mode {
        ConnectionMode::Daemon(client) => {
            tracing::info!(
                socket = %client.socket_path().display(),
                "Routing search through daemon"
            );
            match client.search(&query, Some(limit), None).await {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("Error: Daemon search failed: {}", e);
                    return ExitCode::FAILURE;
                }
            }
        }
        ConnectionMode::Direct { conn, .. } => {
            let q = query.clone();
            match conn
                .call(move |conn| {
                    // C1.3: union FTS over message content AND attachment
                    // textual content. Pre-C1.3 callers see the same
                    // SearchResult shape — `source` defaults to Message
                    // for message-content matches.
                    claude_history_store::fts::search_messages_and_attachments(
                        conn, &q, limit, 0,
                    )
                })
                .await
            {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("Error: Search failed: {}", e);
                    return ExitCode::FAILURE;
                }
            }
        }
    };

    eprintln!("{} result(s) for \"{}\"", results.len(), query);

    if json {
        if output::print_json(&results).is_err() {
            return ExitCode::FAILURE;
        }
    } else {
        output::print_search_results(&results);
    }

    ExitCode::SUCCESS
}

/// List sessions with optional project, date, and limit filters.
///
/// [CLI-04, CLI-15] Routes through daemon HTTP API when available, otherwise
/// uses direct DB access via store::query::list_sessions. Output formatting
/// is identical regardless of data source.
async fn run_sessions(
    mode: ConnectionMode,
    project: Option<String>,
    after: Option<String>,
    before: Option<String>,
    has_plan: Option<bool>,
    limit: usize,
    json: bool,
) -> ExitCode {
    let results = match mode {
        ConnectionMode::Daemon(client) => {
            tracing::info!(
                socket = %client.socket_path().display(),
                "Routing sessions through daemon"
            );
            match client
                .sessions(
                    project.as_deref(),
                    after.as_deref(),
                    before.as_deref(),
                    has_plan,
                    Some(limit),
                )
                .await
            {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("Error: Daemon sessions query failed: {}", e);
                    return ExitCode::FAILURE;
                }
            }
        }
        ConnectionMode::Direct { conn, .. } => {
            match conn
                .call(move |conn| {
                    claude_history_store::query::list_sessions(
                        conn,
                        project.as_deref(),
                        after.as_deref(),
                        before.as_deref(),
                        has_plan,
                        limit,
                    )
                })
                .await
            {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("Error: Sessions query failed: {}", e);
                    return ExitCode::FAILURE;
                }
            }
        }
    };

    if json {
        if output::print_json(&results).is_err() {
            return ExitCode::FAILURE;
        }
    } else {
        output::print_sessions_table(&results);
    }

    ExitCode::SUCCESS
}

/// Query messages with filters, always outputting JSON.
///
/// [CLI-03, CLI-15] Routes through daemon HTTP API when available, otherwise
/// uses direct DB access via store::query::query_messages. This subcommand
/// always outputs JSON to stdout (designed for machine consumption per spec).
#[allow(clippy::too_many_arguments)]
async fn run_query(
    mode: ConnectionMode,
    session_id: Option<String>,
    message_type: Option<String>,
    model: Option<String>,
    tool: Option<String>,
    after: Option<String>,
    before: Option<String>,
    limit: usize,
) -> ExitCode {
    let results = match mode {
        ConnectionMode::Daemon(client) => {
            tracing::info!(
                socket = %client.socket_path().display(),
                "Routing query through daemon"
            );
            match client
                .query_messages(
                    session_id.as_deref(),
                    message_type.as_deref(),
                    model.as_deref(),
                    tool.as_deref(),
                    after.as_deref(),
                    before.as_deref(),
                    Some(limit),
                )
                .await
            {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("Error: Daemon query failed: {}", e);
                    return ExitCode::FAILURE;
                }
            }
        }
        ConnectionMode::Direct { conn, .. } => {
            match conn
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
            {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("Error: Query failed: {}", e);
                    return ExitCode::FAILURE;
                }
            }
        }
    };

    if output::print_json(&results).is_err() {
        return ExitCode::FAILURE;
    }

    ExitCode::SUCCESS
}

/// Show token usage, tool frequency, and model breakdown statistics.
///
/// [CLI-06, CLI-15] Routes through daemon HTTP API when available, otherwise
/// uses direct DB access. Runs three queries: token_stats, tool_frequency, and
/// model_breakdown. Output is either three human-readable sections or a combined
/// JSON object. Output format is identical regardless of data source.
async fn run_stats(
    mode: ConnectionMode,
    session_id: Option<String>,
    json: bool,
) -> ExitCode {
    let stats_result = match mode {
        ConnectionMode::Daemon(client) => {
            tracing::info!(
                socket = %client.socket_path().display(),
                "Routing stats through daemon"
            );
            // For daemon mode: if session_id is given, pass group_by=session + session_id.
            // Otherwise, use default (model) grouping.
            let group_by = if session_id.is_some() {
                Some("session")
            } else {
                None
            };
            let token_result = client
                .stats_tokens(group_by, session_id.as_deref())
                .await;
            let tool_result = client.stats_tools().await;
            let model_result = client.stats_models().await;

            match (token_result, tool_result, model_result) {
                (Ok(t), Ok(tl), Ok(m)) => Ok((t, tl, m)),
                (Err(e), _, _) | (_, Err(e), _) | (_, _, Err(e)) => Err(e),
            }
        }
        ConnectionMode::Direct { conn, .. } => {
            let sid = session_id.clone();
            conn.call(move |conn| -> Result<_, tokio_rusqlite::rusqlite::Error> {
                let token_stats = if let Some(ref sid) = sid {
                    claude_history_store::query::token_stats_by_session(conn, Some(sid.as_str()))
                } else {
                    claude_history_store::query::token_stats_by_model(conn)
                }?;

                let tool_stats = claude_history_store::query::tool_frequency(conn)?;

                let model_stats = claude_history_store::query::model_breakdown(conn)?;

                Ok((token_stats, tool_stats, model_stats))
            })
            .await
            .map_err(|e| daemon_client::DaemonError::Connection(
                std::io::Error::new(std::io::ErrorKind::Other, e.to_string())
            ))
        }
    };

    let (token_stats, tool_stats, model_stats) = match stats_result {
        Ok(r) => r,
        Err(e) => {
            eprintln!("Error: Stats query failed: {}", e);
            return ExitCode::FAILURE;
        }
    };

    if json {
        #[derive(serde::Serialize)]
        struct StatsJson<'a> {
            token_usage: &'a [claude_history_store::query::TokenStats],
            tool_frequency: &'a [claude_history_store::query::ToolStats],
            model_breakdown: &'a [claude_history_store::query::ModelStats],
        }
        let combined = StatsJson {
            token_usage: &token_stats,
            tool_frequency: &tool_stats,
            model_breakdown: &model_stats,
        };
        if output::print_json(&combined).is_err() {
            return ExitCode::FAILURE;
        }
    } else {
        output::print_stats(&token_stats, &tool_stats, &model_stats);
    }

    ExitCode::SUCCESS
}

/// Export a complete session conversation in JSON, Markdown, or CSV format.
///
/// [CLI-07, CLI-15] Routes through daemon HTTP API when available, otherwise
/// uses direct DB access. Validates the format argument, then delegates to
/// either the daemon's export endpoint or the local export functions.
/// Output bytes are written to stdout regardless of data source.
async fn run_export(
    mode: ConnectionMode,
    session_id: String,
    format: String,
) -> ExitCode {
    // Validate format before doing any I/O
    let valid_formats = ["json", "markdown", "csv"];
    if !valid_formats.contains(&format.as_str()) {
        eprintln!(
            "Error: Invalid format '{}'. Valid formats: json, markdown, csv",
            format
        );
        return ExitCode::FAILURE;
    }

    let export_result = match mode {
        ConnectionMode::Daemon(client) => {
            tracing::info!(
                socket = %client.socket_path().display(),
                "Routing export through daemon"
            );
            client.export_session(&session_id, Some(format.as_str())).await
        }
        ConnectionMode::Direct { conn, .. } => {
            let fmt = format.clone();
            let sid = session_id.clone();
            conn.call(move |conn| {
                let mut buffer = Vec::new();
                let result: Result<Vec<u8>, Box<dyn std::error::Error + Send + Sync>> =
                    match fmt.as_str() {
                        "json" => export::export_json(conn, &sid, &mut buffer)
                            .map(|()| buffer)
                            .map_err(|e| e.to_string().into()),
                        "markdown" => export::export_markdown(conn, &sid, &mut buffer)
                            .map(|()| buffer)
                            .map_err(|e| e.to_string().into()),
                        "csv" => export::export_csv(conn, &sid, &mut buffer)
                            .map(|()| buffer)
                            .map_err(|e| e.to_string().into()),
                        _ => unreachable!("format validated above"),
                    };
                result.map_err(|e| tokio_rusqlite::rusqlite::Error::ToSqlConversionFailure(e))
            })
            .await
            .map_err(|e| daemon_client::DaemonError::Connection(
                std::io::Error::new(std::io::ErrorKind::Other, e.to_string())
            ))
        }
    };

    match export_result {
        Ok(buffer) => {
            let stdout = std::io::stdout();
            let mut handle = stdout.lock();
            if let Err(e) = handle.write_all(&buffer) {
                eprintln!("Error: Failed to write export output: {}", e);
                return ExitCode::FAILURE;
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("Error: Export failed: {}", e);
            ExitCode::FAILURE
        }
    }
}

/// Show Claude Code version history detected from ingested data.
///
/// [CLI-08, CLI-15, VER-01] Routes through daemon HTTP API when available,
/// otherwise uses direct DB access via store::query::version_history_enhanced.
/// Displays a 5-column table: VERSION, FIRST_SEEN, LAST_SEEN, SESSIONS,
/// NEW_FIELDS. Output format is identical regardless of data source.
async fn run_version_check(mode: ConnectionMode, json: bool) -> ExitCode {
    let results = match mode {
        ConnectionMode::Daemon(client) => {
            tracing::info!(
                socket = %client.socket_path().display(),
                "Routing version-check through daemon"
            );
            match client.version_history().await {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("Error: Daemon version check failed: {}", e);
                    return ExitCode::FAILURE;
                }
            }
        }
        ConnectionMode::Direct { conn, .. } => {
            match conn
                .call(move |conn| claude_history_store::query::version_history_enhanced(conn))
                .await
            {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("Error: Version check failed: {}", e);
                    return ExitCode::FAILURE;
                }
            }
        }
    };

    if results.is_empty() {
        eprintln!("No version data found. Run 'claude-history sync' first.");
        return ExitCode::SUCCESS;
    }

    if json {
        if output::print_json(&results).is_err() {
            return ExitCode::FAILURE;
        }
    } else {
        output::print_version_history(&results);
    }

    ExitCode::SUCCESS
}

/// Show schema drift events grouped by version and record type.
///
/// [CLI-09, CLI-15, VER-03] Routes through daemon HTTP API when available,
/// otherwise uses direct DB access via store::query::drift_by_version.
/// In daemon mode, the daemon applies record_type filtering and limit
/// server-side. In direct mode, filtering and limit are applied in Rust
/// post-retrieval. Output shows grouped format by version and record type
/// with promotion status and occurrence counts.
async fn run_schema_drift(
    mode: ConnectionMode,
    record_type: Option<String>,
    limit: usize,
    json: bool,
) -> ExitCode {
    let results = match mode {
        ConnectionMode::Daemon(client) => {
            tracing::info!(
                socket = %client.socket_path().display(),
                "Routing schema-drift through daemon"
            );
            // Daemon endpoint handles record_type filter and limit server-side.
            match client
                .schema_drift_grouped(record_type.as_deref(), Some(limit))
                .await
            {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("Error: Daemon schema drift query failed: {}", e);
                    return ExitCode::FAILURE;
                }
            }
        }
        ConnectionMode::Direct { conn, .. } => {
            let mut groups = match conn
                .call(move |conn| claude_history_store::query::drift_by_version(conn))
                .await
            {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("Error: Schema drift query failed: {}", e);
                    return ExitCode::FAILURE;
                }
            };

            // Apply record_type filter in Rust if provided
            if let Some(ref rt) = record_type {
                for group in &mut groups {
                    group
                        .record_types
                        .retain(|rt_group| rt_group.record_type.contains(rt.as_str()));
                }
                groups.retain(|g| !g.record_types.is_empty());
            }

            // Apply limit by counting total fields across all groups
            let mut total_fields = 0usize;
            let mut truncated = Vec::new();
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
                truncated.push(claude_history_store::query::VersionDriftGroup {
                    version: group.version,
                    record_types: truncated_rts,
                });
            }

            truncated
        }
    };

    if results.is_empty() {
        eprintln!("No schema drift detected.");
        return ExitCode::SUCCESS;
    }

    if json {
        if output::print_json(&results).is_err() {
            return ExitCode::FAILURE;
        }
    } else {
        output::print_drift_grouped(&results);
    }

    ExitCode::SUCCESS
}

/// Show record-type drift events from `record_type_drift_log` (migration 007).
///
/// Routes through the daemon HTTP API when available, falling back to a direct
/// DB connection. Output mirrors the `schema-drift` subcommand's table/JSON
/// shape but operates on the variant-level drift table populated by the
/// JSONLRecord::Unknown path introduced in B1.1 and the bytewise re-ingestion
/// completed in B1.2.
async fn run_record_type_drift(
    mode: ConnectionMode,
    type_name: Option<String>,
    version: Option<String>,
    since: Option<String>,
    limit: usize,
    json: bool,
) -> ExitCode {
    let results = match mode {
        ConnectionMode::Daemon(client) => {
            tracing::info!(
                socket = %client.socket_path().display(),
                "Routing record-type-drift through daemon"
            );
            match client
                .record_type_drift(
                    type_name.as_deref(),
                    version.as_deref(),
                    since.as_deref(),
                    Some(limit),
                )
                .await
            {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("Error: Daemon record-type-drift query failed: {}", e);
                    return ExitCode::FAILURE;
                }
            }
        }
        ConnectionMode::Direct { conn, .. } => {
            let type_name_owned = type_name.clone();
            let version_owned = version.clone();
            let since_owned = since.clone();
            match conn
                .call(move |conn| {
                    claude_history_store::query::record_type_drift_list(
                        conn,
                        type_name_owned.as_deref(),
                        version_owned.as_deref(),
                        since_owned.as_deref(),
                        Some(limit),
                    )
                })
                .await
            {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("Error: Record-type drift query failed: {}", e);
                    return ExitCode::FAILURE;
                }
            }
        }
    };

    if results.is_empty() {
        eprintln!("No record-type drift detected.");
        return ExitCode::SUCCESS;
    }

    if json {
        if output::print_json(&results).is_err() {
            return ExitCode::FAILURE;
        }
    } else {
        output::print_record_type_drift(&results);
    }

    ExitCode::SUCCESS
}

/// Dispatch the `attachments` subcommand to its `list` or `show` action.
///
/// Each action routes through the daemon HTTP API when available, falling
/// back to a direct DB connection. Output format is JSON via `--json` or a
/// human-readable table / labelled-block via `output::print_attachments_*`.
async fn run_attachments(mode: ConnectionMode, action: AttachmentsAction) -> ExitCode {
    match action {
        AttachmentsAction::List {
            project,
            inner_type,
            since,
            limit,
            json,
        } => run_attachments_list(mode, project, inner_type, since, limit, json).await,
        AttachmentsAction::Show { uuid, json } => run_attachments_show(mode, uuid, json).await,
    }
}

async fn run_attachments_list(
    mode: ConnectionMode,
    project: Option<String>,
    inner_type: Option<String>,
    since: Option<String>,
    limit: usize,
    json: bool,
) -> ExitCode {
    let results = match mode {
        ConnectionMode::Daemon(client) => {
            tracing::info!(
                socket = %client.socket_path().display(),
                "Routing attachments list through daemon"
            );
            match client
                .attachments_list(
                    project.as_deref(),
                    inner_type.as_deref(),
                    since.as_deref(),
                    Some(limit),
                )
                .await
            {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("Error: Daemon attachments list failed: {}", e);
                    return ExitCode::FAILURE;
                }
            }
        }
        ConnectionMode::Direct { conn, .. } => {
            let project_owned = project.clone();
            let inner_type_owned = inner_type.clone();
            let since_owned = since.clone();
            match conn
                .call(move |conn| {
                    claude_history_store::query::attachments_list(
                        conn,
                        project_owned.as_deref(),
                        inner_type_owned.as_deref(),
                        since_owned.as_deref(),
                        limit,
                    )
                })
                .await
            {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("Error: Attachments list query failed: {}", e);
                    return ExitCode::FAILURE;
                }
            }
        }
    };

    if json {
        if output::print_json(&results).is_err() {
            return ExitCode::FAILURE;
        }
    } else {
        output::print_attachments_table(&results);
    }

    ExitCode::SUCCESS
}

async fn run_attachments_show(mode: ConnectionMode, uuid: String, json: bool) -> ExitCode {
    let result = match mode {
        ConnectionMode::Daemon(client) => {
            tracing::info!(
                socket = %client.socket_path().display(),
                "Routing attachments show through daemon"
            );
            match client.attachment_show(&uuid).await {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("Error: Daemon attachment show failed: {}", e);
                    return ExitCode::FAILURE;
                }
            }
        }
        ConnectionMode::Direct { conn, .. } => {
            let uuid_owned = uuid.clone();
            match conn
                .call(move |conn| {
                    claude_history_store::query::attachment_by_uuid(conn, &uuid_owned)
                })
                .await
            {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("Error: Attachment show query failed: {}", e);
                    return ExitCode::FAILURE;
                }
            }
        }
    };

    match result {
        None => {
            eprintln!("Attachment not found: {}", uuid);
            ExitCode::FAILURE
        }
        Some(row) => {
            if json {
                if output::print_json(&row).is_err() {
                    return ExitCode::FAILURE;
                }
            } else {
                output::print_attachment_show(&row);
            }
            ExitCode::SUCCESS
        }
    }
}

/// Dispatcher for the `plans` subcommand (C2.4).
///
/// Routes to list / show. Both go direct-to-DB regardless of daemon
/// availability: the daemon's `/v1/plans` REST endpoints are C2.6
/// territory and not yet present, so daemon-routing would fail with
/// `404` here. `ensure_direct_conn` opens a fresh `tokio_rusqlite`
/// connection from `CLAUDE_HISTORY_DB_PATH` / `--db-path` / default
/// when the resolved mode was `Daemon`, and reuses the existing direct
/// connection when it is already `Direct`.
async fn run_plans(mode: ConnectionMode, action: PlansAction) -> ExitCode {
    let conn = match ensure_direct_conn(mode).await {
        Ok(c) => c,
        Err(msg) => {
            eprintln!("Error: {}", msg);
            return ExitCode::FAILURE;
        }
    };

    match action {
        PlansAction::List {
            project,
            since,
            limit,
            json,
        } => run_plans_list(conn, project, since, limit, json).await,
        PlansAction::Show { session_id, json } => {
            run_plans_show(conn, session_id, json).await
        }
    }
}

/// Coerce a `ConnectionMode` into a direct `tokio_rusqlite::Connection`.
///
/// When `Daemon` is active, opens a fresh DB connection at the resolved
/// db_path. When already `Direct`, reuses the existing connection. The
/// extra connection in daemon mode runs alongside the daemon's own
/// connection — SQLite handles concurrent readers cleanly under WAL
/// mode (the schema's default).
async fn ensure_direct_conn(
    mode: ConnectionMode,
) -> Result<tokio_rusqlite::Connection, String> {
    match mode {
        ConnectionMode::Direct { conn, .. } => Ok(conn),
        ConnectionMode::Daemon(_) => {
            let db_path = resolve_db_path(None).ok_or_else(|| {
                "Could not determine database path for plans subcommand.".to_string()
            })?;
            tokio_rusqlite::Connection::open(&db_path)
                .await
                .map_err(|e| format!("Failed to open DB at {}: {}", db_path.display(), e))
        }
    }
}

/// `claude-history plans list` — list plan-bearing messages with filters.
async fn run_plans_list(
    conn: tokio_rusqlite::Connection,
    project: Option<String>,
    since: Option<String>,
    limit: usize,
    json: bool,
) -> ExitCode {
    let project_owned = project.clone();
    let since_owned = since.clone();
    let results = match conn
        .call(move |conn| {
            claude_history_store::query::plans_list(
                conn,
                project_owned.as_deref(),
                since_owned.as_deref(),
                limit,
            )
            .map_err(tokio_rusqlite::Error::from)
        })
        .await
    {
        Ok(r) => r,
        Err(e) => {
            eprintln!("Error: Plans list query failed: {}", e);
            return ExitCode::FAILURE;
        }
    };

    if json {
        if output::print_json(&results).is_err() {
            return ExitCode::FAILURE;
        }
    } else {
        output::print_plans_list(&results);
    }

    ExitCode::SUCCESS
}

/// `claude-history plans show <session-id>` — full plan markdown for a session.
async fn run_plans_show(
    conn: tokio_rusqlite::Connection,
    session_id: String,
    json: bool,
) -> ExitCode {
    let session_id_owned = session_id.clone();
    let results = match conn
        .call(move |conn| {
            claude_history_store::query::plan_show(conn, &session_id_owned)
                .map_err(tokio_rusqlite::Error::from)
        })
        .await
    {
        Ok(r) => r,
        Err(e) => {
            eprintln!("Error: Plan show query failed: {}", e);
            return ExitCode::FAILURE;
        }
    };

    if results.is_empty() {
        eprintln!(
            "No plan-bearing messages found for session: {}",
            session_id
        );
        return ExitCode::FAILURE;
    }

    if json {
        if output::print_json(&results).is_err() {
            return ExitCode::FAILURE;
        }
    } else {
        output::print_plan_show(&results);
    }

    ExitCode::SUCCESS
}

/// List rows from the `hook_executions` table (migration 008).
///
/// Routes through the daemon HTTP API when available, falling back to a
/// direct DB connection. Output via `--json` or a human-readable table.
async fn run_hook_executions(
    mode: ConnectionMode,
    tool_use_id: Option<String>,
    hook_event: Option<String>,
    exit_code: Option<i64>,
    limit: usize,
    json: bool,
) -> ExitCode {
    let results = match mode {
        ConnectionMode::Daemon(client) => {
            tracing::info!(
                socket = %client.socket_path().display(),
                "Routing hook-executions through daemon"
            );
            match client
                .hook_executions_list(
                    tool_use_id.as_deref(),
                    hook_event.as_deref(),
                    exit_code,
                    Some(limit),
                )
                .await
            {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("Error: Daemon hook-executions query failed: {}", e);
                    return ExitCode::FAILURE;
                }
            }
        }
        ConnectionMode::Direct { conn, .. } => {
            let tool_use_id_owned = tool_use_id.clone();
            let hook_event_owned = hook_event.clone();
            match conn
                .call(move |conn| {
                    claude_history_store::query::hook_executions_list(
                        conn,
                        tool_use_id_owned.as_deref(),
                        hook_event_owned.as_deref(),
                        exit_code,
                        limit,
                    )
                })
                .await
            {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("Error: Hook-executions query failed: {}", e);
                    return ExitCode::FAILURE;
                }
            }
        }
    };

    if json {
        if output::print_json(&results).is_err() {
            return ExitCode::FAILURE;
        }
    } else {
        output::print_hook_executions(&results);
    }

    ExitCode::SUCCESS
}

/// List files touched by Claude Code across sessions.
///
/// [CLI-10, CLI-15] Routes through daemon HTTP API when available, otherwise
/// uses direct DB access via store::artifact_queries::list_files. Output format
/// is identical regardless of data source.
async fn run_files(
    mode: ConnectionMode,
    session_id: Option<String>,
    path: Option<String>,
    project: Option<String>,
    limit: usize,
    json: bool,
) -> ExitCode {
    let results = match mode {
        ConnectionMode::Daemon(client) => {
            tracing::info!(
                socket = %client.socket_path().display(),
                "Routing files through daemon"
            );
            match client
                .files(session_id.as_deref(), path.as_deref(), project.as_deref(), Some(limit))
                .await
            {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("Error: Daemon files query failed: {}", e);
                    return ExitCode::FAILURE;
                }
            }
        }
        ConnectionMode::Direct { conn, .. } => {
            match conn
                .call(move |conn| {
                    claude_history_store::artifact_queries::list_files(
                        conn,
                        session_id.as_deref(),
                        path.as_deref(),
                        project.as_deref(),
                        limit,
                    )
                })
                .await
            {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("Error: Files query failed: {}", e);
                    return ExitCode::FAILURE;
                }
            }
        }
    };

    if json {
        if output::print_json(&results).is_err() {
            return ExitCode::FAILURE;
        }
    } else {
        output::print_files_table(&results);
    }

    ExitCode::SUCCESS
}

/// Show chronological operations on a file.
///
/// [CLI-11, CLI-15] Routes through daemon HTTP API when available (not yet
/// supported -- daemon does not expose a direct file-history-by-path endpoint),
/// otherwise uses direct DB access via store::artifact_queries::query_file_operations.
/// In v1 this always uses direct DB mode for simplicity. Output format is
/// identical regardless of data source.
async fn run_file_history(
    mode: ConnectionMode,
    path: String,
    session_id: Option<String>,
    project: Option<String>,
    limit: usize,
    json: bool,
) -> ExitCode {
    // File history by path is not directly exposed via daemon API in v1.
    // Use direct DB mode. If daemon mode is active, open the DB anyway.
    let conn = match mode {
        ConnectionMode::Direct { conn, .. } => conn,
        ConnectionMode::Daemon(_) => {
            // Fall back to direct DB for file history queries.
            let db_path = match resolve_db_path(None) {
                Some(p) => p,
                None => {
                    eprintln!("Error: Could not determine database path for file-history.");
                    return ExitCode::FAILURE;
                }
            };
            match claude_history_store::db::init_db(&db_path).await {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("Error: Failed to open database: {}", e);
                    return ExitCode::FAILURE;
                }
            }
        }
    };

    let p = path.clone();
    let results = match conn
        .call(move |conn| {
            claude_history_store::artifact_queries::query_file_operations(
                conn,
                &p,
                session_id.as_deref(),
                project.as_deref(),
                limit,
            )
        })
        .await
    {
        Ok(r) => r,
        Err(e) => {
            eprintln!("Error: File history query failed: {}", e);
            return ExitCode::FAILURE;
        }
    };

    eprintln!("{} operation(s) for \"{}\"", results.len(), path);

    if json {
        if output::print_json(&results).is_err() {
            return ExitCode::FAILURE;
        }
    } else {
        output::print_file_operations(&results);
    }

    ExitCode::SUCCESS
}

/// Reconstruct file content at a point in time.
///
/// [CLI-12] Uses direct DB access only (reconstruction requires sequential
/// operation replay best done locally). Calls
/// artifact_queries::reconstruct_file_content and prints the raw content
/// to stdout. Prints diagnostic to stderr if no content can be reconstructed.
async fn run_reconstruct(
    mode: ConnectionMode,
    path: String,
    session_id: String,
    at: Option<String>,
) -> ExitCode {
    // Reconstruction always uses direct DB for v1.
    let conn = match mode {
        ConnectionMode::Direct { conn, .. } => conn,
        ConnectionMode::Daemon(_) => {
            let db_path = match resolve_db_path(None) {
                Some(p) => p,
                None => {
                    eprintln!("Error: Could not determine database path for reconstruct.");
                    return ExitCode::FAILURE;
                }
            };
            match claude_history_store::db::init_db(&db_path).await {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("Error: Failed to open database: {}", e);
                    return ExitCode::FAILURE;
                }
            }
        }
    };

    let p = path.clone();
    let sid = session_id.clone();
    let result = match conn
        .call(move |conn| {
            claude_history_store::artifact_queries::reconstruct_file_content(
                conn,
                &p,
                &sid,
                at.as_deref(),
            )
        })
        .await
    {
        Ok(r) => r,
        Err(e) => {
            eprintln!("Error: Reconstruction failed: {}", e);
            return ExitCode::FAILURE;
        }
    };

    match result {
        Some(content) => {
            print!("{}", content);
            ExitCode::SUCCESS
        }
        None => {
            eprintln!(
                "No reconstructable content for \"{}\" in session {}",
                path, session_id
            );
            ExitCode::SUCCESS
        }
    }
}

/// Show git operations extracted from Bash tool calls.
///
/// [CLI-13, CLI-15] Routes through daemon HTTP API when available, otherwise
/// uses direct DB access via store::artifact_queries::list_git_operations.
/// Output format is identical regardless of data source.
async fn run_git_log(
    mode: ConnectionMode,
    session_id: Option<String>,
    operation_type: Option<String>,
    limit: usize,
    json: bool,
) -> ExitCode {
    let results = match mode {
        ConnectionMode::Daemon(client) => {
            tracing::info!(
                socket = %client.socket_path().display(),
                "Routing git-log through daemon"
            );
            match client
                .git_operations(
                    session_id.as_deref(),
                    operation_type.as_deref(),
                    Some(limit),
                )
                .await
            {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("Error: Daemon git-log query failed: {}", e);
                    return ExitCode::FAILURE;
                }
            }
        }
        ConnectionMode::Direct { conn, .. } => {
            match conn
                .call(move |conn| {
                    claude_history_store::artifact_queries::list_git_operations(
                        conn,
                        session_id.as_deref(),
                        operation_type.as_deref(),
                        limit,
                    )
                })
                .await
            {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("Error: Git log query failed: {}", e);
                    return ExitCode::FAILURE;
                }
            }
        }
    };

    if json {
        if output::print_json(&results).is_err() {
            return ExitCode::FAILURE;
        }
    } else {
        output::print_git_operations(&results);
    }

    ExitCode::SUCCESS
}

/// Show combined file and git artifacts for a session.
///
/// [CLI-14, CLI-15] Routes through daemon HTTP API when available, otherwise
/// uses direct DB access via store::artifact_queries::query_session_artifacts.
/// Output format is identical regardless of data source.
async fn run_artifacts(
    mode: ConnectionMode,
    session_id: String,
    json: bool,
) -> ExitCode {
    let results = match mode {
        ConnectionMode::Daemon(client) => {
            tracing::info!(
                socket = %client.socket_path().display(),
                "Routing artifacts through daemon"
            );
            match client.artifacts(&session_id).await {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("Error: Daemon artifacts query failed: {}", e);
                    return ExitCode::FAILURE;
                }
            }
        }
        ConnectionMode::Direct { conn, .. } => {
            match conn
                .call(move |conn| {
                    claude_history_store::artifact_queries::query_session_artifacts(
                        conn,
                        &session_id,
                    )
                })
                .await
            {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("Error: Artifacts query failed: {}", e);
                    return ExitCode::FAILURE;
                }
            }
        }
    };

    if json {
        if output::print_json(&results).is_err() {
            return ExitCode::FAILURE;
        }
    } else {
        output::print_artifacts(&results);
    }

    ExitCode::SUCCESS
}

/// Dispatch the `queries` subcommand group.
///
/// List and Show are filesystem-only (no database needed). Run requires a
/// database connection to execute the prepared SQL through sql_passthrough.
async fn run_queries(action: QueriesAction, db_path_arg: Option<PathBuf>) -> ExitCode {
    match action {
        QueriesAction::List { json, queries_dir } => {
            run_queries_list(json, queries_dir).await
        }
        QueriesAction::Show { name, queries_dir } => {
            run_queries_show(name, queries_dir).await
        }
        QueriesAction::Run {
            name,
            params,
            json,
            queries_dir,
        } => run_queries_run(name, params, json, queries_dir, db_path_arg).await,
    }
}

/// List all available canned queries from the queries directory.
///
/// Filesystem-only: no database connection needed. Loads .sql files from
/// the queries directory and prints a summary table or JSON.
async fn run_queries_list(json: bool, queries_dir_arg: Option<PathBuf>) -> ExitCode {
    let dir = queries_dir_arg.unwrap_or_else(claude_history_store::query_registry::resolve_queries_dir);

    let queries = match claude_history_store::query_registry::load_queries(&dir) {
        Ok(q) => q,
        Err(e) => {
            eprintln!("Error: Failed to load queries from {}: {}", dir.display(), e);
            return ExitCode::FAILURE;
        }
    };

    if queries.is_empty() {
        eprintln!("No queries found in {}", dir.display());
        return ExitCode::SUCCESS;
    }

    if json {
        let mut list: Vec<&claude_history_store::query_registry::CannedQuery> =
            queries.values().collect();
        list.sort_by(|a, b| a.name.cmp(&b.name));
        if output::print_json(&list).is_err() {
            return ExitCode::FAILURE;
        }
    } else {
        let mut list: Vec<&claude_history_store::query_registry::CannedQuery> =
            queries.values().collect();
        list.sort_by(|a, b| a.name.cmp(&b.name));
        output::print_queries_list(&list);
    }

    ExitCode::SUCCESS
}

/// Show SQL template and metadata for a specific canned query.
///
/// Filesystem-only: no database connection needed. Prints the raw SQL,
/// description, and parameter definitions in human-readable format.
async fn run_queries_show(name: String, queries_dir_arg: Option<PathBuf>) -> ExitCode {
    let dir = queries_dir_arg.unwrap_or_else(claude_history_store::query_registry::resolve_queries_dir);

    let queries = match claude_history_store::query_registry::load_queries(&dir) {
        Ok(q) => q,
        Err(e) => {
            eprintln!("Error: Failed to load queries from {}: {}", dir.display(), e);
            return ExitCode::FAILURE;
        }
    };

    let query = match queries.get(&name) {
        Some(q) => q,
        None => {
            eprintln!("Error: Query '{}' not found in {}", name, dir.display());
            let available: Vec<&str> = queries.keys().map(|k| k.as_str()).collect();
            if !available.is_empty() {
                eprintln!("Available queries: {}", available.join(", "));
            }
            return ExitCode::FAILURE;
        }
    };

    println!("Query: {}", query.name);
    println!("Description: {}", query.description);
    println!();
    println!("SQL:");
    println!("{}", query.sql);

    if !query.params.is_empty() {
        println!("Parameters:");
        for p in &query.params {
            let default_str = match &p.default {
                Some(d) => format!(" (default: {})", d),
                None => " (required)".to_string(),
            };
            let type_str = match p.param_type {
                claude_history_store::query_registry::ParamType::Text => "",
                claude_history_store::query_registry::ParamType::Integer => " [integer]",
                claude_history_store::query_registry::ParamType::Real => " [real]",
            };
            if p.description.is_empty() {
                println!("  :{}{}{}", p.name, type_str, default_str);
            } else {
                println!("  :{} -- {}{}{}", p.name, p.description, type_str, default_str);
            }
        }
    }

    ExitCode::SUCCESS
}

/// Execute a canned query with named parameter binding.
///
/// Requires a database connection. Loads the query, converts named :param
/// placeholders to positional ?N params via prepare_sql, then executes through
/// sql_passthrough::execute_sql. Output is always JSON (consistent with sql
/// passthrough behavior).
async fn run_queries_run(
    name: String,
    param_pairs: Vec<(String, String)>,
    json: bool,
    queries_dir_arg: Option<PathBuf>,
    db_path_arg: Option<PathBuf>,
) -> ExitCode {
    let dir = queries_dir_arg.unwrap_or_else(claude_history_store::query_registry::resolve_queries_dir);

    let queries = match claude_history_store::query_registry::load_queries(&dir) {
        Ok(q) => q,
        Err(e) => {
            eprintln!("Error: Failed to load queries from {}: {}", dir.display(), e);
            return ExitCode::FAILURE;
        }
    };

    let query = match queries.get(&name) {
        Some(q) => q.clone(),
        None => {
            eprintln!("Error: Query '{}' not found in {}", name, dir.display());
            let available: Vec<&str> = queries.keys().map(|k| k.as_str()).collect();
            if !available.is_empty() {
                eprintln!("Available queries: {}", available.join(", "));
            }
            return ExitCode::FAILURE;
        }
    };

    // Convert param pairs to HashMap
    let params: std::collections::HashMap<String, String> =
        param_pairs.into_iter().collect();

    // Prepare the SQL with positional parameters
    let (sql, positional_params) =
        match claude_history_store::query_registry::prepare_sql(&query, &params) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("Error: Parameter binding failed: {}", e);
                return ExitCode::FAILURE;
            }
        };

    // Resolve connection mode for DB access
    let mode = match resolve_connection_mode(db_path_arg).await {
        Ok(m) => m,
        Err(msg) => {
            eprintln!("Error: {}", msg);
            return ExitCode::FAILURE;
        }
    };

    let results = match mode {
        ConnectionMode::Daemon(_) => {
            // For daemon mode, fall back to direct DB since the daemon's sql
            // endpoint expects raw SQL (not canned query names). We already
            // have the prepared SQL, so open the DB directly.
            let db_path = match resolve_db_path(None) {
                Some(p) => p,
                None => {
                    eprintln!("Error: Could not determine database path for query execution.");
                    return ExitCode::FAILURE;
                }
            };
            let conn = match claude_history_store::db::init_db(&db_path).await {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("Error: Failed to open database: {}", e);
                    return ExitCode::FAILURE;
                }
            };
            conn.call(move |conn| {
                claude_history_store::sql_passthrough::execute_sql(conn, &sql, &positional_params)
                    .map_err(|e| {
                        tokio_rusqlite::rusqlite::Error::ToSqlConversionFailure(
                            Box::new(e) as Box<dyn std::error::Error + Send + Sync>
                        )
                    })
            })
            .await
        }
        ConnectionMode::Direct { conn, .. } => {
            conn.call(move |conn| {
                claude_history_store::sql_passthrough::execute_sql(conn, &sql, &positional_params)
                    .map_err(|e| {
                        tokio_rusqlite::rusqlite::Error::ToSqlConversionFailure(
                            Box::new(e) as Box<dyn std::error::Error + Send + Sync>
                        )
                    })
            })
            .await
        }
    };

    match results {
        Ok(rows) => {
            eprintln!("{} row(s) returned", rows.len());
            if json || true {
                // Default to JSON output for query run (consistent with sql passthrough)
                if output::print_json(&rows).is_err() {
                    return ExitCode::FAILURE;
                }
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("Error: Query execution failed: {}", e);
            ExitCode::FAILURE
        }
    }
}

/// Print MCP client configuration snippets.
///
/// Generates ready-to-use JSON configuration for each supported MCP client,
/// resolving the actual binary path at runtime. Clients differ in their config
/// structure: VS Code uses `servers` (not `mcpServers`), Zed uses
/// `context_servers` with nested `command`, and Claude Code supports both
/// stdio and HTTP transports.
fn print_mcp_config(client: McpClient) {
    // Resolve the binary path for config snippets. Prefer the actual executable
    // path over just "claude-history" so configs work without PATH adjustments.
    let binary = std::env::current_exe()
        .ok()
        .and_then(|p| p.canonicalize().ok())
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "claude-history".to_string());

    match client {
        McpClient::All => {
            print_config_claude_code(&binary);
            println!();
            print_config_claude_desktop(&binary);
            println!();
            print_config_cursor(&binary);
            println!();
            print_config_vscode(&binary);
            println!();
            print_config_windsurf(&binary);
            println!();
            print_config_zed(&binary);
        }
        McpClient::ClaudeCode => print_config_claude_code(&binary),
        McpClient::ClaudeDesktop => print_config_claude_desktop(&binary),
        McpClient::Cursor => print_config_cursor(&binary),
        McpClient::Vscode => print_config_vscode(&binary),
        McpClient::Windsurf => print_config_windsurf(&binary),
        McpClient::Zed => print_config_zed(&binary),
    }
}

fn print_config_claude_code(binary: &str) {
    println!("# Claude Code");
    println!("# Add to .mcp.json (project) or ~/.claude/.mcp.json (global)");
    println!("# Or run: claude mcp add claude-history -- {} mcp-stdio", binary);
    println!();
    println!("# Option 1: stdio transport");
    println!("{}", serde_json::to_string_pretty(&serde_json::json!({
        "mcpServers": {
            "claude-history": {
                "type": "stdio",
                "command": binary,
                "args": ["mcp-stdio"]
            }
        }
    })).unwrap());
    println!();
    println!("# Option 2: HTTP transport (requires daemon running)");
    println!("{}", serde_json::to_string_pretty(&serde_json::json!({
        "mcpServers": {
            "claude-history": {
                "type": "http",
                "url": "http://127.0.0.1:7424/mcp"
            }
        }
    })).unwrap());
}

fn print_config_claude_desktop(binary: &str) {
    println!("# Claude Desktop");
    println!("# Add to ~/Library/Application Support/Claude/claude_desktop_config.json");
    println!("{}", serde_json::to_string_pretty(&serde_json::json!({
        "mcpServers": {
            "claude-history": {
                "command": binary,
                "args": ["mcp-stdio"]
            }
        }
    })).unwrap());
}

fn print_config_cursor(binary: &str) {
    println!("# Cursor");
    println!("# Add to .cursor/mcp.json in your project root");
    println!("{}", serde_json::to_string_pretty(&serde_json::json!({
        "mcpServers": {
            "claude-history": {
                "command": binary,
                "args": ["mcp-stdio"]
            }
        }
    })).unwrap());
}

fn print_config_vscode(binary: &str) {
    println!("# VS Code / GitHub Copilot");
    println!("# Add to .vscode/mcp.json in your project root");
    println!("# Note: VS Code uses \"servers\" (not \"mcpServers\")");
    println!("{}", serde_json::to_string_pretty(&serde_json::json!({
        "servers": {
            "claude-history": {
                "type": "stdio",
                "command": binary,
                "args": ["mcp-stdio"]
            }
        }
    })).unwrap());
}

fn print_config_windsurf(binary: &str) {
    println!("# Windsurf / Codeium");
    println!("# Add to ~/.codeium/windsurf/mcp_config.json");
    println!("{}", serde_json::to_string_pretty(&serde_json::json!({
        "mcpServers": {
            "claude-history": {
                "command": binary,
                "args": ["mcp-stdio"]
            }
        }
    })).unwrap());
}

fn print_config_zed(binary: &str) {
    println!("# Zed");
    println!("# Add to ~/.config/zed/settings.json");
    println!("# Note: Zed uses \"context_servers\" with nested \"command\" structure");
    println!("{}", serde_json::to_string_pretty(&serde_json::json!({
        "context_servers": {
            "claude-history": {
                "command": {
                    "path": binary,
                    "args": ["mcp-stdio"]
                }
            }
        }
    })).unwrap());
}

/// Run as an MCP server over stdin/stdout.
///
/// Opens the database directly (no daemon needed), builds the McpService with
/// shared state, and serves the MCP protocol over stdin/stdout using rmcp's
/// stdio transport. Designed for Claude Desktop integration.
async fn run_mcp_stdio(db_path_arg: Option<PathBuf>) -> ExitCode {
    let db_path = match resolve_db_path(db_path_arg) {
        Some(p) => p,
        None => {
            eprintln!("Error: Could not determine database path.");
            return ExitCode::FAILURE;
        }
    };

    if !db_path.exists() {
        eprintln!(
            "Error: Database file does not exist: {}\n\
             Run `claude-history sync` first to create the database.",
            db_path.display()
        );
        return ExitCode::FAILURE;
    }

    let conn = match claude_history_store::db::init_db(&db_path).await {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Error: Failed to open database: {}", e);
            return ExitCode::FAILURE;
        }
    };

    // Build minimal AppState (no SSE broadcast needed for stdio mode).
    let (event_tx, _rx) = tokio::sync::broadcast::channel::<crate::events::SseEvent>(1);
    let state = Arc::new(state::AppState {
        conn,
        version: env!("CARGO_PKG_VERSION").to_string(),
        db_path,
        event_tx,
    });

    let service = mcp::McpService::new(state);

    // Serve MCP protocol over stdin/stdout.
    let transport = rmcp::transport::io::stdio();
    match rmcp::ServiceExt::serve(service, transport).await {
        Ok(running) => {
            if let Err(e) = running.waiting().await {
                eprintln!("Error: MCP stdio service error: {}", e);
                return ExitCode::FAILURE;
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("Error: Failed to start MCP stdio service: {}", e);
            ExitCode::FAILURE
        }
    }
}
