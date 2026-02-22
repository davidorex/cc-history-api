# cc-history-api

## Post-Build Protocol

After any `cargo build --release`: kill the running daemon and restart it. The daemon serves all other projects via HTTP and UDS. A stale daemon means stale behavior system-wide.

```bash
pgrep -f 'claude-history serve' | xargs kill 2>/dev/null
claude-history serve &
```

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
