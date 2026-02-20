# Roadmap: claude-history

## Overview

claude-history delivers universal queryable access to Claude Code's session history through a single Rust binary. The build follows the Cargo workspace dependency graph bottom-up: pure types and ingestion pipeline first, then search and CLI for immediate usability, then HTTP API and daemon infrastructure, then real-time capabilities, then the artifact layer (the hardest and most differentiated feature), and finally version monitoring polish. Each phase produces a testable, independently verifiable capability.

## Phases

**Phase Numbering:**
- Integer phases (1, 2, 3): Planned milestone work
- Decimal phases (2.1, 2.2): Urgent insertions (marked with INSERTED)

Decimal phases appear between their surrounding integers in numeric order.

- [x] **Phase 1: Core Types and Ingestion Pipeline** - Serde type modeling, JSONL parser, SQLite schema, record decomposer, incremental sync, bulk import
- [ ] **Phase 2: Full-Text Search and CLI** - FTS5 search indexes, CLI interface for querying and exporting session data
- [ ] **Phase 3: HTTP API and Daemon** - axum HTTP API at /v1/, Unix domain socket, daemon mode with graceful shutdown
- [ ] **Phase 4: Real-Time Ingestion and Events** - File watcher for live JSONL changes, SSE event stream for connected consumers
- [ ] **Phase 5: Artifact Layer** - File operation tracking, git operation extraction, tool result matching, content reconstruction, artifact API and CLI
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
**Requirements**: FTS-01, FTS-02, FTS-03, CLI-02, CLI-03, CLI-04, CLI-05, CLI-06, CLI-07, CLI-08, CLI-09
**Success Criteria** (what must be TRUE):
  1. `claude-history search "some phrase"` returns ranked results from across all sessions, matching against message content and file operation content via FTS5
  2. `claude-history sessions` lists sessions with filters (project, date range, status) and `claude-history stats` shows token usage, tool frequency, and model breakdown
  3. `claude-history export <session-id>` produces valid JSON, Markdown, or CSV output of a complete session conversation
  4. `claude-history query` accepts filter arguments and outputs matching messages as JSON to stdout
**Plans**: TBD

Plans:
- [ ] 02-01: TBD
- [ ] 02-02: TBD

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
**Plans**: TBD

Plans:
- [ ] 03-01: TBD
- [ ] 03-02: TBD
- [ ] 03-03: TBD

### Phase 4: Real-Time Ingestion and Events
**Goal**: The daemon automatically ingests new JSONL data as Claude Code writes it, and connected consumers receive events in real time
**Depends on**: Phase 3
**Requirements**: WATCH-01, WATCH-02, WATCH-03, SSE-01, SSE-02, SSE-03, SSE-04, SSE-05
**Success Criteria** (what must be TRUE):
  1. While the daemon is running, starting a new Claude Code session causes the session and its messages to appear in API/CLI query results within seconds -- without manual sync
  2. A client connected to GET /v1/events receives record:added and session:started SSE events as new JSONL data is written by Claude Code
  3. schema:drift and version:changed SSE events fire when new overflow fields or Claude Code version changes are detected during live ingestion
  4. The file watcher debounces rapid writes (minimum 2-second gap per file) and recovers gracefully from transient filesystem errors
**Plans**: TBD

Plans:
- [ ] 04-01: TBD
- [ ] 04-02: TBD

### Phase 5: Artifact Layer
**Goal**: Users can query what files Claude Code touched, what git operations it performed, reconstruct file content at any point in a session, and view a unified timeline of all artifacts produced
**Depends on**: Phase 4
**Requirements**: ART-01, ART-02, ART-03, ART-04, ART-05, ART-06, ART-07, ART-08, ART-09, ART-10, ART-11, API-17, API-18, API-19, API-20, API-21, API-22, API-23, API-24, API-25, API-26, API-27, CLI-10, CLI-11, CLI-12, CLI-13, CLI-14, SSE-06, SSE-07
**Success Criteria** (what must be TRUE):
  1. `claude-history files` lists every file touched by Claude Code across sessions, and `claude-history file-history <path>` shows the chronological Write/Edit/Read operations on that file with content
  2. `claude-history reconstruct <file-path> --at <message-uuid>` replays writes and edits to produce the file's content as it existed at that point in the session
  3. `claude-history git-log` shows git operations extracted from Bash tool calls, with commit messages, branches, and operation types correctly parsed
  4. GET /v1/artifacts/:session_id/timeline returns a chronological feed of all file writes, edits, git commits, and tool outputs for a session
  5. tool_use blocks in assistant messages are correctly linked to their tool_result blocks in subsequent user messages by tool_use_id, and file:written / git:commit SSE events fire during live ingestion
**Plans**: TBD

Plans:
- [ ] 05-01: TBD
- [ ] 05-02: TBD
- [ ] 05-03: TBD

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
| 2. Full-Text Search and CLI | 0/2 | Not started | - |
| 3. HTTP API and Daemon | 0/3 | Not started | - |
| 4. Real-Time Ingestion and Events | 0/2 | Not started | - |
| 5. Artifact Layer | 0/3 | Not started | - |
| 6. Version Monitoring | 0/1 | Not started | - |
