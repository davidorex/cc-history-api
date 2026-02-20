---
phase: 03-http-api-and-daemon
plan: 05
subsystem: api
tags: [hyper, uds, http-client, daemon-client, unix-domain-socket, connection-mode]

requires:
  - phase: 03-http-api-and-daemon
    provides: axum HTTP API with 16 endpoints and dual TCP+UDS listeners
provides:
  - DaemonClient struct with HTTP-over-UDS transport via hyper 1.x handshake
  - 10 endpoint methods mapping to /v1/ API routes
  - ConnectionMode enum (Daemon/Direct) for CLI dispatch abstraction
  - resolve_socket_path with CLI arg > env > default resolution
  - detect_connection_mode with socket health probing before mode selection
  - Deserialize derives on all store query result types and HealthResponse
affects: [03-06-cli-daemon-wiring, phase-4-realtime]

tech-stack:
  added: [hyper 1 (client+http1), hyper-util 0.1 (tokio), http-body-util 0.1]
  patterns: [hyper http1::handshake over TokioIo-wrapped UnixStream, ConnectionMode enum dispatch]

key-files:
  created: [crates/server/src/daemon_client.rs]
  modified: [Cargo.toml, crates/server/Cargo.toml, crates/store/src/query.rs, crates/store/src/fts.rs, crates/server/src/api/health.rs, crates/server/src/main.rs]

key-decisions:
  - "Added serde::Deserialize to all store query result structs and HealthResponse — DaemonClient needs to deserialize daemon JSON responses into the same types the store layer produces"
  - "export_session() returns raw Vec<u8> instead of typed JSON — export responses may be markdown or CSV depending on format parameter"
  - "Minimal custom urlencoded() function instead of percent-encoding crate — query parameter values are simple strings, full crate is excessive"

patterns-established:
  - "DaemonClient methods return the same types as store-layer query functions — callers operate identically regardless of data source"
  - "ConnectionMode enum pattern-matching separates daemon-connected and direct-DB code paths at CLI dispatch level"
  - "resolve_socket_path follows CLI arg > CLAUDE_HISTORY_SOCKET env > /tmp/claude-history.sock default precedence"
  - "detect_connection_mode probes socket with health check before committing to Daemon mode — stale socket files do not cause false positives"

requirements-completed: [CLI-15]

duration: 5min
completed: 2026-02-20
---

# Plan 03-05: DaemonClient and ConnectionMode Summary

**HTTP-over-UDS client with hyper 1.x handshake, 10 endpoint methods, and ConnectionMode enum for CLI daemon/direct dispatch**

## Performance

- **Duration:** 5 min
- **Started:** 2026-02-20
- **Completed:** 2026-02-20
- **Tasks:** 2
- **Files modified:** 8

## Accomplishments
- DaemonClient sends HTTP/1.1 requests over Unix domain socket using hyper's http1::handshake with TokioIo-wrapped UnixStream
- 10 endpoint methods cover all CLI subcommand needs: health, sessions, search, query_messages, stats_tokens, stats_tools, stats_models, export_session, version_history, schema_drift
- ConnectionMode enum cleanly separates Daemon(DaemonClient) and Direct{conn, db_path} code paths
- detect_connection_mode probes socket health before choosing Daemon mode, falling back to Direct on failure
- Added Deserialize to 16 store/server types so DaemonClient can deserialize responses into the same types used throughout the codebase

## Task Commits

Each task was committed atomically:

1. **Task 1: Add UDS HTTP client dependencies to server crate** - `1de6af4` (feat)
2. **Task 2: Implement DaemonClient with HTTP-over-UDS and ConnectionMode enum** - `33d13aa` (feat)

**Plan metadata:** _(pending this commit)_ (docs: complete plan)

## Files Created/Modified
- `crates/server/src/daemon_client.rs` - DaemonClient struct, ConnectionMode enum, DaemonError, resolve_socket_path, detect_connection_mode (549 lines)
- `Cargo.toml` - hyper, hyper-util, http-body-util added to workspace dependencies
- `crates/server/Cargo.toml` - workspace dependency references for the three new crates
- `crates/store/src/query.rs` - Added Deserialize derive to 12 query result structs
- `crates/store/src/fts.rs` - Added Deserialize derive to 2 FTS result structs
- `crates/server/src/api/health.rs` - Added Deserialize derive to HealthResponse
- `crates/server/src/main.rs` - Added mod daemon_client declaration
- `Cargo.lock` - 351 lines of new transitive dependency entries

## Decisions Made
- **03-05-D1:** Added Deserialize to all store query result structs and HealthResponse — these types previously only had Serialize, but DaemonClient deserialization requires both. Backward-compatible change.
- **03-05-D2:** export_session() returns raw Vec<u8> instead of typed JSON deserialization — export endpoint returns different Content-Types (markdown, CSV, JSON) depending on format parameter.
- **03-05-D3:** Minimal custom urlencoded() function instead of adding a percent-encoding crate — query parameter values are simple strings where RFC 3986 unreserved character handling suffices.

## Deviations from Plan

### Auto-fixed Issues

**1. [Critical Missing Piece] Added Deserialize derives to 16 store/server types**
- **Found during:** Task 2 (DaemonClient implementation)
- **Issue:** Store query result structs and HealthResponse only derived Serialize — DaemonClient cannot deserialize daemon JSON responses without Deserialize
- **Fix:** Added `Deserialize` to derive macros on all affected types in query.rs, fts.rs, and health.rs
- **Files modified:** crates/store/src/query.rs, crates/store/src/fts.rs, crates/server/src/api/health.rs
- **Verification:** cargo check --workspace compiles clean
- **Committed in:** 33d13aa (Task 2 commit)

---

**Total deviations:** 1 auto-fixed (1 critical missing piece)
**Impact on plan:** The Deserialize addition is backward-compatible and was implicitly required by the plan's must_haves. No scope creep.

## Issues Encountered
None — both tasks compiled clean (Task 2 required the Deserialize deviation but compiled on first attempt after the fix).

## User Setup Required
None - no external service configuration required.

## Next Phase Readiness
- DaemonClient and ConnectionMode are ready for Plan 03-06 to wire all CLI subcommands through daemon when socket is available
- All store types now have both Serialize and Deserialize, so CLI formatting code works identically regardless of data source

---
*Phase: 03-http-api-and-daemon*
*Completed: 2026-02-20*
