# cc-history-api

## Post-Build Protocol

After any `cargo build --release`: kill the running daemon and restart it. The daemon serves all other projects via HTTP and UDS. A stale daemon means stale behavior system-wide.

```bash
pgrep -f 'claude-history serve' | xargs kill 2>/dev/null
/Users/david/Projects/cc-history-api/target/release/claude-history serve &
```

If seed queries in `queries/` were modified, copy them to `~/.claude/claude-history/queries/`.

## Query Authoring

Run `claude-history queries --help` for the full database schema, table/column reference, views, and TOML sidecar format.
