# Project State

## Project Reference

See: .planning/PROJECT.md (updated 2026-02-20)

**Core value:** Universal, language-agnostic, queryable access to Claude Code's complete session history through a single binary that never discards data and actively detects schema evolution.
**Current focus:** Phase 3 - HTTP API and Daemon

## Current Position

Phase: 3 of 6 (HTTP API and Daemon) — PLANNED
Plan: 0 of 6 complete in current phase — planning finalized, ready for execution
Status: Phase 3 planned. 6 plans in 6 sequential waves. axum 0.8 HTTP API + dual TCP/UDS listeners + DaemonClient for CLI-over-daemon routing.
Last activity: 2026-02-20 -- Phase 3 planning complete (research, plan, verify, generate)

Progress: [█████░░░░░] ~47% (7 of ~19 total plans)

## Performance Metrics

**Velocity:**
- Total plans completed: 7
- Average duration: 7 min
- Total execution time: 0.8 hours

**By Phase:**

| Phase | Plans | Total | Avg/Plan |
|-------|-------|-------|----------|
| 01 | 4/4 | 28 min | 7 min |
| 02 | 3/3 | 22 min | 7.3 min |

**Recent Trend:**
- Last 5 plans: 5, 10, 4, 12, 6 min
- Trend: 02-03 was straightforward — query functions pre-built in 02-02, csv dep pre-staged

*Updated after each plan completion*

## Accumulated Context

### Decisions

Decisions are logged in PROJECT.md Key Decisions table.
Recent decisions affecting current work:

- [Roadmap]: 6-phase structure following crate dependency graph (core -> store -> server), artifact layer deferred to Phase 5 per research recommendation
- [Roadmap]: tokio-rusqlite bridge and split writer/reader pool established in Phase 1 to avoid costly retrofitting
- [01-01]: Pinned rusqlite to 0.37 (not 0.38) — tokio-rusqlite 0.7.0 depends on rusqlite 0.37 via libsqlite3-sys 0.35
- [01-01]: Removed fts5 feature flag — bundled SQLite includes FTS5 by default; rusqlite 0.37 does not expose it separately
- [01-01]: SchemaError mapped to rusqlite::Error via ToSqlConversionFailure inside conn.call closure
- [01-02]: sourceToolAssistantUUID requires explicit serde(rename) — camelCase transform produces lowercase 'uuid' not uppercase 'UUID'
- [01-02]: RecordBase has no overflow HashMap — only ONE overflow per struct at outermost level to avoid serde(flatten) ambiguity
- [01-02]: ProgressRecord data stored as serde_json::Value — 8+ data.type variants too varied for Phase 1 typed modeling
- [01-02]: Parser error model: ParseError for file-level I/O, ParseWarning for line-level deser failures — malformed lines never halt parsing
- [01-03]: drift.rs co-committed with decompose.rs — compile-time dependency (decompose imports drift::log_overflow) prevented separate commits
- [01-03]: Qualified record_type names for assistant overflow maps: 'assistant', 'assistant.message', 'assistant.message.usage' — enables per-layer drift analysis
- [01-03]: file_history_snapshot decomposition skips with debug log — no target table in Phase 1 schema
- [01-04]: No deviation-level decisions — sync engine and CLI implemented per plan specification
- [02-01]: FTS5 external-content mode with rebuild-after-sync — avoids storage duplication while keeping index consistent
- [02-01]: User query input sanitized by double-quote wrapping — prevents FTS5 syntax injection, treats as phrase search
- [02-01]: Dynamic query parameters use Box<dyn ToSql> with params_from_iter — handles variable-count WHERE clauses
- [02-02]: Moved --db-path to global Cli struct — shared across all subcommands without repetition
- [02-02]: open_db() checks db file existence before init_db — actionable error message suggesting sync first
- [02-02]: Query subcommand always JSON (no --json flag) — machine consumption per spec
- [02-02]: Stats --json combines three queries into single JSON object with token_usage/tool_frequency/model_breakdown keys
- [02-02]: csv crate pre-staged in workspace deps for Plan 03
- [02-03]: Export functions use tokio_rusqlite::rusqlite::Connection re-export — server crate does not depend on rusqlite directly
- [02-03]: Export writes to Vec<u8> buffer inside conn.call, flushed to stdout outside — avoids blocking DB thread with I/O
- [02-03]: SchemaDrift record_type filter applied in Rust post-retrieval — keeps query.rs simple

### Pending Todos

None yet.

### Blockers/Concerns

- FTS-02 (file_operations FTS index) deferred to Phase 5 — file_operations table does not exist until Phase 5 (Artifact Layer). Phase 2 SC-1 is partially satisfied for message content only.

## Session Continuity

Last session: 2026-02-20
Stopped at: Phase 3 planning complete, ready for execution
Resume file: .planning/phases/03-http-api-and-daemon/03-01-PLAN.md (first plan)
