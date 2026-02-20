---
phase: 02-full-text-search-and-cli
plan: 02
subsystem: cli
tags: [clap, search, sessions, query, stats, fts5, output-formatting, json]

# Dependency graph
requires:
  - phase: 02-full-text-search-and-cli
    plan: 01
    provides: "FTS5 index (fts::search_messages), 9 query builder functions (query::list_sessions, query_messages, token_stats_*, tool_frequency, model_breakdown)"
provides:
  - "4 CLI subcommands: search, sessions, query, stats — end-to-end operational"
  - "output.rs formatting module with print_search_results, print_sessions_table, print_stats, print_json"
  - "Global --db-path CLI option shared across all subcommands"
  - "open_db() helper for read-path subcommands with existence check and user-friendly errors"
affects: [02-03-cli-export-version-drift]

# Tech tracking
tech-stack:
  added: [chrono, csv]
  patterns:
    - "Global CLI argument (--db-path) on Cli struct with global=true, shared across subcommands"
    - "open_db() helper: resolve path, check existence, init_db — reused by all read-path subcommands"
    - "conn.call closure for async-to-sync bridge with explicit return type annotation when error type is ambiguous"
    - "Human/JSON dual output: --json flag routes to print_json, default to print_* formatters"
    - "Stats JSON combines three query results into single object with token_usage/tool_frequency/model_breakdown keys"

key-files:
  created:
    - "crates/server/src/output.rs"
  modified:
    - "Cargo.toml"
    - "crates/server/Cargo.toml"
    - "crates/server/src/main.rs"

key-decisions:
  - "Moved --db-path from Sync variant to top-level Cli struct as global arg — avoids repeating on every subcommand, consistent UX"
  - "open_db() checks db file existence before init_db — provides actionable error message suggesting sync first, rather than cryptic SQLite errors"
  - "Query subcommand always outputs JSON (no --json flag) — designed for machine consumption per spec, pipe-friendly"
  - "Stats --json combines three query results into single JSON object — avoids three separate outputs, single parseable result"
  - "csv crate added to workspace Cargo.toml proactively for Plan 03 — avoids touching workspace-level deps again"

patterns-established:
  - "Global CLI arg pattern: #[arg(long, global = true)] on Cli struct fields, accessible via cli.field_name before match"
  - "Read-path subcommand pattern: open_db() -> handler function -> output formatter"
  - "Dual output mode: all formatting functions in output.rs, handlers choose based on --json flag"
  - "Explicit closure return type: |conn| -> Result<_, tokio_rusqlite::rusqlite::Error> when multiple From impls cause ambiguity"

requirements-completed: [CLI-03, CLI-04, CLI-05, CLI-06]

# Metrics
duration: 12min
completed: 2026-02-20
---

# Phase 2 Plan 2: CLI Search, Sessions, Query, Stats Summary

**Four CLI subcommands wired end-to-end to FTS5 search and parameterized query layer with dual human-readable/JSON output formatting**

## Performance

- **Duration:** 12 min
- **Started:** 2026-02-20T04:08:55Z
- **Completed:** 2026-02-20T04:21:02Z
- **Tasks:** 2
- **Files modified:** 4

## Accomplishments
- Four new CLI subcommands (search, sessions, query, stats) operational end-to-end with real synced data
- output.rs formatting module providing column-aligned human-readable tables and pretty-printed JSON for all subcommands
- Global --db-path option shared across all subcommands, with open_db() helper providing existence checks and actionable error messages
- Stats subcommand combines three SQL aggregation queries (token usage, tool frequency, model breakdown) into unified output

## Task Commits

Each task was committed atomically:

1. **Task 1: Dependencies + Commands enum + output formatting module** - `d16d187` (feat)
2. **Task 2: Wire search, sessions, query, stats handlers** - `8c81868` (feat)

## Files Created/Modified
- `Cargo.toml` - Added chrono and csv workspace dependencies
- `crates/server/Cargo.toml` - Added serde, serde_json, chrono, tokio-rusqlite server crate dependencies
- `crates/server/src/output.rs` - Output formatting: print_search_results, print_sessions_table, print_stats, print_json
- `crates/server/src/main.rs` - Expanded Commands enum (Search, Sessions, Query, Stats), global --db-path, open_db() helper, four handler functions (run_search, run_sessions, run_query, run_stats)

## Decisions Made
- Moved --db-path to global Cli struct — shared across all subcommands without repetition
- open_db() checks file existence before init_db — provides "run sync first" suggestion rather than opaque SQLite errors
- Query subcommand has no --json flag — always JSON, designed for machine consumption per spec
- Stats --json outputs single combined object with three keys rather than three separate outputs
- csv crate added to workspace deps proactively for Plan 03 export functionality

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] Fixed --type flag clap attribute**
- **Found during:** Task 2 (wire handlers)
- **Issue:** Plan specified `#[arg(long, name = "type")]` which sets the value name, not the long flag name. This produced `--message-type` instead of the intended `--type`
- **Fix:** Changed to `#[arg(long = "type")]` which correctly produces `--type` as the CLI flag
- **Files modified:** `crates/server/src/main.rs`
- **Verification:** `claude-history query --help` shows `--type <MESSAGE_TYPE>`
- **Committed in:** 8c81868 (Task 2 commit)

**2. [Rule 3 - Blocking] Added tokio-rusqlite dependency to server crate**
- **Found during:** Task 2 (wire handlers)
- **Issue:** Server crate needed tokio_rusqlite::Connection type and rusqlite::Error re-export for conn.call() async bridge, but only had claude-history-store (which uses it internally)
- **Fix:** Added `tokio-rusqlite = { workspace = true }` to server crate dependencies
- **Files modified:** `crates/server/Cargo.toml`
- **Verification:** cargo build succeeds
- **Committed in:** 8c81868 (Task 2 commit)

**3. [Rule 3 - Blocking] Added explicit return type to stats closure**
- **Found during:** Task 2 (wire handlers)
- **Issue:** Multiple `From<rusqlite::Error>` implementations (DbError, DecomposeError, SchemaError, SyncError, tokio_rusqlite::Error) caused ambiguous error type inference in the stats conn.call closure with `?` operator
- **Fix:** Added explicit closure return type annotation `-> Result<_, tokio_rusqlite::rusqlite::Error>` to disambiguate
- **Files modified:** `crates/server/src/main.rs`
- **Verification:** cargo build compiles without type inference errors
- **Committed in:** 8c81868 (Task 2 commit)

---

**Total deviations:** 3 auto-fixed (1 bug fix, 2 blocking issues)
**Impact on plan:** All three auto-fixes were necessary for correct compilation and correct CLI behavior. No scope creep.

## Issues Encountered
None beyond the deviations documented above.

## User Setup Required
None - no external service configuration required.

## Next Phase Readiness
- All four query-facing subcommands are operational and verified with real synced data
- output.rs formatting patterns are established for Plan 03's export, version-check, and schema-drift subcommands
- open_db() helper is ready for reuse by the remaining three subcommands
- csv workspace dependency is pre-staged for export functionality

---
*Phase: 02-full-text-search-and-cli*
*Completed: 2026-02-20*
