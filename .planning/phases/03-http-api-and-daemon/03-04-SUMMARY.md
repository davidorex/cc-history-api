---
phase: 03-http-api-and-daemon
plan: 04
subsystem: infra
tags: [axum, tokio, tcp-listener, unix-domain-socket, cancellation-token, graceful-shutdown, serve, daemon, clap]

requires:
  - phase: 03-http-api-and-daemon
    provides: build_router with all 16 /v1/ routes (03-03), AppState and SharedState (03-01)
  - phase: 02-full-text-search-and-cli
    provides: CLI scaffolding with Commands enum and open_db pattern
provides:
  - run_server function binding Router to dual TCP + UDS listeners with coordinated shutdown
  - CLI `serve` subcommand with --port and --socket options
  - CancellationToken-based graceful shutdown on SIGTERM and SIGINT
  - Socket file lifecycle management (stale removal, post-shutdown cleanup)
affects: [03-05-uds-client, 03-06-cli-daemon-wiring, 04-real-time-ingestion-and-events]

tech-stack:
  added: [tokio-util (CancellationToken)]
  patterns: [dual-listener spawn pattern with shared CancellationToken, async move wrapper for 'static lifetime in with_graceful_shutdown, three-tier CLI arg fallback (flag -> env -> default)]

key-files:
  created:
    - crates/server/src/serve.rs
  modified:
    - crates/server/src/main.rs

key-decisions:
  - "CancellationToken::cancelled() requires async move wrapper for 'static lifetime in tokio::spawn -- with_graceful_shutdown(token.cancelled()) fails because cancelled() borrows the token, but spawned tasks need 'static. Wrapping in async move { token.cancelled().await } moves ownership of the cloned token into the future."

patterns-established:
  - "Dual-listener pattern: tokio::spawn separate tasks for TCP and UDS, each with cloned CancellationToken, tokio::select! on JoinHandles for coordinated completion"
  - "Socket path resolution uses three-tier fallback: CLI --socket arg, then CLAUDE_HISTORY_SOCKET env var, then /tmp/claude-history.sock default"
  - "Stale socket file removed before binding, socket file cleaned up after shutdown -- prevents 'address already in use' on restart after crash"

requirements-completed: [INFRA-04, INFRA-05, INFRA-06, UDS-01, UDS-02, CLI-01]

duration: 5min
completed: 2026-02-20
---

# Phase 03-04: Dual-Listener Serve Infrastructure Summary

**TCP + Unix domain socket serve with CancellationToken-based graceful shutdown, wired into CLI as `claude-history serve` with --port/--socket options**

## Performance

- **Duration:** 5 min
- **Started:** 2026-02-20
- **Completed:** 2026-02-20
- **Tasks:** 2
- **Files modified:** 2

## Accomplishments
- serve.rs with run_server function that binds the complete Router to both TCP (127.0.0.1:{port}) and UDS ({socket_path}) simultaneously using tokio::spawn + CancellationToken coordination
- shutdown_signal() catching SIGTERM and SIGINT via tokio::select! with cross-platform cfg branches
- CLI Serve command with --port (default 7424) and --socket (runtime-resolved via three-tier fallback)
- Previously 27 dead-code warnings from API handlers now resolved since build_router is reachable through the serve path

## Task Commits

Each task was committed atomically:

1. **Task 1: Dual-listener serve.rs with graceful shutdown** - `8bec7fa` (feat) + `d22dfe5` (chore: mod declaration)
2. **Task 2: CLI Serve command variant and main.rs integration** - `11d96f6` (feat)

**Plan metadata:** (this commit)

## Files Created/Modified
- `crates/server/src/serve.rs` - run_server function with dual TCP+UDS listeners, CancellationToken shutdown, signal handler, socket cleanup
- `crates/server/src/main.rs` - Serve command variant added to Commands enum, run_serve handler with DB open, AppState construction, and socket path resolution

## Decisions Made
- CancellationToken::cancelled() requires async move wrapper for 'static lifetime in tokio::spawn tasks -- with_graceful_shutdown(token.cancelled()) fails because cancelled() borrows the token, but the spawned async block must be 'static; wrapping in async move { token.cancelled().await } moves ownership of the cloned token into the future

## Deviations from Plan

None - plan executed as written. The async move wrapper pattern for CancellationToken was anticipated during execution as a lifetime constraint inherent to the approach, not a deviation from the plan's intent.

## Issues Encountered
- mod serve; declaration missed in initial staging for Task 1, requiring a supplementary commit (d22dfe5) to make the module visible to the compiler

## User Setup Required

None - no external service configuration required.

## Next Phase Readiness
- `claude-history serve` starts a foreground daemon binding TCP on 127.0.0.1:7424 and UDS on /tmp/claude-history.sock
- All 16 API endpoints are reachable via HTTP and UDS listeners
- Plan 03-05 can now build DaemonClient and ConnectionMode abstraction for HTTP-over-UDS client access
- Plan 03-06 can wire CLI subcommands through the daemon when socket is available

---
*Phase: 03-http-api-and-daemon*
*Completed: 2026-02-20*
