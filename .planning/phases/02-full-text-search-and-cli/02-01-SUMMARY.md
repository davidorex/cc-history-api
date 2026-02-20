---
phase: 02-full-text-search-and-cli
plan: 01
subsystem: database
tags: [fts5, sqlite, search, bm25, query-builder, rusqlite]

# Dependency graph
requires:
  - phase: 01-core-types-and-ingestion-pipeline
    provides: "message_content table, sync pipeline, schema migration runner, decompose engine"
provides:
  - "FTS5 fts_message_content virtual table (external-content, unicode61)"
  - "rebuild_fts_index() and search_messages() in fts.rs"
  - "9 query functions in query.rs for all CLI subcommands"
  - "Automatic FTS rebuild after sync_all when new data ingested"
affects: [02-02-cli-search-sessions-query-stats, 02-03-cli-export-version-drift, 05-artifact-layer]

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "FTS5 external-content with rebuild-after-sync"
    - "Dynamic WHERE clause builder with params_from_iter and Box<dyn ToSql>"
    - "SQL aggregation for stats (SUM, COUNT, GROUP BY) — not in-memory"
    - "Batch-based message export to avoid OOM on large sessions"

key-files:
  created:
    - "crates/store/migrations/002_fts5.sql"
    - "crates/store/src/fts.rs"
    - "crates/store/src/query.rs"
  modified:
    - "crates/store/src/schema.rs"
    - "crates/store/src/sync.rs"
    - "crates/store/src/lib.rs"
    - "crates/store/src/db.rs"

key-decisions:
  - "FTS5 external-content mode with rebuild-after-sync — avoids storage duplication while keeping index consistent via single rebuild at end of sync_all"
  - "User query input sanitized by double-quote wrapping with internal quote escaping — prevents FTS5 syntax injection while treating input as phrase search"
  - "Dynamic query parameters use Box<dyn ToSql> with params_from_iter — handles variable-count WHERE clauses without compile-time parameter count constraints"
  - "COALESCE used in SQL aggregation for NULL-safety on optional token fields"

patterns-established:
  - "FTS5 external-content table: content='table_name', content_rowid='id', rebuild via INSERT INTO fts_table(fts_table) VALUES('rebuild')"
  - "Dynamic WHERE clause: Vec<Box<dyn ToSql>> accumulated with numbered ?N placeholders, finalized with params_from_iter"
  - "Query function signature: (conn: &Connection, filters...) -> Result<Vec<T>, rusqlite::Error>"
  - "Batch export: load messages in LIMIT/OFFSET batches, sub-query content blocks and token usage per message"

requirements-completed: [FTS-01, FTS-03, CLI-02, CLI-03, CLI-04, CLI-06, CLI-07, CLI-08, CLI-09]

# Metrics
duration: 4min
completed: 2026-02-20
---

# Phase 2 Plan 1: FTS5 Index + Store Query Layer Summary

**FTS5 external-content search index over message_content with BM25 ranking, plus 9 parameterized query builder functions covering sessions, messages, stats, export, versions, and drift**

## Performance

- **Duration:** 4 min
- **Started:** 2026-02-20T03:59:11Z
- **Completed:** 2026-02-20T04:03:35Z
- **Tasks:** 2
- **Files modified:** 7

## Accomplishments
- FTS5 virtual table `fts_message_content` created via migration 002 using external-content mode (no storage duplication)
- Search module with `rebuild_fts_index()` and `search_messages()` providing BM25-ranked results with snippet extraction and input sanitization
- Complete query layer (9 functions) covering all CLI subcommand data needs: list_sessions, query_messages, token_stats_by_model, token_stats_by_session, tool_frequency, model_breakdown, version_history, schema_drift_list, session_messages_for_export
- FTS index rebuild integrated into sync pipeline — triggers automatically after sync_all when new data was ingested

## Task Commits

Each task was committed atomically:

1. **Task 1: FTS5 migration + search module** - `bb16e78` (feat)
2. **Task 2: Query builder + stats + FTS rebuild in sync** - `231321b` (feat)

## Files Created/Modified
- `crates/store/migrations/002_fts5.sql` - FTS5 external-content virtual table DDL indexing message_content.text_content
- `crates/store/src/fts.rs` - SearchResult struct, rebuild_fts_index(), search_messages() with BM25 ranking and snippet extraction
- `crates/store/src/query.rs` - 9 query builder functions with Serialize+Debug result structs, parameterized SQL, SQL aggregation for stats
- `crates/store/src/schema.rs` - MIGRATIONS array extended with ("002", include_str!("../migrations/002_fts5.sql"))
- `crates/store/src/sync.rs` - fts::rebuild_fts_index call added after sync_all file-walking loop when files_synced > 0
- `crates/store/src/lib.rs` - pub mod fts and pub mod query declarations
- `crates/store/src/db.rs` - Test assertion updated from 1 to 2 migration versions

## Decisions Made
- FTS5 external-content mode chosen over content-storing FTS5 to avoid roughly doubling DB size (message_content can contain multi-thousand-line tool results)
- Query input sanitized by wrapping in double quotes with escaped internal quotes — simple phrase matching prevents FTS5 syntax injection (Research Pitfall 3), advanced syntax can be added later via --raw flag
- BM25 ordering uses ascending sort (lower = better match per SQLite FTS5 documentation) — Research Pitfall 2 addressed
- FTS-02 (file_operations FTS index) documented as deferred to Phase 5 in migration comments — table does not exist until Phase 5
- Dynamic SQL parameters use Box<dyn ToSql> rather than converting everything to strings — preserves type safety for integer parameters like LIMIT

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] Fixed hardcoded migration count assertion in db.rs test**
- **Found during:** Task 1 (FTS5 migration + search module)
- **Issue:** `test_init_db_creates_schema_and_sets_pragmas` asserted `count == 1` for schema_versions rows, which broke when migration 002 was added (now 2 rows)
- **Fix:** Updated assertion from `count == 1` to `count == 2` with updated message text
- **Files modified:** `crates/store/src/db.rs`
- **Verification:** All 26 tests pass after fix
- **Committed in:** bb16e78 (Task 1 commit)

---

**Total deviations:** 1 auto-fixed (1 bug fix)
**Impact on plan:** Minimal — hardcoded test count was inherently fragile to new migrations. The fix is the correct behavior.

## Issues Encountered
None

## User Setup Required
None - no external service configuration required.

## Next Phase Readiness
- Store query layer is complete — all 9 functions are ready for CLI subcommand consumption in Plan 02-02 and 02-03
- FTS rebuild integrates seamlessly with existing sync pipeline
- All result structs are Serialize+Debug, ready for JSON output in CLI
- FTS-02 (file_operations index) remains blocked on Phase 5 table creation — documented in migration comments

---
*Phase: 02-full-text-search-and-cli*
*Completed: 2026-02-20*
