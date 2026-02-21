---
phase: 06-version-monitoring
plan: 02
subsystem: database
tags: [sqlite, decomposer, schema-drift, overflow-extraction, extra-json, compact-summary]

# Dependency graph
requires:
  - phase: 06-version-monitoring
    plan: 01
    provides: is_compact_summary, source_tool_use_id, extra_json columns on messages; occurrence_count, last_seen_at on schema_drift_log
  - phase: 01-foundation
    provides: decompose.rs decomposition engine, drift.rs overflow logger
provides:
  - drift.rs INSERT ON CONFLICT with occurrence_count increment and last_seen_at update
  - decompose_user extracts isCompactSummary -> is_compact_summary and sourceToolUseID -> source_tool_use_id from overflow
  - extra_json populated on messages from remaining overflow (user) or merged record+message overflow (assistant)
  - Promoted keys excluded from extra_json (no duplication)
affects: [06-03-watcher-queries, 06-04-api-cli]

# Tech tracking
tech-stack:
  added: []
  patterns: [on-conflict-do-update-occurrence-tracking, promoted-field-extraction-from-overflow, merged-overflow-to-extra-json]

key-files:
  created: []
  modified:
    - crates/store/src/drift.rs
    - crates/store/src/decompose.rs

key-decisions:
  - "drift.rs return value semantic change: log_overflow now returns count of both inserts and updates (not just new entries) because ON CONFLICT DO UPDATE returns 1 for each affected row — acceptable since return value is used only for SSE event emission and debug logging"
  - "decompose_user drift logging uses ORIGINAL overflow (including promoted keys) because drift detection tracks what Claude Code sends, not what gets promoted to columns"
  - "decompose_assistant merges record-level and message-level overflow into single extra_json — avoids two separate JSON columns while preserving all overflow data"

patterns-established:
  - "Promoted field extraction: remove known keys from overflow clone, serialize remainder to extra_json, UPDATE promoted columns after INSERT OR IGNORE — enables backfill on re-sync"
  - "ON CONFLICT DO UPDATE for occurrence tracking: single statement handles both first-observation insert and re-observation update without needing separate SELECT"

requirements-completed: [VER-03]

# Metrics
duration: 3min
completed: 2026-02-21
---

# Phase 6 Plan 2: Decomposer Overflow Extraction and Drift Occurrence Tracking Summary

**drift.rs ON CONFLICT occurrence counting plus decompose_user compact summary field extraction and extra_json population for both user and assistant records**

## Performance

- **Duration:** 3 min
- **Started:** 2026-02-21T08:26:15Z
- **Completed:** 2026-02-21T08:29:30Z
- **Tasks:** 2
- **Files modified:** 2

## Accomplishments
- drift.rs now uses INSERT ... ON CONFLICT DO UPDATE to increment occurrence_count and refresh last_seen_at on each re-observation of a schema drift field
- decompose_user extracts isCompactSummary and sourceToolUseID from overflow into promoted messages columns, with remaining overflow serialized to extra_json
- decompose_assistant merges record-level and message-level overflow into single extra_json on the messages row
- Promoted keys (isCompactSummary, sourceToolUseID) do not appear in extra_json — no duplication between columns and JSON
- 5 new tests added: 1 occurrence count test in drift.rs, 4 decomposer tests covering compact summary extraction, extra_json content, empty overflow defaults, and assistant overflow merging

## Task Commits

Each task was committed atomically:

1. **Task 1: Update drift.rs to track occurrence counts** - `1faa680` (feat)
2. **Task 2: Update decompose_user and decompose_assistant for compact summary fields and extra_json** - `db9f789` (feat)

## Files Created/Modified
- `crates/store/src/drift.rs` - INSERT OR IGNORE replaced with INSERT ON CONFLICT DO UPDATE for occurrence_count/last_seen_at tracking; updated doc comment, log messages, and 2 tests (1 updated, 1 new)
- `crates/store/src/decompose.rs` - decompose_user extracts promoted fields and builds extra_json; decompose_assistant merges overflow into extra_json; 4 new tests

## Decisions Made
- drift.rs return value now counts both inserts and updates — acceptable since it's used only for SSE events and debug logging, not for distinguishing new vs. existing fields
- Drift logging uses original overflow (including promoted keys) to preserve accurate tracking of what Claude Code sends
- Assistant extra_json merges record-level and message-level overflow into one JSON object rather than maintaining separate columns

## Deviations from Plan

None - plan executed exactly as written.

## Issues Encountered
None

## User Setup Required
None - no external service configuration required.

## Next Phase Readiness
- drift.rs and decompose.rs are fully updated for version monitoring
- Plans 06-03 (watcher/query functions) and 06-04 (API/CLI) can proceed — they depend on the columns and tracking behavior established here
- All 107 store crate tests pass
- No blockers identified

---
*Phase: 06-version-monitoring*
*Completed: 2026-02-21*
