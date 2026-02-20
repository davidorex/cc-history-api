# Requirements: claude-history

**Defined:** 2026-02-20
**Core Value:** Universal, language-agnostic, queryable access to Claude Code's complete session history through a single binary that never discards data and actively detects schema evolution.

## v1 Requirements

Requirements for initial release. Each maps to roadmap phases.

### Core Types & Parsing

- [ ] **CORE-01**: Exact serde modeling of every JSONL record type (user, assistant, progress, queue-operation) with discriminated union via serde(tag = "type")
- [ ] **CORE-02**: serde(flatten) overflow capture on every struct with variable shape — unknown fields land in HashMap<String, Value>
- [ ] **CORE-03**: Content block modeling: text, thinking, tool_use, tool_result as tagged enum
- [ ] **CORE-04**: MessageContent as untagged enum (plain string or array of blocks)
- [ ] **CORE-05**: UsageStats with overflow capture for cache_creation subfields and unknown billing fields
- [ ] **CORE-06**: Streaming JSONL parser with byte-offset awareness — parse from arbitrary offset, return new offset
- [ ] **CORE-07**: Per-line error isolation — malformed lines produce warnings, never halt ingestion

### SQLite Store

- [ ] **STORE-01**: Normalized schema: sessions, messages, message_content, token_usage, tool_executions, agents, queue_operations, progress_events tables
- [ ] **STORE-02**: sync_metadata table tracking per-file byte offset, record count, last sync timestamp
- [ ] **STORE-03**: schema_versions table for embedded migration tracking
- [ ] **STORE-04**: schema_drift_log table capturing overflow fields with version, field_name, first_seen, sample_value
- [ ] **STORE-05**: Embedded migrations via include_str! with schema_version pragma
- [ ] **STORE-06**: WAL mode enabled, busy timeout configured for concurrent read/write

### Artifact Layer

- [ ] **ART-01**: files table — one row per unique file path per session, tracking first_seen, last_modified, operation_count
- [ ] **ART-02**: file_operations table — every Write/Edit/Read/Bash file touch with content, old_content, command, result_summary, is_error
- [ ] **ART-03**: git_operations table — extracted from Bash commands matching git patterns, with operation_type, command, commit_message, branch
- [ ] **ART-04**: Tool result matching — link tool_use from assistant message to tool_result in subsequent user message by tool_use_id
- [ ] **ART-05**: Artifact decomposer: parse Write tool_use for file_path + content → file_operations insert
- [ ] **ART-06**: Artifact decomposer: parse Edit tool_use for file_path + old_string + new_string → file_operations insert
- [ ] **ART-07**: Artifact decomposer: parse Read tool_use for file_path → file_operations insert (no mutation)
- [ ] **ART-08**: Artifact decomposer: parse Bash tool_use for git commands → git_operations insert with commit message/branch extraction
- [ ] **ART-09**: Artifact decomposer: parse Bash tool_use for file-touching commands (cp, mv, rm, mkdir, touch) → file_operations insert
- [ ] **ART-10**: File content reconstruction — replay writes + edits in timestamp order to reconstruct file state at any message point
- [ ] **ART-11**: Diff generation — unified diff of all edits to a file in order

### Record Decomposition

- [ ] **DECOMP-01**: Decompose user messages → messages + message_content rows
- [ ] **DECOMP-02**: Decompose assistant messages → messages + message_content (per block) + token_usage + tool_executions rows
- [ ] **DECOMP-03**: Decompose progress records → progress_events rows
- [ ] **DECOMP-04**: Decompose queue-operation records → queue_operations rows
- [ ] **DECOMP-05**: Log overflow fields to schema_drift_log during decomposition
- [ ] **DECOMP-06**: All decomposition in a single transaction per sync batch

### Sync & Ingestion

- [ ] **SYNC-01**: Incremental sync — read only new bytes from JSONL file using stored byte offset
- [ ] **SYNC-02**: Bulk import — walk ~/.claude/projects/ recursively, sync every .jsonl file found
- [ ] **SYNC-03**: Batch transactions — wrap multiple record decompositions in single SQLite transaction
- [ ] **SYNC-04**: sync_metadata updated atomically with record insertion

### Full-Text Search

- [ ] **FTS-01**: FTS5 virtual table over message_content text
- [ ] **FTS-02**: FTS5 virtual table over file_operations content, old_content, command
- [ ] **FTS-03**: Search endpoint returning ranked results across all indexed content

### File Watching

- [ ] **WATCH-01**: notify crate watching ~/.claude/projects/ recursively for .jsonl changes
- [ ] **WATCH-02**: Debounced event processing (2-second minimum between syncs per file)
- [ ] **WATCH-03**: notify watcher in blocking thread, events forwarded via tokio channel

### HTTP API

- [ ] **API-01**: GET /v1/health — status, db_size, record_count, version
- [ ] **API-02**: GET /v1/sessions — list with filters (status, project, after, before, limit)
- [ ] **API-03**: GET /v1/sessions/:id — single session detail
- [ ] **API-04**: GET /v1/sessions/:id/conversation — ordered messages with optional thinking/tool_io inclusion
- [ ] **API-05**: GET /v1/sessions/:id/tree — conversation tree with sidechain structure
- [ ] **API-06**: GET /v1/sessions/:id/agents — agent hierarchy for session
- [ ] **API-07**: GET /v1/sessions/:id/summary — session summary (token totals, tool counts, duration)
- [ ] **API-08**: POST /v1/messages/query — flexible query body compiled to parameterized SQL
- [ ] **API-09**: GET /v1/messages/:uuid — single message by UUID
- [ ] **API-10**: GET /v1/search?q= — FTS5 search across all content
- [ ] **API-11**: GET /v1/analytics/tokens — token analysis with grouping (session, day, model)
- [ ] **API-12**: GET /v1/analytics/tools — tool frequency and error rates
- [ ] **API-13**: GET /v1/analytics/models — model usage breakdown
- [ ] **API-14**: GET /v1/export/:session_id — streamed export (json, markdown, csv)
- [ ] **API-15**: GET /v1/schema/versions — tracked Claude Code versions
- [ ] **API-16**: GET /v1/schema/drift — detected schema drift events
- [ ] **API-17**: GET /v1/files — list files with filters (session, path, date, limit)
- [ ] **API-18**: GET /v1/files/:file_id — file entry with all operations
- [ ] **API-19**: GET /v1/files/:file_id/content — reconstructed file at latest state (or at specific message via ?at=)
- [ ] **API-20**: GET /v1/files/:file_id/diff — unified diff of all edits
- [ ] **API-21**: GET /v1/files/search?q= — FTS across file contents
- [ ] **API-22**: POST /v1/files/query — flexible file operation query with glob pattern support
- [ ] **API-23**: GET /v1/git — git operations with filters (session, type, date)
- [ ] **API-24**: GET /v1/git/commits — commit operations across all sessions
- [ ] **API-25**: GET /v1/git/commits/:session_id — commits in specific session
- [ ] **API-26**: GET /v1/artifacts/:session_id — combined files + git + tool outputs view
- [ ] **API-27**: GET /v1/artifacts/:session_id/timeline — chronological artifact events

### SSE Events

- [ ] **SSE-01**: GET /v1/events — SSE stream endpoint
- [ ] **SSE-02**: record:added event when new record ingested
- [ ] **SSE-03**: session:started event when new session detected
- [ ] **SSE-04**: schema:drift event when new overflow fields detected
- [ ] **SSE-05**: version:changed event when Claude Code version changes
- [ ] **SSE-06**: file:written and file:edited events when file operations detected
- [ ] **SSE-07**: git:commit event when git commit extracted

### Unix Domain Socket

- [ ] **UDS-01**: Same HTTP API served over Unix domain socket at $CLAUDE_HISTORY_SOCKET or /tmp/claude-history.sock
- [ ] **UDS-02**: Lower-latency alternative for local consumers

### CLI Interface

- [ ] **CLI-01**: claude-history serve — start daemon (file watcher + HTTP API + version monitor)
- [ ] **CLI-02**: claude-history sync — one-shot bulk import
- [ ] **CLI-03**: claude-history query — query messages with filters, output JSON to stdout
- [ ] **CLI-04**: claude-history sessions — list sessions with filters
- [ ] **CLI-05**: claude-history search — full-text search across all sessions
- [ ] **CLI-06**: claude-history stats — token usage, tool frequency, model breakdown
- [ ] **CLI-07**: claude-history export — export session to JSON/Markdown/CSV
- [ ] **CLI-08**: claude-history version-check — show Claude Code version and drift
- [ ] **CLI-09**: claude-history schema drift — show schema drift events
- [ ] **CLI-10**: claude-history files — list files touched, with filters
- [ ] **CLI-11**: claude-history file-history — chronological operations on a file
- [ ] **CLI-12**: claude-history reconstruct — reconstruct file content at a point in time
- [ ] **CLI-13**: claude-history git-log — show git operations
- [ ] **CLI-14**: claude-history artifacts — combined view for a session
- [ ] **CLI-15**: CLI connects to daemon socket if available, otherwise opens DB read-only

### Version & Drift Monitoring

- [ ] **VER-01**: Detect Claude Code version from most recent JSONL record, claude --version, or npm ls
- [ ] **VER-02**: Record version changes in schema_versions table
- [ ] **VER-03**: Compare overflow field sets between versions to detect new fields
- [ ] **VER-04**: Periodic version check loop in daemon mode

### Infrastructure

- [ ] **INFRA-01**: Cargo workspace with 3 crates: core, store, server
- [ ] **INFRA-02**: Single binary output from server crate
- [ ] **INFRA-03**: DB location: $CLAUDE_HISTORY_DB_PATH or ~/.claude/.claude-history.db
- [ ] **INFRA-04**: Default HTTP port: 7424
- [ ] **INFRA-05**: Daemon mode as foreground process (launchd/systemd manages backgrounding)
- [ ] **INFRA-06**: Graceful shutdown with tokio CancellationToken
- [ ] **INFRA-07**: tracing crate for structured logging

## v2 Requirements

Deferred to future release. Tracked but not in current roadmap.

### Enhanced Features

- **MCP-01**: MCP server mode — thin translation layer over HTTP API for direct Claude Code integration
- **PROV-01**: Cross-session file provenance — "show every session that touched src/main.rs"
- **COST-01**: Cost analytics with configurable pricing table
- **DIST-01**: Cross-compiled binaries for macOS ARM, macOS x86, Linux ARM, Linux x86 via GitHub Actions
- **DIST-02**: launchd plist and systemd service file templates
- **SPEC-01**: openapi.yaml as API contract source of truth

## Out of Scope

| Feature | Reason |
|---------|--------|
| Web UI / dashboard frontend | API is the product; UIs are consumer concerns. Keeps scope focused. |
| Cloud sync / remote database | Session data contains proprietary code and credentials. Local-only avoids privacy/security liability. |
| AI-powered analysis / summary | Requires external API keys, costs money, non-deterministic. Tool is a data layer, not analysis layer. |
| Write-back to JSONL files | JSONL files are Claude Code's data. Writing risks corruption and breaks append-only invariant. |
| Multi-user / authentication | Localhost tool, single-user. Teams aggregate at a higher layer. |
| Plugin / extension system | HTTP API is the extension mechanism. Plugin runtimes are premature. |
| Windows support | Claude Code has limited Windows support. macOS + Linux first. |
| Real-time token cost predictions | Pricing changes frequently. Report actuals, never predict burn rate. |

## Traceability

| Requirement | Phase | Status |
|-------------|-------|--------|
| CORE-01 | Phase 1 | Pending |
| CORE-02 | Phase 1 | Pending |
| CORE-03 | Phase 1 | Pending |
| CORE-04 | Phase 1 | Pending |
| CORE-05 | Phase 1 | Pending |
| CORE-06 | Phase 1 | Pending |
| CORE-07 | Phase 1 | Pending |
| STORE-01 | Phase 1 | Pending |
| STORE-02 | Phase 1 | Pending |
| STORE-03 | Phase 1 | Pending |
| STORE-04 | Phase 1 | Pending |
| STORE-05 | Phase 1 | Pending |
| STORE-06 | Phase 1 | Pending |
| DECOMP-01 | Phase 1 | Pending |
| DECOMP-02 | Phase 1 | Pending |
| DECOMP-03 | Phase 1 | Pending |
| DECOMP-04 | Phase 1 | Pending |
| DECOMP-05 | Phase 1 | Pending |
| DECOMP-06 | Phase 1 | Pending |
| SYNC-01 | Phase 1 | Pending |
| SYNC-02 | Phase 1 | Pending |
| SYNC-03 | Phase 1 | Pending |
| SYNC-04 | Phase 1 | Pending |
| INFRA-01 | Phase 1 | Pending |
| INFRA-02 | Phase 1 | Pending |
| INFRA-03 | Phase 1 | Pending |
| INFRA-07 | Phase 1 | Pending |
| FTS-01 | Phase 2 | Pending |
| FTS-02 | Phase 5 | Pending |
| FTS-03 | Phase 2 | Pending |
| CLI-02 | Phase 2 | Pending |
| CLI-03 | Phase 2 | Pending |
| CLI-04 | Phase 2 | Pending |
| CLI-05 | Phase 2 | Pending |
| CLI-06 | Phase 2 | Pending |
| CLI-07 | Phase 2 | Pending |
| CLI-08 | Phase 2 | Pending |
| CLI-09 | Phase 2 | Pending |
| API-01 | Phase 3 | Pending |
| API-02 | Phase 3 | Pending |
| API-03 | Phase 3 | Pending |
| API-04 | Phase 3 | Pending |
| API-05 | Phase 3 | Pending |
| API-06 | Phase 3 | Pending |
| API-07 | Phase 3 | Pending |
| API-08 | Phase 3 | Pending |
| API-09 | Phase 3 | Pending |
| API-10 | Phase 3 | Pending |
| API-11 | Phase 3 | Pending |
| API-12 | Phase 3 | Pending |
| API-13 | Phase 3 | Pending |
| API-14 | Phase 3 | Pending |
| API-15 | Phase 3 | Pending |
| API-16 | Phase 3 | Pending |
| UDS-01 | Phase 3 | Pending |
| UDS-02 | Phase 3 | Pending |
| INFRA-04 | Phase 3 | Pending |
| INFRA-05 | Phase 3 | Pending |
| INFRA-06 | Phase 3 | Pending |
| CLI-01 | Phase 3 | Pending |
| CLI-15 | Phase 3 | Pending |
| WATCH-01 | Phase 4 | Pending |
| WATCH-02 | Phase 4 | Pending |
| WATCH-03 | Phase 4 | Pending |
| SSE-01 | Phase 4 | Pending |
| SSE-02 | Phase 4 | Pending |
| SSE-03 | Phase 4 | Pending |
| SSE-04 | Phase 4 | Pending |
| SSE-05 | Phase 4 | Pending |
| ART-01 | Phase 5 | Pending |
| ART-02 | Phase 5 | Pending |
| ART-03 | Phase 5 | Pending |
| ART-04 | Phase 5 | Pending |
| ART-05 | Phase 5 | Pending |
| ART-06 | Phase 5 | Pending |
| ART-07 | Phase 5 | Pending |
| ART-08 | Phase 5 | Pending |
| ART-09 | Phase 5 | Pending |
| ART-10 | Phase 5 | Pending |
| ART-11 | Phase 5 | Pending |
| API-17 | Phase 5 | Pending |
| API-18 | Phase 5 | Pending |
| API-19 | Phase 5 | Pending |
| API-20 | Phase 5 | Pending |
| API-21 | Phase 5 | Pending |
| API-22 | Phase 5 | Pending |
| API-23 | Phase 5 | Pending |
| API-24 | Phase 5 | Pending |
| API-25 | Phase 5 | Pending |
| API-26 | Phase 5 | Pending |
| API-27 | Phase 5 | Pending |
| CLI-10 | Phase 5 | Pending |
| CLI-11 | Phase 5 | Pending |
| CLI-12 | Phase 5 | Pending |
| CLI-13 | Phase 5 | Pending |
| CLI-14 | Phase 5 | Pending |
| SSE-06 | Phase 5 | Pending |
| SSE-07 | Phase 5 | Pending |
| VER-01 | Phase 6 | Pending |
| VER-02 | Phase 6 | Pending |
| VER-03 | Phase 6 | Pending |
| VER-04 | Phase 6 | Pending |

**Coverage:**
- v1 requirements: 102 total
- Mapped to phases: 102
- Unmapped: 0

---
*Requirements defined: 2026-02-20*
*Last updated: 2026-02-20 after roadmap creation -- traceability table populated, requirement count corrected from 78 to 102*
