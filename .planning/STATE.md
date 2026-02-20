# Project State

## Project Reference

See: .planning/PROJECT.md (updated 2026-02-20)

**Core value:** Universal, language-agnostic, queryable access to Claude Code's complete session history through a single binary that never discards data and actively detects schema evolution.
**Current focus:** Phase 2 - Full-Text Search and CLI

## Current Position

Phase: 2 of 6 (Full-Text Search and CLI) — IN PROGRESS
Plan: 1 of 3 complete in current phase
Status: Plan 02-01 (FTS5 Index + Store Query Layer) complete. Next: 02-02 (CLI search/sessions/query/stats).
Last activity: 2026-02-20 -- Completed 02-01-PLAN.md (FTS5 + query layer)

Progress: [████░░░░░░] ~33% (5 of ~15 total plans)

## Performance Metrics

**Velocity:**
- Total plans completed: 5
- Average duration: 6.4 min
- Total execution time: 0.5 hours

**By Phase:**

| Phase | Plans | Total | Avg/Plan |
|-------|-------|-------|----------|
| 01 | 4/4 | 28 min | 7 min |
| 02 | 1/3 | 4 min | 4 min |

**Recent Trend:**
- Last 5 plans: 6, 5, 10, 4 min
- Trend: stable-to-improving (02-01 faster due to no TDD and familiar patterns)

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

### Pending Todos

None yet.

### Blockers/Concerns

- FTS-02 (file_operations FTS index) deferred to Phase 5 — file_operations table does not exist until Phase 5 (Artifact Layer). Phase 2 SC-1 is partially satisfied for message content only.

## Session Continuity

Last session: 2026-02-20
Stopped at: Plan 02-01 complete, ready for 02-02 execution
Resume file: .planning/phases/02-full-text-search-and-cli/02-02-PLAN.md
