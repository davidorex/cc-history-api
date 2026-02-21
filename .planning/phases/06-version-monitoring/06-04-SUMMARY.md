---
phase: 06-version-monitoring
plan: 04
subsystem: server
tags: [api, cli, output-formatting, version-history, drift-analysis, grouped-output, daemon-client]

# Dependency graph
requires:
  - phase: 06-version-monitoring
    plan: 02
    provides: drift.rs ON CONFLICT occurrence counting, decompose.rs extra_json population
  - phase: 06-version-monitoring
    plan: 03
    provides: version_history_enhanced, version_history_with_diff, drift_by_version query functions
provides:
  - GET /v1/schema/versions returns VersionHistoryEntry with session_count and new_fields_count
  - GET /v1/schema/versions?diff=true returns VersionDiffEntry with new_fields and disappeared_fields
  - GET /v1/schema/drift returns Vec<VersionDriftGroup> grouped by version then record_type
  - CLI version-check shows 5-column table (VERSION, FIRST_SEEN, LAST_SEEN, SESSIONS, NEW_FIELDS)
  - CLI schema-drift shows grouped output by version and record_type with promotion status
  - DaemonClient updated to deserialize enhanced response types
affects: []

# Tech tracking
tech-stack:
  added: []
  patterns: [serde-untagged-enum-for-polymorphic-json-response, grouped-limit-by-field-count-across-nested-structures]

key-files:
  created: []
  modified:
    - crates/server/src/api/schema.rs
    - crates/server/src/main.rs
    - crates/server/src/output.rs
    - crates/server/src/daemon_client.rs

key-decisions:
  - "VersionsResponse uses #[serde(untagged)] enum so JSON output is a flat array without wrapper tag — Timeline or Diff variants serialize identically to their inner Vec"
  - "Drift limit applied by counting total fields across all nested groups rather than counting top-level version groups — provides consistent field-count limiting regardless of grouping depth"
  - "DaemonClient schema_drift (legacy flat DriftEntry) removed since endpoint now returns grouped VersionDriftGroup — keeping the old method would fail to deserialize"
  - "Unused VersionEntry and DriftEntry imports removed from daemon_client.rs and schema.rs — superseded by VersionHistoryEntry and VersionDriftGroup"

patterns-established:
  - "Polymorphic API response via serde untagged enum: same endpoint returns different JSON shapes based on query parameter"
  - "Grouped limit application: counting items across nested structures for accurate field-level limiting"

requirements-completed: [VER-01, VER-02, VER-03]

# Metrics
duration: 4min
completed: 2026-02-21
---

# Phase 6 Plan 4: API Handler and CLI Enhancement for Version Monitoring Summary

**Enhanced /v1/schema/versions with ?diff=true query param and VersionsResponse enum, /v1/schema/drift returning grouped VersionDriftGroup, CLI version-check 5-column table, CLI schema-drift grouped output with promotion status**

## Performance

- **Duration:** 4 min
- **Started:** 2026-02-21T08:38:26Z
- **Completed:** 2026-02-21T08:42:20Z
- **Tasks:** 2
- **Files modified:** 4

## Accomplishments
- GET /v1/schema/versions now returns VersionHistoryEntry (session_count, new_fields_count) by default, or VersionDiffEntry (new_fields, disappeared_fields arrays) with ?diff=true
- VersionsResponse untagged enum enables polymorphic JSON responses from same endpoint
- GET /v1/schema/drift now returns Vec<VersionDriftGroup> with entries grouped by version then record_type, each field showing occurrence_count, promotion_status, and sample_value
- Drift endpoint record_type filter applied post-retrieval by filtering within each version group
- Drift endpoint limit applied by counting total fields across nested groups
- CLI version-check outputs 5-column table: VERSION, FIRST_SEEN, LAST_SEEN, SESSIONS, NEW_FIELDS
- CLI schema-drift outputs hierarchical grouped format with version headers, record_type subheaders, and per-field rows showing FIELD, OCCURRENCES, STATUS, SAMPLE
- DaemonClient version_history() returns Vec<VersionHistoryEntry>, new schema_drift_grouped() returns Vec<VersionDriftGroup>
- Both daemon and direct DB modes produce identical output for both enhanced subcommands

## Task Commits

Each task was committed atomically:

1. **Task 1: Enhance /v1/schema/versions and /v1/schema/drift API handlers** - `1cc4132` (feat)
2. **Task 2: Enhance CLI version-check and schema-drift subcommands** - `d6c646a` (feat)

## Files Created/Modified
- `crates/server/src/api/schema.rs` - VersionsParams query struct, VersionsResponse untagged enum, versions handler with ?diff=true routing, drift handler calling drift_by_version with grouped post-retrieval filtering and limiting
- `crates/server/src/main.rs` - run_version_check calls version_history_enhanced with print_version_history output, run_schema_drift calls drift_by_version with print_drift_grouped output
- `crates/server/src/output.rs` - print_version_history for 5-column version table, print_drift_grouped for hierarchical version/record_type/field display
- `crates/server/src/daemon_client.rs` - version_history returns VersionHistoryEntry, new schema_drift_grouped returns VersionDriftGroup, removed unused DriftEntry/VersionEntry imports

## Decisions Made
- VersionsResponse uses #[serde(untagged)] enum for polymorphic JSON (no wrapper tag in output)
- Drift limit counts total fields across nested groups rather than top-level groups
- Legacy DaemonClient schema_drift method removed (would fail to deserialize the new grouped response)
- Unused old type imports cleaned up from daemon_client.rs and schema.rs

## Deviations from Plan

None - plan executed exactly as written.

## Issues Encountered
None

## User Setup Required
None - no external service configuration required.

## Next Phase Readiness
- This is the final plan (06-04) in Phase 6, completing the version monitoring feature
- All API endpoints and CLI subcommands are enhanced with enriched version history and grouped drift analysis
- Full project compiles cleanly (cargo build) and all 148 tests pass (38 core + 110 store)
- No blockers identified

---
*Phase: 06-version-monitoring*
*Completed: 2026-02-21*
