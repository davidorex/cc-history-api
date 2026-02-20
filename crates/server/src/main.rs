//! claude-history CLI binary.
//!
//! Provides the `claude-history` command with 8 subcommands for syncing JSONL
//! session files into a local SQLite database, searching message content,
//! browsing sessions, querying messages, viewing usage statistics, exporting
//! sessions, checking Claude Code versions, and inspecting schema drift.
//!
//! Usage:
//!   claude-history sync [--projects-dir <path>] [--db-path <path>]
//!   claude-history search <query> [--limit N] [--json]
//!   claude-history sessions [--project <path>] [--after <date>] [--before <date>] [--limit N] [--json]
//!   claude-history query [--session-id <id>] [--type <type>] [--model <m>] [--tool <t>] [--limit N]
//!   claude-history stats [--session-id <id>] [--json]
//!   claude-history export <session-id> [--format json|markdown|csv]
//!   claude-history version-check [--json]
//!   claude-history schema-drift [--record-type <type>] [--limit N] [--json]
//!
//! All subcommands share a global --db-path option. Logs go to stderr,
//! structured output to stdout (PAT-020).

use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

mod export;
mod output;

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
/// 2. $HOME/.claude/projects/ (fallback default)
fn resolve_projects_dir(cli_arg: Option<PathBuf>) -> Option<PathBuf> {
    if let Some(p) = cli_arg {
        return Some(p);
    }

    std::env::var("HOME").ok().map(|home| {
        PathBuf::from(home).join(".claude").join("projects")
    })
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
        Commands::Sync { projects_dir } => run_sync(projects_dir, cli.db_path).await,
        Commands::Search { query, limit, json } => {
            run_search(cli.db_path, query, limit, json).await
        }
        Commands::Sessions {
            project,
            after,
            before,
            limit,
            json,
        } => run_sessions(cli.db_path, project, after, before, limit, json).await,
        Commands::Query {
            session_id,
            message_type,
            model,
            tool,
            after,
            before,
            limit,
        } => run_query(cli.db_path, session_id, message_type, model, tool, after, before, limit).await,
        Commands::Stats { session_id, json } => run_stats(cli.db_path, session_id, json).await,
        Commands::Export { session_id, format } => {
            run_export(cli.db_path, session_id, format).await
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

/// Open the database for read-only query subcommands.
///
/// Resolves the db_path and initializes the connection. Returns the async
/// connection handle, or prints an error to stderr and returns None.
async fn open_db(db_path_arg: Option<PathBuf>) -> Option<tokio_rusqlite::Connection> {
    let db_path = match resolve_db_path(db_path_arg) {
        Some(p) => p,
        None => {
            eprintln!("Error: Could not determine database path. Set CLAUDE_HISTORY_DB_PATH or HOME environment variable, or pass --db-path.");
            return None;
        }
    };

    if !db_path.exists() {
        eprintln!(
            "Error: Database file does not exist: {}\n\
             Run `claude-history sync` first to create the database.",
            db_path.display()
        );
        return None;
    }

    match claude_history_store::db::init_db(&db_path).await {
        Ok(conn) => Some(conn),
        Err(e) => {
            eprintln!(
                "Error: Failed to open database at {}: {}",
                db_path.display(),
                e
            );
            None
        }
    }
}

/// Search message content using FTS5 full-text search.
///
/// [CLI-05] Calls store::fts::search_messages inside conn.call(), then
/// formats output via output.rs based on the --json flag. Prints result
/// count summary to stderr.
async fn run_search(
    db_path_arg: Option<PathBuf>,
    query: String,
    limit: usize,
    json: bool,
) -> ExitCode {
    let conn = match open_db(db_path_arg).await {
        Some(c) => c,
        None => return ExitCode::FAILURE,
    };

    let q = query.clone();
    let results = match conn
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
/// [CLI-04] Calls store::query::list_sessions inside conn.call(), then
/// formats output as a table or JSON based on the --json flag.
async fn run_sessions(
    db_path_arg: Option<PathBuf>,
    project: Option<String>,
    after: Option<String>,
    before: Option<String>,
    limit: usize,
    json: bool,
) -> ExitCode {
    let conn = match open_db(db_path_arg).await {
        Some(c) => c,
        None => return ExitCode::FAILURE,
    };

    let results = match conn
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
/// [CLI-03] Calls store::query::query_messages inside conn.call().
/// This subcommand always outputs JSON to stdout (designed for machine
/// consumption per spec).
#[allow(clippy::too_many_arguments)]
async fn run_query(
    db_path_arg: Option<PathBuf>,
    session_id: Option<String>,
    message_type: Option<String>,
    model: Option<String>,
    tool: Option<String>,
    after: Option<String>,
    before: Option<String>,
    limit: usize,
) -> ExitCode {
    let conn = match open_db(db_path_arg).await {
        Some(c) => c,
        None => return ExitCode::FAILURE,
    };

    let results = match conn
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
    };

    if output::print_json(&results).is_err() {
        return ExitCode::FAILURE;
    }

    ExitCode::SUCCESS
}

/// Show token usage, tool frequency, and model breakdown statistics.
///
/// [CLI-06] Runs three queries: token_stats_by_model (or by_session if
/// --session-id provided), tool_frequency, and model_breakdown. Output is
/// either three human-readable sections or a combined JSON object.
async fn run_stats(
    db_path_arg: Option<PathBuf>,
    session_id: Option<String>,
    json: bool,
) -> ExitCode {
    let conn = match open_db(db_path_arg).await {
        Some(c) => c,
        None => return ExitCode::FAILURE,
    };

    let sid = session_id.clone();
    let stats_result = conn
        .call(move |conn| -> Result<_, tokio_rusqlite::rusqlite::Error> {
            let token_stats = if let Some(ref sid) = sid {
                claude_history_store::query::token_stats_by_session(conn, Some(sid.as_str()))
            } else {
                claude_history_store::query::token_stats_by_model(conn)
            }?;

            let tool_stats = claude_history_store::query::tool_frequency(conn)?;

            let model_stats = claude_history_store::query::model_breakdown(conn)?;

            Ok((token_stats, tool_stats, model_stats))
        })
        .await;

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
/// [CLI-07] Validates the format argument, then delegates to the appropriate
/// export function. The export runs inside conn.call(), writing to a Vec<u8>
/// buffer. The buffer is then flushed to stdout outside the closure to avoid
/// blocking the DB connection thread with I/O.
async fn run_export(
    db_path_arg: Option<PathBuf>,
    session_id: String,
    format: String,
) -> ExitCode {
    // Validate format before opening the database
    let valid_formats = ["json", "markdown", "csv"];
    if !valid_formats.contains(&format.as_str()) {
        eprintln!(
            "Error: Invalid format '{}'. Valid formats: json, markdown, csv",
            format
        );
        return ExitCode::FAILURE;
    }

    let conn = match open_db(db_path_arg).await {
        Some(c) => c,
        None => return ExitCode::FAILURE,
    };

    let fmt = format.clone();
    let sid = session_id.clone();
    let export_result = conn
        .call(move |conn| {
            let mut buffer = Vec::new();
            let result: Result<Vec<u8>, Box<dyn std::error::Error + Send + Sync>> = match fmt.as_str() {
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
            result.map_err(|e| {
                tokio_rusqlite::rusqlite::Error::ToSqlConversionFailure(e)
            })
        })
        .await;

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
