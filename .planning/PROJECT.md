# claude-history

## What This Is

A Rust binary (`claude-history`) that exactly models Claude Code's JSONL session history, maintains a decomposed SQLite data store, watches for file changes in real time, actively monitors for schema drift across Claude Code versions, and exposes a stable API surface via HTTP (localhost), Unix domain socket, and CLI. Any language or process consumes the same interface. Zero runtime dependencies.

## Core Value

Universal, language-agnostic, queryable access to Claude Code's complete session history — messages, tool usage, file operations, git operations, token analytics — through a single binary that never discards data and actively detects when Claude Code's schema evolves.

## Requirements

### Validated

(None yet — ship to validate)

### Active

- [ ] Exact serde modeling of every JSONL record type with overflow capture for unknown fields
- [ ] Streaming JSONL parser with byte-offset awareness for incremental sync
- [ ] Normalized SQLite store (sessions, messages, content blocks, token usage, tool executions, agents, queue operations, progress events)
- [ ] Artifact layer: file operations table tracking every Write/Edit/Read/Bash that touches files
- [ ] Artifact layer: git operations table extracted from Bash tool calls matching git patterns
- [ ] File content reconstruction engine — replay writes and edits to reconstruct file state at any message point
- [ ] Incremental sync engine with byte-offset tracking (only process new bytes)
- [ ] FTS5 full-text search across message content and file contents
- [ ] File watcher via notify crate for real-time JSONL ingestion
- [ ] Bulk import: walk ~/.claude/projects/ and sync every .jsonl file
- [ ] HTTP API (axum) at /v1/ — sessions, messages, search, analytics, files, git, artifacts, schema drift
- [ ] Flexible query endpoints (POST /v1/messages/query, POST /v1/files/query) with parameterized SQL compilation
- [ ] SSE event stream for real-time events (record:added, file:written, git:commit, schema:drift)
- [ ] Unix domain socket serving the same HTTP API for lower-latency local consumers
- [ ] CLI interface: serve, sync, query, sessions, search, stats, export, files, git-log, reconstruct, artifacts
- [ ] Active version monitoring — detect Claude Code version changes, flag new schema fields
- [ ] Schema drift detection via serde(flatten) overflow fields logged to schema_drift_log table
- [ ] Embedded migrations (include_str!) with schema_version pragma tracking
- [ ] Tool result matching: link tool_use in assistant messages to tool_result in subsequent user messages by tool_use_id
- [ ] Daemon mode (claude-history serve) and one-shot mode (sync, query, etc.)

### Out of Scope

- Web UI / dashboard frontend — API consumers build their own
- Multi-user / remote access — localhost only, single-user
- Write-back to JSONL files — read-only ingestion
- Cloud sync / remote database — local SQLite only
- Windows support — macOS and Linux only (for initial release)

## Context

Claude Code stores session history as JSONL files under `~/.claude/projects/`. Each line is a discriminated record (type field: user, assistant, progress, queue-operation). Assistant messages contain content blocks (text, thinking, tool_use, tool_result). Tool results live in the subsequent user message, linked by tool_use_id.

The JSONL schema is undocumented and evolves with Claude Code releases. The serde overflow pattern (`#[serde(flatten)] pub overflow: HashMap<String, Value>`) captures unknown fields without breaking deserialization, enabling active drift detection.

The artifact layer (files, file_operations, git_operations tables) provides structured access to what the model actually produced — every file touched, every edit made, every git commit — derived from tool_use content blocks.

## Constraints

- **Language**: Rust — single binary, zero runtime dependencies, ~5-10MB
- **Storage**: SQLite via rusqlite with bundled + fts5 features
- **HTTP**: axum on tokio async runtime
- **File watching**: notify crate
- **CLI**: clap
- **Workspace**: Cargo workspace with 3 crates — core (types, parser, version), store (SQLite, decomposition, sync, watcher, FTS), server (HTTP, CLI, events)
- **DB location**: `$CLAUDE_HISTORY_DB_PATH` or `~/.claude/.claude-history.db`
- **Socket**: `$CLAUDE_HISTORY_SOCKET` or `/tmp/claude-history.sock`
- **Default port**: 7424

## Key Decisions

| Decision | Rationale | Outcome |
|----------|-----------|---------|
| Rust over TypeScript | Single binary, zero deps, serde enum system maps directly to JSONL discriminated records, zero-cost drift capture | -- Pending |
| Cargo workspace (3 crates) | Separation of concerns: core types are reusable, store is the engine, server is the interface | -- Pending |
| serde(flatten) overflow on every struct | Never discard unknown fields; enables active schema drift detection | -- Pending |
| Byte-offset incremental sync | Only read new bytes from JSONL files; append-only nature of session logs makes this safe | -- Pending |
| Tool result matching via buffered assistant message | tool_use is in assistant msg, tool_result is in next user msg — decomposer buffers and matches by tool_use_id | -- Pending |
| Artifact layer as secondary decomposition pass | File/git operations extracted from tool_use blocks in same transaction as message decomposition | -- Pending |
| File content reconstruction via operation replay | Replay writes + edits in timestamp order to reconstruct file state at any point — session-derived version control | -- Pending |

---
*Last updated: 2026-02-20 after initialization*
