---
phase: 04-real-time-ingestion-and-events
plan: 01
subsystem: api
tags: [sse, broadcast-channel, tokio-stream, futures-util, server-sent-events, axum]

# Dependency graph
requires:
  - phase: 03-http-api-and-daemon
    provides: "AppState, build_router, serve infrastructure, 16 API endpoints"
provides:
  - "SseEvent enum with 4 typed variants (record:added, session:started, schema:drift, version:changed)"
  - "GET /v1/events SSE endpoint with broadcast channel fan-out"
  - "AppState.event_tx broadcast::Sender<SseEvent> for event producers"
affects: [04-02-file-watcher, real-time-ingestion]

# Tech tracking
tech-stack:
  added: [tokio-stream 0.1, futures-util 0.3, notify 8.2 (workspace only)]
  patterns: [broadcast channel fan-out for SSE, BroadcastStream with filter_map for lagged error handling]

key-files:
  created:
    - crates/server/src/events.rs
  modified:
    - Cargo.toml
    - crates/server/Cargo.toml
    - crates/server/src/state.rs
    - crates/server/src/api/mod.rs
    - crates/server/src/main.rs

key-decisions:
  - "SseEvent uses manual event_type()/to_json_data() methods instead of serde tag/content — SSE event name is set via axum Event::event(), data payload is flat JSON without enum wrapper"
  - "Broadcast channel capacity 1024 — aims for ~100 seconds buffer at 10 events/second, tunable later"
  - "Lagged errors silently dropped via filter_map — slow SSE clients lose events gracefully rather than blocking producers"

patterns-established:
  - "SSE fan-out: broadcast::Sender in AppState, subscribe() per handler invocation, BroadcastStream adapter"
  - "SSE error handling: filter_map drops Lagged, json_data errors produce fallback 'serialization error' event"

requirements-completed: [SSE-01, SSE-02, SSE-03, SSE-04, SSE-05]

# Metrics
duration: 3min
completed: 2026-02-20
---

# Phase 4 Plan 01: SSE Event Infrastructure Summary

**SseEvent enum with 4 typed variants, broadcast channel in AppState, and GET /v1/events SSE endpoint with keep-alive and lagged-client handling**

## Performance

- **Duration:** 3 min
- **Started:** 2026-02-20T10:52:30Z
- **Completed:** 2026-02-20T10:55:40Z
- **Tasks:** 2
- **Files modified:** 6

## Accomplishments

- SseEvent enum with 4 variants (RecordAdded, SessionStarted, SchemaDrift, VersionChanged) each mapping to spec-compliant SSE event names
- broadcast::Sender<SseEvent> in AppState with 1024-event capacity for fan-out to multiple concurrent SSE clients
- GET /v1/events endpoint registered as route 17, returning text/event-stream with keep-alive pings
- Lagged slow consumers handled gracefully via silent drop in filter_map

## Task Commits

Each task was committed atomically:

1. **Task 1: Add workspace dependencies and create SseEvent types + SSE handler** - `b8dd82c` (feat)
2. **Task 2: Register GET /v1/events SSE route in build_router** - `64dd621` (feat)

## Files Created/Modified

- `Cargo.toml` - Added workspace dependencies: notify 8.2, tokio-stream 0.1, futures-util 0.3
- `Cargo.lock` - Lockfile updated with new dependency versions
- `crates/server/Cargo.toml` - Added tokio-stream and futures-util to server crate dependencies
- `crates/server/src/events.rs` - New module: SseEvent enum (4 variants), event_type(), to_json_data(), events_handler SSE endpoint
- `crates/server/src/state.rs` - Added event_tx: broadcast::Sender<SseEvent> field to AppState
- `crates/server/src/api/mod.rs` - Added /v1/events route, updated doc comments to 17 endpoints across 8 groups
- `crates/server/src/main.rs` - Added events module declaration, broadcast::channel(1024) construction in run_serve

## Decisions Made

- SseEvent uses manual event_type()/to_json_data() methods rather than serde tag/content attributes — the SSE event name is set through axum's Event::event() method, not embedded in the JSON payload, so the data field contains only flat variant fields
- Broadcast channel capacity set to 1024 — aims for approximately 100 seconds of buffer at 10 events/second based on research recommendations
- Lagged errors (when slow clients fall behind buffer) are silently dropped via filter_map rather than terminating the stream or blocking the producer

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 3 - Blocking] AppState changes pulled forward from Task 2 into Task 1 commit**
- **Found during:** Task 1 (cargo check verification)
- **Issue:** events_handler in events.rs references `state.event_tx` which does not exist until Task 2 adds the field to AppState — the crate cannot compile with events.rs present but event_tx absent
- **Fix:** Included state.rs (event_tx field) and main.rs (broadcast::channel construction) changes in the Task 1 commit alongside the events.rs creation
- **Files modified:** crates/server/src/state.rs, crates/server/src/main.rs
- **Verification:** cargo check -p claude-history passes after Task 1 commit
- **Committed in:** b8dd82c (Task 1 commit)

---

**Total deviations:** 1 auto-fixed (1 blocking)
**Impact on plan:** Commit boundary shifted for compile coherence. Same total changes, same functionality. No scope creep.

## Issues Encountered

None — plan executed cleanly aside from the commit boundary adjustment noted above.

## User Setup Required

None - no external service configuration required.

## Next Phase Readiness

- SSE infrastructure is complete and the endpoint is live (returns empty stream with keep-alive)
- Plan 02 (file watcher) can now import SseEvent, clone event_tx from AppState, and send events through the broadcast channel
- notify 8.2 is already in workspace Cargo.toml — Plan 02 only needs to add it to the server crate's [dependencies]

---
*Phase: 04-real-time-ingestion-and-events*
*Completed: 2026-02-20*
