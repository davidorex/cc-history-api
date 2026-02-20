//! claude-history CLI binary.
//!
//! Provides the `claude-history` command with 14 subcommands for syncing JSONL
//! session files into a local SQLite database, starting the HTTP/UDS daemon,
//! searching message content, browsing sessions, querying messages, viewing
//! usage statistics, exporting sessions, checking Claude Code versions,
//! inspecting schema drift, and interacting with the artifact layer.
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
//!   claude-history files [--session-id <id>] [--path <substr>] [--limit N] [--json]
//!   claude-history file-history <path> [--session-id <id>] [--limit N] [--json]
//!   claude-history reconstruct <path> --session-id <id> [--at <uuid>]
//!   claude-history git-log [--session-id <id>] [--type <type>] [--limit N] [--json]
//!   claude-history artifacts <session-id> [--json]
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
mod output;
mod serve;
mod state;
mod watcher;

use daemon_client::{ConnectionMode, detect_connection_mode, resolve_socket_path};

#[derive(Parser)]
#[command(name = "claude-history", about = "Claude Code session history API")]
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
    /// List files touched by Claude Code across sessions
    Files {
        /// Filter by session ID
        #[arg(long)]
        session_id: Option<String>,
        /// Filter by path substring
        #[arg(long)]
        path: Option<String>,
        /// Maximum results
        #[arg(long, default_value = "100")]
        limit: usize,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// Show chronological operations on a file
    FileHistory {
        /// File path to show history for
        path: String,
        /// Filter by session ID
        #[arg(long)]
        session_id: Option<String>,
        /// Maximum operations to show
        #[arg(long, default_value = "50")]
        limit: usize,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// Reconstruct file content at a point in time
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
}

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
                    limit,
                    json,
                } => run_sessions(mode, project, after, before, limit, json).await,
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
                Commands::Files {
                    session_id,
                    path,
                    limit,
                    json,
                } => run_files(mode, session_id, path, limit, json).await,
                Commands::FileHistory {
                    path,
                    session_id,
                    limit,
                    json,
                } => run_file_history(mode, path, session_id, limit, json).await,
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
                // Serve and Sync already handled above.
                Commands::Serve { .. } | Commands::Sync { .. } => unreachable!(),
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
                    claude_history_store::fts::search_messages(conn, &q, limit, 0)
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
/// [CLI-08, CLI-15] Routes through daemon HTTP API when available, otherwise
/// uses direct DB access via store::query::version_history. Output format
/// is identical regardless of data source.
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
                .call(move |conn| claude_history_store::query::version_history(conn))
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
        println!(
            "{:<30}  {:<24}  {:<24}",
            "VERSION", "FIRST_SEEN", "LAST_SEEN"
        );
        println!(
            "{:<30}  {:<24}  {:<24}",
            "------------------------------",
            "------------------------",
            "------------------------"
        );
        for entry in &results {
            let version_display = if entry.version.len() > 30 {
                &entry.version[..30]
            } else {
                &entry.version
            };
            let first_display = if entry.first_seen.len() > 24 {
                &entry.first_seen[..24]
            } else {
                &entry.first_seen
            };
            let last_display = if entry.last_seen.len() > 24 {
                &entry.last_seen[..24]
            } else {
                &entry.last_seen
            };
            println!(
                "{:<30}  {:<24}  {:<24}",
                version_display, first_display, last_display
            );
        }
    }

    ExitCode::SUCCESS
}

/// Show schema drift events (unknown fields captured during ingestion).
///
/// [CLI-09, CLI-15] Routes through daemon HTTP API when available, otherwise
/// uses direct DB access via store::query::schema_drift_list. In daemon mode,
/// the daemon applies record_type filtering and limit on the server side.
/// In direct mode, filtering and limit are applied in Rust post-retrieval.
/// Output format is identical regardless of data source.
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
                .schema_drift(record_type.as_deref(), Some(limit))
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
            let all_results = match conn
                .call(move |conn| claude_history_store::query::schema_drift_list(conn))
                .await
            {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("Error: Schema drift query failed: {}", e);
                    return ExitCode::FAILURE;
                }
            };

            // Apply record_type filter in Rust if provided
            let filtered: Vec<_> = if let Some(ref rt) = record_type {
                all_results
                    .into_iter()
                    .filter(|e| e.record_type == *rt)
                    .collect()
            } else {
                all_results
            };

            // Apply limit truncation
            filtered.into_iter().take(limit).collect()
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
        println!(
            "{:<25}  {:<20}  {:<16}  {:<24}  {:<50}",
            "FIELD", "RECORD_TYPE", "VERSION", "FIRST_SEEN", "SAMPLE_VALUE"
        );
        println!(
            "{:<25}  {:<20}  {:<16}  {:<24}  {:<50}",
            "-------------------------",
            "--------------------",
            "----------------",
            "------------------------",
            "--------------------------------------------------"
        );
        for entry in &results {
            let field_display = if entry.field_name.len() > 25 {
                &entry.field_name[..25]
            } else {
                &entry.field_name
            };
            let rt_display = if entry.record_type.len() > 20 {
                &entry.record_type[..20]
            } else {
                &entry.record_type
            };
            let version_display = entry.version.as_deref().unwrap_or("-");
            let version_display = if version_display.len() > 16 {
                &version_display[..16]
            } else {
                version_display
            };
            let first_seen_display = if entry.first_seen_at.len() > 24 {
                &entry.first_seen_at[..24]
            } else {
                &entry.first_seen_at
            };
            let sample = entry.sample_value.as_deref().unwrap_or("-");
            let sample_display = if sample.len() > 50 {
                &sample[..50]
            } else {
                sample
            };
            println!(
                "{:<25}  {:<20}  {:<16}  {:<24}  {:<50}",
                field_display, rt_display, version_display, first_seen_display, sample_display
            );
        }
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
                .files(session_id.as_deref(), path.as_deref(), Some(limit))
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
