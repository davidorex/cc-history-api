# Roadmap: claude-history

## Overview

claude-history delivers universal queryable access to Claude Code's session history through a single Rust binary. The build follows the Cargo workspace dependency graph bottom-up: pure types and ingestion pipeline first, then search and CLI for immediate usability, then HTTP API and daemon infrastructure, then real-time capabilities, then the artifact layer (the hardest and most differentiated feature), and finally version monitoring polish. Each phase produces a testable, independently verifiable capability.

## Phases

**Phase Numbering:**
- Integer phases (1, 2, 3): Planned milestone work
- Decimal phases (2.1, 2.2): Urgent insertions (marked with INSERTED)

Decimal phases appear between their surrounding integers in numeric order.

- [x] **Phase 1: Core Types and Ingestion Pipeline** - Serde type modeling, JSONL parser, SQLite schema, record decomposer, incremental sync, bulk import
- [x] **Phase 2: Full-Text Search and CLI** - FTS5 search indexes, CLI interface for querying and exporting session data
- [x] **Phase 3: HTTP API and Daemon** - axum HTTP API at /v1/, Unix domain socket, daemon mode with graceful shutdown
- [x] **Phase 4: Real-Time Ingestion and Events** - File watcher for live JSONL changes, SSE event stream for connected consumers
- [x] **Phase 5: Artifact Layer** - File operation tracking, git operation extraction, tool result matching, content reconstruction, artifact API and CLI
- [ ] **Phase 6: Version Monitoring** - Active Claude Code version detection, schema drift analysis across versions

## Phase Details

### Phase 1: Core Types and Ingestion Pipeline
**Goal**: A working ingestion pipeline that reads any Claude Code JSONL file and decomposes every record into normalized SQLite tables, with incremental sync and overflow capture for unknown fields
**Depends on**: Nothing (first phase)
**Requirements**: CORE-01, CORE-02, CORE-03, CORE-04, CORE-05, CORE-06, CORE-07, STORE-01, STORE-02, STORE-03, STORE-04, STORE-05, STORE-06, DECOMP-01, DECOMP-02, DECOMP-03, DECOMP-04, DECOMP-05, DECOMP-06, SYNC-01, SYNC-02, SYNC-03, SYNC-04, INFRA-01, INFRA-02, INFRA-03, INFRA-07
**Success Criteria** (what must be TRUE):
  1. Running `claude-history sync` against a real `~/.claude/projects/` directory parses every JSONL file and populates sessions, messages, message_content, token_usage, tool_executions, agents, queue_operations, and progress_events tables with correct data
  2. Running sync a second time on the same files processes zero records (byte-offset incremental sync skips already-ingested data)
  3. Malformed JSONL lines produce logged warnings but do not halt ingestion -- all valid records in the same file are still decomposed
  4. Unknown fields in JSONL records (simulated or real) appear in the schema_drift_log table with field name, sample value, and source context
  5. The SQLite database uses WAL mode, embedded migrations track schema version, and the Cargo workspace compiles to a single binary
**Plans**: 4 plans

Plans:
- [x] 01-01-PLAN.md — Cargo workspace + SQLite schema + migrations + DB initialization
- [x] 01-02-PLAN.md — Serde types for all 7 JSONL record types + JSONL parser with byte-offset tracking
- [x] 01-03-PLAN.md — Record decomposition engine + schema drift logger
- [x] 01-04-PLAN.md — Sync engine + CLI sync subcommand (end-to-end integration)

### Phase 2: Full-Text Search and CLI
**Goal**: Users can search across all ingested session content and interact with their history through a complete CLI that answers questions about sessions, messages, tokens, and tools
**Depends on**: Phase 1
**Requirements**: FTS-01, FTS-03, CLI-02, CLI-03, CLI-04, CLI-05, CLI-06, CLI-07, CLI-08, CLI-09
**Note**: FTS-02 (file_operations FTS index) moved to Phase 5 — the file_operations table it indexes is created there.
**Success Criteria** (what must be TRUE):
  1. `claude-history search "some phrase"` returns ranked results from across all sessions, matching against message content via FTS5 (file operation content search extends this in Phase 5 when FTS-02 and the file_operations table are created)
  2. `claude-history sessions` lists sessions with filters (project, date range, status) and `claude-history stats` shows token usage, tool frequency, and model breakdown
  3. `claude-history export <session-id>` produces valid JSON, Markdown, or CSV output of a complete session conversation
  4. `claude-history query` accepts filter arguments and outputs matching messages as JSON to stdout
**Plans**: 3 plans

Plans:
- [x] 02-01-PLAN.md — FTS5 index + store-layer search and query functions
- [x] 02-02-PLAN.md — CLI subcommands: search, sessions, query, stats
- [x] 02-03-PLAN.md — CLI subcommands: export, version-check, schema-drift

### Phase 3: HTTP API and Daemon
**Goal**: Any language or process can query Claude Code history through a stable HTTP API at localhost:7424 or a Unix domain socket, with the daemon managing its own lifecycle
**Depends on**: Phase 2
**Requirements**: API-01, API-02, API-03, API-04, API-05, API-06, API-07, API-08, API-09, API-10, API-11, API-12, API-13, API-14, API-15, API-16, UDS-01, UDS-02, INFRA-04, INFRA-05, INFRA-06, CLI-01, CLI-15
**Success Criteria** (what must be TRUE):
  1. `curl http://localhost:7424/v1/health` returns status, db_size, record_count, and version after running `claude-history serve`
  2. GET endpoints for sessions, messages, search, analytics, and schema return correct JSON responses matching the data visible through the CLI
  3. POST /v1/messages/query accepts a structured query body and returns filtered results with parameterized SQL compilation (no injection)
  4. The same API is accessible over the Unix domain socket at /tmp/claude-history.sock (or configured path), and CLI commands automatically connect to the daemon socket when available
  5. `claude-history serve` runs as a foreground daemon with graceful shutdown on SIGTERM/SIGINT -- in-flight requests complete, no data loss
**Plans**: 6 plans

Plans:
- [x] 03-01-PLAN.md — Foundation: workspace deps, AppState, ApiError, and 7 new store query functions
- [x] 03-02-PLAN.md — API handlers: health, sessions, messages, search (10 endpoints)
- [x] 03-03-PLAN.md — API handlers: analytics, export, schema (6 endpoints) + complete router
- [x] 03-04-PLAN.md — Dual-listener serve: TCP + Unix domain socket with graceful shutdown
- [x] 03-05-PLAN.md — HTTP-over-UDS client: DaemonClient and ConnectionMode abstraction
- [x] 03-06-PLAN.md — CLI-15: Wire all read-only CLI subcommands through daemon when socket is available

### Phase 4: Real-Time Ingestion and Events
**Goal**: The daemon automatically ingests new JSONL data as Claude Code writes it, and connected consumers receive events in real time
**Depends on**: Phase 3
**Requirements**: WATCH-01, WATCH-02, WATCH-03, SSE-01, SSE-02, SSE-03, SSE-04, SSE-05
**Success Criteria** (what must be TRUE):
  1. While the daemon is running, starting a new Claude Code session causes the session and its messages to appear in API/CLI query results within seconds -- without manual sync
  2. A client connected to GET /v1/events receives record:added and session:started SSE events as new JSONL data is written by Claude Code
  3. schema:drift and version:changed SSE events fire when new overflow fields or Claude Code version changes are detected during live ingestion
  4. The file watcher debounces rapid writes (minimum 2-second gap per file) and recovers gracefully from transient filesystem errors
**Plans**: 2 plans

Plans:
- [x] 04-01-PLAN.md — SSE event types, broadcast channel in AppState, GET /v1/events endpoint
- [x] 04-02-PLAN.md — File watcher module with debounce, sync triggering, SSE event emission, version detection

### Phase 5: Artifact Layer
**Goal**: Users can query what files Claude Code touched, what git operations it performed, reconstruct file content at any point in a session, and view a unified timeline of all artifacts produced
**Depends on**: Phase 4
**Requirements**: FTS-02, ART-01, ART-02, ART-03, ART-04, ART-05, ART-06, ART-07, ART-08, ART-09, ART-10, ART-11, API-17, API-18, API-19, API-20, API-21, API-22, API-23, API-24, API-25, API-26, API-27, CLI-10, CLI-11, CLI-12, CLI-13, CLI-14, SSE-06, SSE-07
**Success Criteria** (what must be TRUE):
  1. `claude-history files` lists every file touched by Claude Code across sessions, and `claude-history file-history <path>` shows the chronological Write/Edit/Read operations on that file with content
  2. `claude-history reconstruct <file-path> --at <message-uuid>` replays writes and edits to produce the file's content as it existed at that point in the session
  3. `claude-history git-log` shows git operations extracted from Bash tool calls, with commit messages, branches, and operation types correctly parsed
  4. GET /v1/artifacts/:session_id/timeline returns a chronological feed of all file writes, edits, git commits, and tool outputs for a session
  5. tool_use blocks in assistant messages are correctly linked to their tool_result blocks in subsequent user messages by tool_use_id, and file:written / git:commit SSE events fire during live ingestion
**Plans**: 8 plans

Plans:
- [x] 05-01-PLAN.md — Migration 003 (files, file_operations, git_operations tables + FTS5) and workspace deps
- [x] 05-02-PLAN.md — Artifact decomposer: Write/Edit/Read/Bash parsing into artifact tables
- [x] 05-03-PLAN.md — Tool result matching (ART-04) and retroactive artifact decomposition
- [x] 05-04-PLAN.md — Artifact queries: list files, reconstruction via edit replay, unified diffs
- [x] 05-05-PLAN.md — FTS5 file_operations index: rebuild and search functions
- [x] 05-06-PLAN.md — HTTP API handlers: files, git, artifacts (11 new endpoints)
- [x] 05-07-PLAN.md — CLI subcommands: files, file-history, reconstruct, git-log, artifacts
- [x] 05-08-PLAN.md — SSE events (file:written, file:edited, git:commit) and watcher integration

### Phase 6: Version Monitoring
**Goal**: The daemon actively tracks Claude Code version changes and provides actionable schema drift analysis across versions
**Depends on**: Phase 4
**Requirements**: VER-01, VER-02, VER-03, VER-04
**Success Criteria** (what must be TRUE):
  1. `claude-history version-check` (and GET /v1/schema/versions) shows the detected Claude Code version and a history of version changes with timestamps
  2. GET /v1/schema/drift shows overflow fields grouped by version, highlighting new fields that appeared when Claude Code updated
  3. In daemon mode, a periodic check loop detects version changes and logs them to schema_versions without requiring a restart
**Plans**: TBD

Plans:
- [ ] 06-01: TBD

## Progress

**Execution Order:**
Phases execute in numeric order: 1 -> 2 -> 3 -> 4 -> 5 -> 6
(Phase 5 and Phase 6 both depend on Phase 4 and could execute in either order)

| Phase | Plans Complete | Status | Completed |
|-------|----------------|--------|-----------|
| 1. Core Types and Ingestion Pipeline | 4/4 | Complete | 2026-02-20 |
| 2. Full-Text Search and CLI | 3/3 | Complete | 2026-02-20 |
| 3. HTTP API and Daemon | 6/6 | Complete | 2026-02-20 |
| 4. Real-Time Ingestion and Events | 2/2 | Complete | 2026-02-20 |
| 5. Artifact Layer | 8/8 | Complete | 2026-02-20 |
| 6. Version Monitoring | 0/1 | Not started | - |
