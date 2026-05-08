# cc-history-api

## Post-Build Protocol

After any `cargo build --release`: restart the daemon so the new binary is mmapped by the running process. The daemon serves all other projects via HTTP and UDS. A stale daemon means stale behavior system-wide.

The daemon runs under a user-level launchd LaunchAgent at `~/Library/LaunchAgents/com.davidrex.claude-history.plist` (Label: `com.davidrex.claude-history`). It auto-starts at user login, restarts on crash, and survives terminal close / logout / reboot. The plist itself lives outside the repo — it's user-level config with machine-specific absolute paths.

```bash
# Restart the supervised daemon — picks up the freshly-built binary
# via the ~/.local/bin/claude-history symlink. The -k flag terminates
# the running process before relaunching.
launchctl kickstart -k gui/$(id -u)/com.davidrex.claude-history

# Status
launchctl list | grep claude-history          # numeric PID + exit-status 0 = running
curl -s http://127.0.0.1:7424/v1/health       # responds when healthy

# Logs (tracing crate writes to stderr)
tail -f ~/Library/Logs/claude-history.err.log

# Disable / re-enable persistently
launchctl unload -w ~/Library/LaunchAgents/com.davidrex.claude-history.plist
launchctl load   -w ~/Library/LaunchAgents/com.davidrex.claude-history.plist
```

**Do NOT** `pgrep -f 'claude-history serve' | xargs kill` then `claude-history serve &`. Killing the supervised process triggers a launchd respawn within `ThrottleInterval` (10 s); a manual `serve &` then races the respawn for port 7424 and the UDS socket, leaving the system in a confusing state. Use `launchctl kickstart -k` instead — it cleanly terminates and replaces the supervised process atomically.

If the LaunchAgent is not yet installed on a new machine, the `claude-history serve` binary still runs as a foreground process (per `crates/server/src/serve.rs:69-70`); installing the plist is what gives it supervision.

If seed queries in `queries/` were modified, copy them to `~/.claude/claude-history/queries/`.

## Binary

`claude-history` is on PATH via symlink: `~/.local/bin/claude-history` → `target/release/claude-history`. Rebuilding the release binary updates the symlink target automatically.

## Query Authoring

Run `claude-history queries --help` for the full database schema, table/column reference, views, and TOML sidecar format.

## MCP Server

The daemon exposes an MCP endpoint at `/mcp` (streamable HTTP transport). For Claude Desktop or stdio-based MCP clients:

```bash
claude-history mcp-stdio
```

10 tools: search_messages, list_sessions, query_messages, list_files, file_history, git_log, get_stats, execute_sql, run_query, list_queries.
