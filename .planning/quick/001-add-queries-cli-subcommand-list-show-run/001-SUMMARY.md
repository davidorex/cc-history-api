---
phase: quick
plan: 01
subsystem: cli
tags: [clap, toml, sql, query-registry, named-params]

# Dependency graph
requires:
  - phase: 03-query-layer
    provides: sql_passthrough module for read-only SQL execution
provides:
  - query_registry module with CannedQuery struct and named-to-positional param conversion
  - queries CLI subcommand group (list, show, run)
  - 3 seed SQL query files with .toml sidecar metadata
  - resolve_queries_dir for configurable query directory ($CLAUDE_HISTORY_QUERIES or ~/.claude/claude-history/queries/)
affects: [api, cli]

# Tech tracking
tech-stack:
  added: [toml 0.8]
  patterns: [named-param-to-positional conversion, .sql+.toml sidecar pattern, filesystem-only vs DB-required subcommand routing]

key-files:
  created:
    - crates/store/src/query_registry.rs
    - queries/recent-sessions.sql
    - queries/recent-sessions.toml
    - queries/tool-usage-by-session.sql
    - queries/tool-usage-by-session.toml
    - queries/message-search-context.sql
    - queries/message-search-context.toml
  modified:
    - Cargo.toml
    - crates/store/Cargo.toml
    - crates/store/src/lib.rs
    - crates/server/src/main.rs
    - crates/server/src/output.rs

key-decisions:
  - "Queries list/show routed at top-level match alongside Serve/Sync (no DB needed), only Run resolves connection mode"
  - "All query run output defaults to JSON for consistency with sql_passthrough behavior"
  - "Seed queries corrected to match actual schema (sessions.first_seen_at not first_message_at, tool_executions not tool_blocks, message_content not text_blocks)"

patterns-established:
  - "Sidecar pattern: .sql file + optional .toml for metadata, auto-discover params from SQL when no sidecar"
  - "Named param extraction via character-level state machine skipping single-quoted strings"

requirements-completed: []

# Metrics
duration: 6min
completed: 2026-02-22
---

# Quick Task 001: Add Queries CLI Subcommand Summary

**Canned SQL query system with .sql+.toml sidecar loading, named :param to ?N positional conversion, and list/show/run CLI subcommands**

## Performance

- **Duration:** 5 min 39 sec
- **Started:** 2026-02-22T11:51:20Z
- **Completed:** 2026-02-22T11:56:59Z
- **Tasks:** 2
- **Files modified:** 13

## Accomplishments
- query_registry module in store crate: CannedQuery/ParamDef structs, load_queries from directory, extract_named_params state machine, prepare_sql for named-to-positional conversion with default resolution
- Queries subcommand group: `queries list` shows table of available queries, `queries show <name>` displays SQL/metadata, `queries run <name> --param key=value` executes through sql_passthrough with param binding
- 3 seed query files: recent-sessions (sessions with message count), tool-usage-by-session (tool execution breakdown), message-search-context (LIKE-based content search)
- 11 unit tests for param extraction, positional conversion, defaults, error handling, quoted string skipping

## Task Commits

Each task was committed atomically:

1. **Task 1: Add toml dependency and create query_registry module** - `ff0c174` (feat)
2. **Task 2: Add Queries subcommand group with list/show/run and seed queries** - `16a252b` (feat)

## Files Created/Modified
- `crates/store/src/query_registry.rs` - CannedQuery struct, load_queries, extract_named_params, prepare_sql, resolve_queries_dir, 11 unit tests
- `crates/server/src/main.rs` - QueriesAction enum, parse_key_val, Queries routing, run_queries_list/show/run handlers
- `crates/server/src/output.rs` - print_queries_list table formatter
- `Cargo.toml` - Added toml 0.8 workspace dependency
- `crates/store/Cargo.toml` - Added toml workspace reference
- `crates/store/src/lib.rs` - Added pub mod query_registry
- `queries/recent-sessions.sql` - Sessions list with message count via JOIN
- `queries/recent-sessions.toml` - Metadata: description, limit param with default=20
- `queries/tool-usage-by-session.sql` - Tool execution breakdown per session
- `queries/tool-usage-by-session.toml` - Metadata: description, session_id param (required)
- `queries/message-search-context.sql` - LIKE content search via message_content
- `queries/message-search-context.toml` - Metadata: description, search_term (required), limit (default=50)

## Decisions Made
- Routed list/show at top-level match (no DB needed) rather than inside the read_cmd catch-all that resolves ConnectionMode. Only `run` needs a database connection.
- Query `run` always outputs JSON regardless of --json flag, consistent with the sql passthrough endpoint behavior.
- For daemon mode, `run` falls back to direct DB access since the daemon's sql endpoint expects raw SQL, not canned query names.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] Corrected seed query SQL to match actual database schema**
- **Found during:** Task 2 (seed query creation and verification)
- **Issue:** Plan specified column names that do not exist in the actual schema: `first_message_at`/`last_message_at`/`message_count`/`model` on sessions table (actual: `first_seen_at`/`last_seen_at`, no message_count/model columns). Plan referenced `tool_blocks` and `text_blocks` tables (actual: `tool_executions` and `message_content`).
- **Fix:** Rewrote recent-sessions.sql to use `first_seen_at`/`last_seen_at` with LEFT JOIN + COUNT for message_count and subquery for model. Rewrote tool-usage-by-session.sql to use `tool_executions` table. Rewrote message-search-context.sql to use `message_content` table with `text_content` column.
- **Files modified:** queries/recent-sessions.sql, queries/tool-usage-by-session.sql, queries/message-search-context.sql
- **Verification:** `cargo run -- queries run recent-sessions --queries-dir ./queries --json` returned 20 rows with correct data from live database
- **Committed in:** 16a252b (Task 2 commit)

---

**Total deviations:** 1 auto-fixed (1 bug)
**Impact on plan:** Column/table name correction was necessary for the seed queries to actually execute against the real database. No scope creep.

## Issues Encountered
None beyond the schema mismatch documented above.

## User Setup Required
None - no external service configuration required. Users can optionally copy seed queries to ~/.claude/claude-history/queries/ or set $CLAUDE_HISTORY_QUERIES to a custom directory, or use --queries-dir on each invocation.

## Next Phase Readiness
- Query registry module is available for potential HTTP API endpoint exposure in the future
- Additional seed queries can be added to the queries/ directory

## Self-Check: PASSED

- All 10 created/modified files verified present on disk
- Commit ff0c174 (Task 1) verified in git log
- Commit 16a252b (Task 2) verified in git log
- All 159 tests pass (38 core + 121 store)
- `queries list`, `queries show`, `queries run` verified functional against live database

---
*Quick Task: 001-add-queries-cli-subcommand-list-show-run*
*Completed: 2026-02-22*
