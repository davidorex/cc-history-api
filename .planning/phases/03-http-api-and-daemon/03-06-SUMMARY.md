---
phase: 03-http-api-and-daemon
plan: 06
subsystem: api
tags: [cli, daemon, uds, connection-mode, http-client]

# Dependency graph
requires:
  - phase: 03-http-api-and-daemon (plan 05)
    provides: DaemonClient with 10 endpoint methods, ConnectionMode enum, detect_connection_mode health-check probe
provides:
  - All 7 read-only CLI subcommands wired to route through daemon HTTP API over UDS when socket is available
  - Direct DB fallback when daemon is not running (identical output in both modes)
  - Sync subcommand excluded from daemon routing (write operation)
  - Single connection mode detection at startup shared across all read subcommands
affects: [04-phase-testing, 05-artifact-layer]

# Tech tracking
tech-stack:
  added: []
  patterns: [ConnectionMode dispatch pattern, single-detect-at-startup routing, daemon-first with direct fallback]

key-files:
  created: []
  modified:
    - crates/server/src/main.rs
    - crates/server/src/daemon_client.rs

key-decisions:
  - "All 7 read-only handlers updated in single commit due to shared dispatch refactor — cannot compile with partial conversion"
  - "Daemon communication failures produce explicit errors rather than silent fallback to direct DB mid-request"
  - "Stats daemon routing uses group_by=session when session_id filter is present, matching the direct DB logic that switches between token_stats_by_model and token_stats_by_session"

patterns-established:
  - "ConnectionMode dispatch: each run_* function takes ConnectionMode, matches on Daemon/Direct, calls identical output formatting regardless of source"
  - "resolve_connection_mode called once at startup, not per-subcommand — avoids redundant health checks"

requirements-completed: [CLI-15]

# Metrics
duration: 4min
completed: 2026-02-20
---

# Phase 3, Plan 6: CLI-15 Wire CLI Subcommands Through Daemon Summary

**All 7 read-only CLI subcommands (sessions, search, query, stats, export, version-check, schema-drift) route through daemon HTTP API over UDS when socket is available, with transparent direct DB fallback**

## Performance

- **Duration:** 4 min
- **Started:** 2026-02-20T10:15:31Z
- **Completed:** 2026-02-20T10:19:55Z
- **Tasks:** 2
- **Files modified:** 2

## Accomplishments
- Wired all 7 read-only CLI subcommands to route through DaemonClient when daemon socket is available and healthy
- Refactored main() dispatch to detect connection mode once at startup and pass the shared ConnectionMode to all read subcommands
- Sync subcommand correctly excluded from daemon routing (write operation that must open DB directly)
- Serve subcommand correctly excluded (it IS the daemon)
- Removed dead open_db() helper function that was superseded by ConnectionMode's built-in DB opening

## Task Commits

Each task was committed atomically:

1. **Task 1: Integrate ConnectionMode into main dispatch and update sessions/search/query subcommands** - `3cac252` (feat)
2. **Task 2: Update stats/export/version-check/schema-drift subcommands for daemon routing** - `8e7bf7a` (feat)

## Files Created/Modified
- `crates/server/src/main.rs` - Refactored main() dispatch to use ConnectionMode, updated all 7 read-only run_* functions with Daemon/Direct match arms, removed unused open_db(), added resolve_connection_mode() helper
- `crates/server/src/daemon_client.rs` - Added debug log at Direct DB return point in detect_connection_mode for full mode selection observability

## Decisions Made
- All 7 read-only handlers were updated in Task 1's commit because the dispatch refactor in main() required all handlers to accept ConnectionMode simultaneously for compilation. Task 2's commit covers the daemon_client.rs debug log addition.
- Daemon communication failures produce explicit error messages (e.g., "Daemon search failed: ...") rather than silently falling back to direct DB mid-request. This follows the plan's specification that connection mode is decided once at startup.
- Stats daemon routing passes group_by=session when session_id is present, mirroring the direct DB logic that switches between token_stats_by_model and token_stats_by_session.

## Deviations from Plan

None -- plan executed as written. The only structural difference is that all 7 handler updates were committed together in Task 1 because they share a single dispatch refactor point in main() that cannot compile with partial conversion. Task 2's commit adds the debug log line called for in the plan.

## Issues Encountered
None.

## User Setup Required
None -- no external service configuration required.

## Next Phase Readiness
- Phase 3 (HTTP API and Daemon) is now complete: all 6 plans executed
- All daemon infrastructure is in place: serve command, HTTP API handlers, DaemonClient, ConnectionMode dispatch, CLI wiring
- Ready for Phase 4 (testing/validation) or Phase 5 (artifact layer)
- SC-4 ("CLI commands automatically connect to the daemon socket when available") is satisfied
- CLI-15 ("CLI connects to daemon socket if available, otherwise opens DB read-only") is satisfied

---
*Phase: 03-http-api-and-daemon*
*Completed: 2026-02-20*
