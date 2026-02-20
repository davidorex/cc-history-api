---
phase: 04-real-time-ingestion-and-events
plan: 02
subsystem: server
tags: [notify, filesystem-watcher, debounce, sse, live-ingestion, fts5]

# Dependency graph
requires:
  - phase: 04-01
    provides: "SseEvent enum, broadcast channel in AppState, GET /v1/events SSE endpoint"
  - phase: 01-04
    provides: "sync_file and extract_session_id functions for incremental JSONL ingestion"
  - phase: 02-01
    provides: "rebuild_fts_index for periodic FTS5 index maintenance"
provides:
  - "FileDebouncer for 2-second per-file change coalescing"
  - "spawn_watcher on std::thread with notify::recommended_watcher for recursive filesystem monitoring"
  - "watcher_loop async task emitting all 4 SSE event types based on sync results"
  - "Version change detection comparing session version against cached last_known_version"
  - "Periodic FTS5 rebuild on 30-second timer (only when new data ingested)"
  - "--projects-dir CLI argument for Serve subcommand with CLAUDE_PROJECTS_DIR env fallback"
affects: [phase-05-artifact-layer, phase-06-polish]

# Tech tracking
tech-stack:
  added: [notify 8.2 with macos_fsevent feature]
  patterns: [std-thread-for-blocking-watcher, mpsc-bridge-to-tokio, oneshot-setup-error-propagation]

key-files:
  created:
    - crates/server/src/watcher.rs
  modified:
    - crates/server/Cargo.toml
    - crates/server/src/serve.rs
    - crates/server/src/main.rs

key-decisions:
  - "Removed clap env attribute for projects_dir — derive feature alone does not include env support; handled via resolve_projects_dir with explicit CLAUDE_PROJECTS_DIR env check"
  - "Watcher startup failure logs warning and continues — live ingestion is optional for basic daemon operation"
  - "Oneshot channel propagates watcher setup errors back to caller for proper error handling before thread parks"

patterns-established:
  - "std::thread for long-lived blocking subsystems: notify watcher uses std::thread::spawn + park() rather than tokio::spawn_blocking to avoid occupying the blocking thread pool"
  - "mpsc bridge pattern: blocking OS callbacks forward events to tokio tasks via mpsc channel with blocking_send"
  - "Graceful degradation: watcher failure does not prevent daemon from serving API requests"

requirements-completed: [WATCH-01, WATCH-02, WATCH-03, SSE-02, SSE-03, SSE-04, SSE-05]

# Metrics
duration: 5min
completed: 2026-02-20
---

# Phase 4 Plan 2: File Watcher Summary

**Notify-based filesystem watcher with per-file debounce, live JSONL ingestion via sync_file, and all 4 SSE event types emitted through broadcast channel**

## Performance

- **Duration:** 5 min (~271 seconds)
- **Started:** 2026-02-20T11:06:17Z
- **Completed:** 2026-02-20T11:10:48Z
- **Tasks:** 2
- **Files modified:** 4

## Accomplishments
- Complete live-ingestion pipeline: filesystem change -> notify -> mpsc -> watcher_loop -> sync_file -> broadcast -> SSE event stream
- FileDebouncer with 2-second per-file debounce and periodic stale-entry pruning for bounded memory growth
- All 4 SSE event types emitted based on sync results: record:added, session:started, schema:drift, version:changed
- Version change detection initialized from DB at startup to avoid spurious events on daemon restart
- Periodic FTS5 rebuild every 30 seconds (only when new data was ingested) rather than per-file

## Task Commits

Each task was committed atomically:

1. **Task 1: Create watcher module with FileDebouncer, spawn_watcher, and watcher_loop** - `112c4bc` (feat)
2. **Task 2: Wire watcher into serve.rs and update main.rs with projects_dir parameter** - `074ad6e` (feat)

## Files Created/Modified
- `crates/server/src/watcher.rs` - FileDebouncer, spawn_watcher (std::thread + notify), watcher_loop (tokio::select! with event processing, FTS timer, cancellation), is_new_file, check_version_change (495 lines)
- `crates/server/Cargo.toml` - Added notify workspace dependency
- `crates/server/src/serve.rs` - Updated run_server to accept projects_dir, spawn watcher thread and watcher_loop task with error recovery
- `crates/server/src/main.rs` - Added --projects-dir to Serve subcommand, updated resolve_projects_dir to check CLAUDE_PROJECTS_DIR env, wired projects_dir into run_server call

## Decisions Made
- Removed clap `env` attribute for projects_dir — the `derive` feature alone does not include env support in clap 4 without the `env` feature flag. Instead, `CLAUDE_PROJECTS_DIR` is handled by `resolve_projects_dir` using `std::env::var`, consistent with the existing `resolve_db_path` pattern.
- Watcher startup failure is non-fatal — logs a warning and continues serving API requests from existing data. This allows the daemon to start even if the projects directory does not yet exist (first-time Claude Code users).
- Used oneshot channel (std::sync::mpsc) to propagate watcher setup errors from the spawned thread back to the caller, rather than logging and hoping.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 3 - Blocking] Removed clap env attribute that required missing feature flag**
- **Found during:** Task 2 (Wire watcher into serve.rs)
- **Issue:** Plan specified `#[arg(long, env = "CLAUDE_PROJECTS_DIR")]` but clap's `env` attribute requires the `env` feature flag which is not enabled in the workspace (only `derive` is configured)
- **Fix:** Removed `env` attribute from the clap arg and added explicit `CLAUDE_PROJECTS_DIR` env var check to `resolve_projects_dir` function, maintaining the same resolution priority
- **Files modified:** crates/server/src/main.rs
- **Verification:** `cargo build -p claude-history` succeeds
- **Committed in:** 074ad6e (Task 2 commit)

---

**Total deviations:** 1 auto-fixed (1 blocking)
**Impact on plan:** Auto-fix necessary because the specified clap attribute was incompatible with the workspace's feature configuration. The alternative approach (explicit env var check) is equally correct and consistent with existing patterns.

## Issues Encountered
None beyond the deviation noted above.

## User Setup Required
None - no external service configuration required.

## Next Phase Readiness
- Phase 4 complete: both SSE event infrastructure (Plan 01) and file watcher (Plan 02) are operational
- The daemon now automatically detects .jsonl file changes and streams events to SSE clients
- Ready for Phase 5 (Artifact Layer) which may extend the watcher to handle additional file types
- Manual integration testing recommended: run `claude-history serve`, create/modify a .jsonl file in ~/.claude/projects/, and observe SSE events at http://localhost:7424/v1/events

---
*Phase: 04-real-time-ingestion-and-events*
*Completed: 2026-02-20*
