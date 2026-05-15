# cc-history-api

Ingests Claude Code JSONL session files into a normalized SQLite database and serves them via CLI, HTTP API, and MCP tools.

I use it as an archaeological tool to surface historical intentions, decisions, and actions to inform development of projects.

- [What it captures](#what-it-captures)
- [Features](#features)
- [Build](#build)
- [Surfaces](#surfaces)
- [Architecture](#architecture)
- [Schema](#schema)
- [Sync model](#sync-model)
- [Canned queries](#canned-queries)
- [Daemon supervision (macOS)](#daemon-supervision-macos)
- [Known Limitations](#known-limitations)
- [License](#license)

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
- Session metadata: start/end timestamps, project path, Claude Code version per turn, model used per turn
- Subagent session records from `*/subagents/agent-*.jsonl` with parent-session linkage
- Compaction summary records (`is_compact_summary`-flagged user messages)

## Features

**Ingestion**

- Filesystem watcher syncs JSONL writes within ~5 seconds (debounced); runs foreground or under macOS launchd supervision
- Manual `claude-history sync` catches up from any state; idempotent via UUID-keyed `INSERT OR IGNORE` — no duplicate rows on re-run
- Bytewise re-ingestion: scoped `sync_metadata.last_byte_offset` reset re-decodes chosen files through the current code path (recovery for newly-typed records, schema fixes, etc.)

**Search**

- Full-text search across message content via FTS5 with BM25 ranking and `>>>highlight<<<` snippets
- Cross-source unified search: results tagged `{kind: "message"}` or `{kind: "attachment", subtype: "..."}`
- Search restricted to plan-mode markdown only via `/v1/plans/search`
- Search across file operations (path + content)
- FTS5 query syntax: AND, OR, NOT, `"phrase match"`, `prefix*`

**Listing / browsing**

- Surfaces for sessions, messages, files, file history, git operations, attachments, hook executions, plans
- Filter axes by surface: project (substring), date range, model, tool, `has_plan`, `inner_type`, `hook_event`, `exit_code`, `tool_use_id`
- Output: human-readable tables or `--json` for machine parsing

**Analytics**

- Token usage breakdown by model, tool, date range
- Tool frequency stats
- 7 analytical SQL views (`v_file_token_cost`, `v_file_conversation_context`, `v_project_summary`, `v_file_provenance`, `v_git_commit_context`, `v_tool_errors`, `v_session_cost`)

**Recovery / reconstruction**

- File content reconstruction at a point in time from accumulated read/edit operations
- Session export as markdown or JSON; synthetic plan_content rows filtered from export output
- Combined artifacts view: files + git operations per session

**Drift detection**

- Field-level: unknown JSON fields captured to `schema_drift_log` with per-version occurrence counts and sample values
- Discriminator-level: unknown JSONL record-`type` values captured to `record_type_drift_log` (outer-level catch-all via `JSONLRecord::Unknown`; namespaced inner subtypes like `attachment.<subtype>` for unmodeled attachment shapes)
- Version-change events tracked in `version_history`; `claude-history version-check` surfaces Claude Code version progression
- CLI + REST + MCP surfaces for both drift logs

**Query authoring**

- Read-only SQL passthrough via `execute_sql` MCP tool or `claude-history queries run`
- Canned queries: `.sql` + optional `.toml` sidecar pairs (param type hints `integer` / `real` for numeric comparisons)
- Schema reference: `claude-history queries --help`

**Bookmarks**

- Read-only access to bookmarks created in **ClaudeHistoryBrowser (CHB)** — a separate macOS Core Data app, not part of this repo. CHB lets users save assistant messages with labels and tags while browsing session history.
- The CHB database lives at `~/.claude/cache/chb/ClaudeHistory.sqlite`, independent of cc-history-api's `~/.claude/.claude-history.db`. Bookmarks survive rebuilds of the session-history DB. If CHB is not installed, the bookmark queries return empty / "not found".
- 3 MCP tools: `list_bookmarks`, `search_bookmarks`, `get_bookmark`

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
- **MCP**: 17 tools at `/mcp` (streamable HTTP) or via `claude-history mcp-stdio` (Claude Desktop). Tools: `search_messages`, `list_sessions`, `query_messages`, `list_files`, `file_history`, `git_log`, `get_stats`, `execute_sql`, `run_query`, `list_queries`, `list_bookmarks`, `search_bookmarks`, `get_bookmark`, `list_attachments`, `get_hook_executions`, `list_plans`, `get_plan`. Run `claude-history mcp-config` for client configuration snippets.

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

User-facing canned queries live in `~/.claude/claude-history/queries/` as `.sql` + optional `.toml` sidecar pairs.

```bash
claude-history queries list
claude-history queries run <name> --param key=value
```

## Daemon supervision (macOS)

- Default: `claude-history serve` runs as a foreground process (per `run_server` in `crates/server/src/serve.rs`)
- Supervised operation that survives terminal close / logout / reboot: install a user-level launchd LaunchAgent
- Author's instance uses label `com.davidrex.claude-history` at `~/Library/LaunchAgents/com.davidrex.claude-history.plist` — substitute your own label / path; commands below assume you have

```bash
launchctl list | grep claude-history                                    # status
launchctl kickstart -k gui/$(id -u)/<your-label>                        # restart (e.g., after `cargo build --release`)
launchctl unload   -w  ~/Library/LaunchAgents/<your-label>.plist        # disable persistently
launchctl load     -w  ~/Library/LaunchAgents/<your-label>.plist        # re-enable
tail -f ~/Library/Logs/claude-history.err.log                           # live structured logs (tracing crate, stderr)
```

- **After `cargo build --release`**: restart the daemon (`launchctl kickstart -k`) so the new binary is mmapped. Stale daemon = stale behavior across all CLI / HTTP / MCP / UDS clients.
- **Anti-pattern**: do NOT `pgrep -f 'claude-history serve' | xargs kill` then `claude-history serve &`. Killing the supervised process triggers a launchd respawn within `ThrottleInterval` (10 s); manual `serve &` then races the respawn for port 7424 and the UDS socket. Use `launchctl kickstart -k` — it cleanly terminates and replaces the supervised process atomically.
- Linux operation via systemd or other supervisors is untested; the foreground `serve` binary works on any Unix-like system

## Known Limitations

- **Historical attachment records require one-time backfill.** The typed `attachments` and `hook_executions` tables are populated by ingestion through the current decomposer. Records ingested before the typed Attachment surface existed remain in `record_type_drift_log` only and do not appear in `attachments` / `hook_executions` / `fts_attachment_text_content`. A bytewise re-ingestion procedure (scope-reset `sync_metadata.last_byte_offset` for affected files, then run `claude-history sync`) populates the typed tables retroactively. Procedure outlined at `.planning/audit/c1-attachments-backfill-procedure-summary-2026-05-10T0712-asia-shanghai.md`.

- **Inner content-block discriminator has no unknown-variant catch-all.** The `ContentBlock` enum (text / thinking / tool_use / tool_result) uses serde's default tagged-enum derive. Records with content-block types outside the four known discriminators (e.g., a future `image` or `video` block) fail deserialization at the parent-message level and route through the `JSONLRecord::Unknown` outer-level catch-all, losing the typed envelope. Resolution requires manual two-pass `Deserialize` on `ContentBlock` analogous to the outer-level fix.

- **Supervised daemon operation is documented for macOS only.** The daemon-supervision protocol in `CLAUDE.md` uses launchd. Linux operation via systemd or other supervisors is untested. The `claude-history serve` binary itself runs as a foreground process on any Unix-like system.

- **Claude Desktop MCPB extension may lag the source binary.** The `mcpb/manifest.json` references a bundled binary at `mcpb/bin/claude-history`. Updates to the source do not auto-propagate to Claude Desktop; the `.mcpb` archive must be rebuilt and re-installed manually via Claude Desktop → Settings → Extensions.

- **Seed canned queries do not auto-sync.** Seed `.sql` / `.toml` pairs in this repo's `queries/` directory are not automatically installed to the user-facing location at `~/.claude/claude-history/queries/`. Changes to seeds require manual copy; there is no install or sync mechanism.

Produced and directed by me, coded by Claude Code. 

## License

MIT. See [LICENSE](LICENSE).

