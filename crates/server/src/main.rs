use std::path::PathBuf;
use tracing_subscriber::EnvFilter;

/// Resolve the database file path.
///
/// Priority:
/// 1. CLAUDE_HISTORY_DB_PATH environment variable (if set and non-empty)
/// 2. $HOME/.claude/.claude-history.db (fallback default)
///
/// Returns None if HOME cannot be determined and env var is unset.
fn db_path() -> Option<PathBuf> {
    // Check explicit env var first
    if let Ok(p) = std::env::var("CLAUDE_HISTORY_DB_PATH") {
        if !p.is_empty() {
            return Some(PathBuf::from(p));
        }
    }

    // Fall back to ~/.claude/.claude-history.db
    std::env::var("HOME").ok().map(|home| {
        PathBuf::from(home)
            .join(".claude")
            .join(".claude-history.db")
    })
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize tracing with env-filter; defaults to INFO if RUST_LOG is unset
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let path = db_path().expect("Could not determine database path: set CLAUDE_HISTORY_DB_PATH or HOME");
    tracing::info!(db_path = %path.display(), "claude-history starting");

    Ok(())
}
