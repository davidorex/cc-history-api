---
phase: 06-version-monitoring
plan: 01
subsystem: database
tags: [sqlite, migration, version-tracking, schema-drift, analytical-views]

# Dependency graph
requires:
  - phase: 01-foundation
    provides: sessions table with version column, messages table, schema_drift_log table
  - phase: 04-modeling
    provides: 7 analytical views (v_file_token_cost, v_file_conversation_context, v_project_summary, v_file_provenance, v_git_commit_context, v_tool_errors, v_session_cost)
provides:
  - version_history table for persistent version tracking with session counts and new_fields_count
  - is_compact_summary, source_tool_use_id, extra_json columns on messages table
  - occurrence_count, last_seen_at columns on schema_drift_log table
  - All 7 analytical views recreated with is_compact_summary = 0 filtering
affects: [06-02-decomposer, 06-03-watcher-queries, 06-04-api-cli]

# Tech tracking
tech-stack:
  added: []
  patterns: [correlated-subquery-backfill, view-recreation-with-filter-injection]

key-files:
  created:
    - crates/store/migrations/006_version_monitoring.sql
  modified:
    - crates/store/src/schema.rs
    - crates/store/src/db.rs

key-decisions:
  - "version_history table deliberately named to avoid collision with schema_versions migration tracker — schema_versions is created by bootstrap code in schema.rs"
  - "is_compact_summary defaults to 0 for all existing rows — precise backfill deferred to enhanced decomposer (future syncs will set this from JSONL re-parsing)"
  - "new_fields_count backfilled via correlated subquery checking no earlier first_seen_at with different version for same (field_name, record_type) — attributes drift fields to the version where they first appeared"
  - "All 7 views filter on m.is_compact_summary = 0 at the JOIN level rather than WHERE — applies to both INNER and LEFT JOINs consistently"

patterns-established:
  - "View recreation pattern: DROP VIEW IF EXISTS then CREATE VIEW when upstream table columns change — used for all 7 analytical views"
  - "Compact summary exclusion: is_compact_summary = 0 filter on messages JOINs in analytical views to exclude synthetic aggregated content"

requirements-completed: [VER-02]

# Metrics
duration: 2min
completed: 2026-02-21
---

# Phase 6 Plan 1: Version Monitoring Migration Summary

**Migration 006 establishing version_history table, messages enrichment columns (is_compact_summary, source_tool_use_id, extra_json), schema_drift_log occurrence tracking, and 7 analytical views recreated with compact summary filtering**

## Performance

- **Duration:** 2 min
- **Started:** 2026-02-21T08:21:08Z
- **Completed:** 2026-02-21T08:23:37Z
- **Tasks:** 2
- **Files modified:** 3

## Accomplishments
- version_history table created with backfill from sessions (session counts) and schema_drift_log (new_fields_count via correlated subquery)
- messages table enriched with is_compact_summary, source_tool_use_id, extra_json columns for future decomposer use
- schema_drift_log enhanced with occurrence_count and last_seen_at for tracking drift field frequency
- All 7 analytical views recreated with is_compact_summary = 0 filter to exclude synthetic compact summary content from aggregations
- 6 new tests added to schema.rs covering table creation, column presence, view queryability, idempotency, and naming collision prevention

## Task Commits

Each task was committed atomically:

1. **Task 1: Create migration 006 SQL file** - `1ad7ee5` (feat)
2. **Task 2: Register migration 006 in schema.rs and add tests** - `fcaa28d` (feat)

## Files Created/Modified
- `crates/store/migrations/006_version_monitoring.sql` - Migration SQL with 6 sections: version_history DDL, sessions backfill, drift correlation backfill, messages ALTER TABLE, schema_drift_log ALTER TABLE, 7 view recreations
- `crates/store/src/schema.rs` - Migration 006 registration in MIGRATIONS array plus 6 new tests
- `crates/store/src/db.rs` - Updated hardcoded migration count assertion from 5 to 6

## Decisions Made
- version_history named deliberately to avoid collision with schema_versions migration tracker
- is_compact_summary defaults to 0 — precise backfill requires JSONL re-parsing by enhanced decomposer on future syncs
- new_fields_count uses correlated subquery to attribute drift fields to their first-appearance version
- is_compact_summary = 0 filter applied at JOIN level (not WHERE) for consistent behavior across INNER and LEFT JOINs

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 3 - Blocking] Updated db.rs hardcoded migration count assertion**
- **Found during:** Task 2 (test verification)
- **Issue:** test_init_db_creates_schema_and_sets_pragmas asserted exactly 5 migration versions in schema_versions; migration 006 makes it 6
- **Fix:** Changed assertion from `count == 5` to `count == 6` with updated message
- **Files modified:** crates/store/src/db.rs
- **Verification:** All 102 store crate tests pass
- **Committed in:** fcaa28d (Task 2 commit)

---

**Total deviations:** 1 auto-fixed (1 blocking)
**Impact on plan:** Necessary correction for test to account for the new migration. No scope creep.

## Issues Encountered
None

## User Setup Required
None - no external service configuration required.

## Next Phase Readiness
- version_history table, messages enrichment columns, and schema_drift_log enhancements are in place for plan 06-02 (decomposer updates to populate is_compact_summary, source_tool_use_id, version_history upserts)
- All 7 analytical views are ready with compact summary filtering for plan 06-03 (query functions) and 06-04 (API/CLI)
- No blockers identified

---
*Phase: 06-version-monitoring*
*Completed: 2026-02-21*
