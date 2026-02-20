# Project State

## Project Reference

See: .planning/PROJECT.md (updated 2026-02-20)

**Core value:** Universal, language-agnostic, queryable access to Claude Code's complete session history through a single binary that never discards data and actively detects schema evolution.
**Current focus:** Phase 4 - Real-Time Ingestion and Events -- IN PROGRESS

## Current Position

Phase: 4 of 6 (Real-Time Ingestion and Events) -- IN PROGRESS
Plan: 1 of 2 complete in current phase
Status: Plan 01 (SSE Event Infrastructure) complete. Plan 02 (File Watcher) ready for execution.
Last activity: 2026-02-20 -- Completed 04-01-PLAN.md (SSE event types, broadcast channel, GET /v1/events endpoint)

Progress: [████████░░] ~71% (14 of ~21 total plans)

## Performance Metrics

**Velocity:**
- Total plans completed: 14
- Average duration: ~6.2 min
- Total execution time: ~1.45 hours

**By Phase:**

| Phase | Plans | Total | Avg/Plan |
|-------|-------|-------|----------|
| 01 | 4/4 | 28 min | 7 min |
| 02 | 3/3 | 22 min | 7.3 min |
| 03 | 6/6 | ~30 min | ~5 min |
| 04 | 1/2 | 3 min | 3 min |

**Recent Trend:**
- Last 5 plans: 5, 5, 5, 4, 3 min
- Trend: 04-01 SSE infrastructure completed in 3 min — 1 deviation (commit boundary shift for compile coherence)

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
- [03-01]: rusqlite accessed via tokio_rusqlite::rusqlite re-export in server crate (per decision [02-03]), avoiding direct rusqlite dependency
- [03-01]: tokio_rusqlite::Error variant is Error(E) not Rusqlite — plan pseudocode adjusted at implementation time
- [03-01]: tokio_rusqlite::Error is #[non_exhaustive], requiring wildcard catch-all arm in match
- [03-02]: Used axum 0.8 path parameter syntax ({id}) instead of :id — axum 0.8 requires curly-brace syntax
- [03-02]: search handler validates non-empty q parameter, returns 400 BadRequest — exercises the previously-unused BadRequest variant
- [03-03]: Export handler maps Box<dyn Error> to string then wraps in rusqlite::Error::ToSqlConversionFailure — original error type lacks Send+Sync bounds required by the rusqlite error variant
- [03-04]: CancellationToken::cancelled() requires async move wrapper for 'static lifetime in tokio::spawn — with_graceful_shutdown(token.cancelled()) fails because cancelled() borrows the token, but spawned tasks need 'static
- [03-05]: Added serde::Deserialize to all store query result structs and HealthResponse — DaemonClient needs to deserialize daemon JSON responses into the same types the store layer produces
- [03-05]: export_session() returns raw Vec<u8> instead of typed JSON — export responses may be markdown or CSV depending on format parameter
- [03-05]: Minimal custom urlencoded() function instead of percent-encoding crate — query parameter values are simple strings, full crate is excessive
- [03-06]: All 7 read-only handlers updated in single commit due to shared dispatch refactor — cannot compile with partial conversion
- [03-06]: Daemon communication failures produce explicit errors rather than silent fallback to direct DB mid-request
- [03-06]: Stats daemon routing uses group_by=session when session_id present, matching direct DB token_stats_by_model vs token_stats_by_session logic
- [04-01]: SseEvent uses manual event_type()/to_json_data() methods instead of serde tag/content — SSE event name set via axum Event::event(), data payload is flat JSON without enum wrapper
- [04-01]: Broadcast channel capacity 1024 — aims for ~100 seconds buffer at 10 events/second, tunable later
- [04-01]: Lagged errors silently dropped via filter_map — slow SSE clients lose events gracefully rather than blocking producers

### Pending Todos

None yet.

### Blockers/Concerns

- FTS-02 (file_operations FTS index) deferred to Phase 5 — file_operations table does not exist until Phase 5 (Artifact Layer). Phase 2 SC-1 is partially satisfied for message content only.

## Session Continuity

Last session: 2026-02-20
Stopped at: Phase 4 Plan 01 complete, Plan 02 ready for execution
Resume file: /gsd:execute-phase 4
