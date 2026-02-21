---
phase: 06-version-monitoring
plan: 03
subsystem: server, database
tags: [watcher, query, version-history, drift-analysis, promotion-status, schema-introspection]

# Dependency graph
requires:
  - phase: 06-version-monitoring
    plan: 01
    provides: version_history table, messages enrichment columns, schema_drift_log occurrence tracking
  - phase: 06-version-monitoring
    plan: 02
    provides: drift.rs ON CONFLICT occurrence counting, decompose.rs extra_json population
provides:
  - version_history persistence in watcher check_version_change (INSERT ON CONFLICT)
  - watcher startup backfill of version_history from sessions table
  - version_history_enhanced() query returning timeline with session_count and new_fields_count
  - version_history_with_diff() query computing new_fields and disappeared_fields per version
  - drift_by_version() query grouping drift entries by version then record_type with dynamic promotion_status
affects: [06-04-api-cli]

# Tech tracking
tech-stack:
  added: []
  patterns: [dynamic-promotion-status-via-pragma-table-info, startup-backfill-idempotent, persist-after-event-emission]

key-files:
  created: []
  modified:
    - crates/server/src/watcher.rs
    - crates/store/src/query.rs

key-decisions:
  - "Version persistence happens AFTER SSE event emission — DB failure does not prevent event delivery, logged at warn level"
  - "Startup backfill uses INSERT OR IGNORE — safe to run repeatedly, only populates rows that don't already exist"
  - "Promotion status computed dynamically via PRAGMA table_info cached per query invocation (not per field) — avoids stale static mappings"
  - "record_type_to_table mapping: user/assistant/assistant.message/progress -> messages, assistant.message.usage -> token_usage, system -> system_events, summary -> summaries, queue-operation/file-history-snapshot -> None"
  - "sample_value truncated to 200 chars in drift_by_version per CONTEXT decision"

patterns-established:
  - "Dynamic schema introspection via PRAGMA table_info for computing field promotion status"
  - "Persist-after-emit pattern: SSE event emission precedes database writes so delivery is not blocked by DB failures"

requirements-completed: [VER-01, VER-04]

# Metrics
duration: 4min
completed: 2026-02-21
---

# Phase 6 Plan 3: Watcher Version Persistence and Enhanced Query Functions Summary

**Watcher version_history persistence via INSERT ON CONFLICT plus three query functions (version_history_enhanced, version_history_with_diff, drift_by_version) with dynamic promotion status computed via PRAGMA table_info schema introspection**

## Performance

- **Duration:** 4 min
- **Started:** 2026-02-21T08:32:03Z
- **Completed:** 2026-02-21T08:35:47Z
- **Tasks:** 2
- **Files modified:** 2

## Accomplishments
- check_version_change() in watcher.rs now persists version changes to version_history table after SSE event emission, using INSERT ON CONFLICT to handle both first-seen and re-observed versions with session_count increment
- watcher_loop() startup backfills version_history from sessions table via INSERT OR IGNORE for idempotent population of historical data
- version_history_enhanced() reads from dedicated version_history table with session_count and new_fields_count
- version_history_with_diff() computes per-version field changes (new_fields, disappeared_fields) by comparing drift entries between consecutive versions
- drift_by_version() groups schema_drift_log by version then record_type, computing promotion_status dynamically from PRAGMA table_info: "promoted" if field matches a real column, "extra_json" if target table has extra_json column, "unhandled" otherwise
- 3 new tests covering version_history_enhanced data retrieval, drift_by_version grouping structure, and promotion_status correctness across promoted/extra_json/unhandled states

## Task Commits

Each task was committed atomically:

1. **Task 1: Add version_history persistence to watcher.rs** - `c0d73bd` (feat)
2. **Task 2: Add enhanced query functions for version history and grouped drift** - `17e62ee` (feat)

## Files Created/Modified
- `crates/server/src/watcher.rs` - version_history persistence in check_version_change after SSE emission, startup backfill in watcher_loop, updated module doc comment
- `crates/store/src/query.rs` - 5 new result structs (VersionHistoryEntry, DriftFieldEntry, VersionDriftGroup, RecordTypeDriftGroup, VersionDiffEntry), 3 new query functions (version_history_enhanced, version_history_with_diff, drift_by_version), 2 helper functions (record_type_to_table, table_columns), 3 new tests

## Decisions Made
- Version persistence happens AFTER SSE event emission so DB failure does not prevent event delivery
- Startup backfill uses INSERT OR IGNORE for safe repeated execution
- Promotion status computed dynamically via PRAGMA table_info, cached per query invocation rather than per field
- record_type mapping covers all 9 drift record_types across 4 target tables (messages, token_usage, system_events, summaries) with 2 mapping to None (queue-operation, file-history-snapshot)

## Deviations from Plan

None - plan executed exactly as written.

## Issues Encountered
None

## User Setup Required
None - no external service configuration required.

## Next Phase Readiness
- All query functions are in place for plan 06-04 (API endpoints and CLI enhancements)
- Watcher persistence and backfill are operational
- All 110 store crate tests pass
- No blockers identified

---
*Phase: 06-version-monitoring*
*Completed: 2026-02-21*
