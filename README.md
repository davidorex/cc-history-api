# cc-history-api

Ingests Claude Code JSONL session files into a normalized SQLite database and serves them via CLI, HTTP API, and MCP tools.

## What it captures

From each session JSONL (`~/.claude/projects/<encoded-dir>/<session-id>.jsonl` plus `*/subagents/agent-*.jsonl`):

- Conversation messages (user, assistant, system) with parent_uuid graph
- Tool calls (Bash, Read, Edit, Write, etc.) and their results
- File operations (read / write / edit) with content snapshots
- Git operations extracted from Bash invocations
- Token usage and model identification
- Schema-drift events (unknown JSON fields and unknown record-type discriminators)
- Attachment records (hook executions, mcp_instructions_delta, skill_listing, edited_text_file, plan-mode, etc.)
- Plan-mode markdown bodies (`user.planContent`)

## Build

```bash
cargo build --release
```

The single binary lands at `target/release/claude-history`. Symlink it to `~/.local/bin/`:

```bash
ln -sf "$PWD/target/release/claude-history" ~/.local/bin/claude-history
```

For supervised daemon operation on macOS, install the user-level launchd LaunchAgent — see `CLAUDE.md` for the protocol. Without supervision, `claude-history serve` still runs as a foreground process.

## Surfaces

- **CLI**: 21 subcommands (`claude-history --help`). Major: `sync`, `search`, `sessions`, `query`, `files`, `file-history`, `git-log`, `stats`, `export`, `attachments`, `plans`, `hook-executions`, `queries`, `record-type-drift`, `schema-drift`, `version-check`, `reconstruct`, `artifacts`, `mcp-stdio`, `mcp-config`, `serve`. Most subcommands accept `--json` for machine-readable output.
- **HTTP API**: 39 routes under `/v1/*` served on `127.0.0.1:7424` and a Unix domain socket at `/tmp/claude-history.sock`. Resource groups: sessions, messages, search, analytics, export, schema, projects, sql, files, git, artifacts, attachments, hook-executions, plans, events, health.
- **MCP**: 17 tools at `/mcp` (streamable HTTP) or via `claude-history mcp-stdio` (Claude Desktop). Tool surface enumerated in `CLAUDE.md`. Run `claude-history mcp-config` for client configuration snippets.

## Architecture

Cargo workspace, three crates:

| Crate | Role |
|---|---|
| `claude-history-core` | JSONL record types (`JSONLRecord` enum + variants), manual `Deserialize` impls with two-pass dispatch, `AttachmentBody` enum (12 modeled subtypes + Unknown catch-all). |
| `claude-history-store` | SQLite schema (11 migrations), decomposer (record → typed table rows), drift logging, FTS5 indices (`fts_message_content`, `fts_file_operations`, `fts_attachment_text_content`), query functions, canned-query loader. |
| `claude-history` | CLI dispatch, HTTP API (axum), MCP service, filesystem watcher (tokio + notify-rs), daemon supervision shim, daemon-client HTTP wrappers. |

Storage: SQLite WAL mode with bundled rusqlite. Database lives at `~/.claude/.claude-history.db` by default (override with `CLAUDE_HISTORY_DB_PATH` or `--db-path`).

## Schema

Run `claude-history queries --help` for the canonical schema reference: tables, columns, indices, FTS5 virtual tables, analytical views, and TOML sidecar format for canned queries.

Quick orientation:

- Core tables: `sessions`, `messages`, `message_content`, `tool_executions`, `files`, `file_operations`, `git_operations`, `token_usage`, `attachments`, `hook_executions`.
- Drift tables: `schema_drift_log` (field-level), `record_type_drift_log` (discriminator-level).
- FTS5: `fts_message_content`, `fts_file_operations`, `fts_attachment_text_content`.
- 7 analytical views: `v_file_token_cost`, `v_file_conversation_context`, `v_project_summary`, `v_file_provenance`, `v_git_commit_context`, `v_tool_errors`, `v_session_cost`.

## Sync model

`sync_metadata.last_byte_offset` per JSONL file tracks ingestion progress. Re-running `sync` is idempotent; new bytes are appended to existing tables (uuid-keyed PKs with `INSERT OR IGNORE`). The daemon's filesystem watcher debounces and re-syncs files as Claude Code writes new turns. On daemon startup, `sync_all` catches up any sessions missed while the daemon was down.

A scoped `UPDATE sync_metadata SET last_byte_offset = 0 WHERE file_path IN (...)` followed by re-running `sync` re-decomposes specific files through the current decomposer code path — the recovery mechanism for cases where new typed paths have been added since last ingestion.

## Canned queries

User-facing canned queries live in `~/.claude/claude-history/queries/` as `.sql` + optional `.toml` sidecar pairs. Seeds in this repo's `queries/` directory must be copied to the user directory to be available at runtime.

```bash
claude-history queries list
claude-history queries run <name> --param key=value
```

## Development

See `CLAUDE.md` for:

- Post-build daemon-restart protocol (`launchctl kickstart -k`)
- Daemon supervision via launchd LaunchAgent
- Anti-pattern callout against manual `pgrep + kill + serve &` recipes
- Seed-query copy step

## Known Limitations

- **Historical attachment records require one-time backfill.** The typed `attachments` and `hook_executions` tables are populated by ingestion through the current decomposer. Records ingested before the typed Attachment surface existed remain in `record_type_drift_log` only and do not appear in `attachments` / `hook_executions` / `fts_attachment_text_content`. A bytewise re-ingestion procedure (scope-reset `sync_metadata.last_byte_offset` for affected files, then run `claude-history sync`) populates the typed tables retroactively. Procedure outlined at `.planning/audit/c1-attachments-backfill-procedure-summary-2026-05-10T0712-asia-shanghai.md`.

- **Inner content-block discriminator has no unknown-variant catch-all.** The `ContentBlock` enum (text / thinking / tool_use / tool_result) uses serde's default tagged-enum derive. Records with content-block types outside the four known discriminators (e.g., a future `image` or `video` block) fail deserialization at the parent-message level and route through the `JSONLRecord::Unknown` outer-level catch-all, losing the typed envelope. Resolution requires manual two-pass `Deserialize` on `ContentBlock` analogous to the outer-level fix.

- **Supervised daemon operation is documented for macOS only.** The daemon-supervision protocol in `CLAUDE.md` uses launchd. Linux operation via systemd or other supervisors is untested. The `claude-history serve` binary itself runs as a foreground process on any Unix-like system.

- **Claude Desktop MCPB extension may lag the source binary.** The `mcpb/manifest.json` references a bundled binary at `mcpb/bin/claude-history`. Updates to the source do not auto-propagate to Claude Desktop; the `.mcpb` archive must be rebuilt and re-installed manually via Claude Desktop → Settings → Extensions.

## License

MIT. See [LICENSE](LICENSE).

