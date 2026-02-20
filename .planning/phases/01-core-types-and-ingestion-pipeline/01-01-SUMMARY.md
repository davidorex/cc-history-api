---
phase: 01-core-types-and-ingestion-pipeline
plan: 01
subsystem: infra
tags: [rust, cargo-workspace, sqlite, rusqlite, tokio-rusqlite, wal, migrations, thiserror]

requires: []
provides:
  - cargo workspace with 3-crate structure (core, store, server)
  - single binary at target/debug/claude-history
  - SQLite schema with 13 normalized tables and 10 indexes
  - embedded migration runner with schema_versions tracking
  - WAL mode + busy_timeout + foreign_keys pragmas
  - async DB initialization via tokio-rusqlite
  - configurable DB path (CLAUDE_HISTORY_DB_PATH env var)
  - tracing with env-filter initialization
affects: [01-02, 01-03, 01-04, phase-2, phase-3]

tech-stack:
  added: [rust, cargo, rusqlite 0.37, tokio-rusqlite 0.7, tokio 1, serde 1, serde_json 1, tracing 0.1, tracing-subscriber 0.3, clap 4, walkdir 2, anyhow 1, thiserror 2]
  patterns: [cargo workspace with path dependencies, embedded SQL migrations via include_str!, async SQLite via tokio-rusqlite conn.call closure, thiserror error enums per crate, SchemaError-to-rusqlite mapping via ToSqlConversionFailure]

key-files:
  created: [Cargo.toml, Cargo.lock, .gitignore, crates/core/Cargo.toml, crates/core/src/lib.rs, crates/store/Cargo.toml, crates/store/src/lib.rs, crates/store/src/db.rs, crates/store/src/schema.rs, crates/store/migrations/001_initial.sql, crates/server/Cargo.toml, crates/server/src/main.rs]
  modified: []

key-decisions:
  - "Pinned rusqlite to 0.37 instead of 0.38 due to tokio-rusqlite 0.7.0 depending on rusqlite 0.37 via libsqlite3-sys 0.35 — Cargo cannot link two native sqlite3 versions"
  - "Removed fts5 feature flag from rusqlite — bundled SQLite includes FTS5 by default; rusqlite 0.37 does not expose fts5 as a standalone feature"
  - "Used HOME env var directly for DB path fallback instead of adding dirs crate dependency"
  - "SchemaError mapped to rusqlite::Error via ToSqlConversionFailure inside conn.call closure — pragmatic workaround for the closure signature requiring Result<_, rusqlite::Error>"

patterns-established:
  - "Crate naming: claude-history-core, claude-history-store, claude-history (binary)"
  - "Error handling: per-crate thiserror enums (SchemaError in schema.rs, DbError in db.rs)"
  - "Migration pattern: include_str! embedding with version tracking in schema_versions table"
  - "Async SQLite: tokio-rusqlite conn.call() for all synchronous rusqlite operations"
  - "DB initialization: init_db(path) creates dirs, opens connection, sets pragmas, runs migrations"

requirements-completed: [INFRA-01, INFRA-02, INFRA-03, INFRA-07, STORE-01, STORE-02, STORE-03, STORE-04, STORE-05, STORE-06]

duration: 7min
completed: 2026-02-20
---

# Phase 1 Plan 01: Cargo Workspace + SQLite Schema Summary

**Cargo workspace with 3 crates compiling to single binary, SQLite with 13 normalized tables, WAL mode, and embedded migration runner**

## Performance

- **Duration:** ~7 min (394 seconds)
- **Started:** 2026-02-20T02:29:20Z
- **Completed:** 2026-02-20T02:35:54Z
- **Tasks:** 2
- **Files created:** 16

## Accomplishments
- Cargo workspace with core/store/server crate structure compiling to `claude-history` binary
- SQLite schema with all 13 normalized tables covering the 7 empirically-discovered JSONL record types
- 10 indexes for query performance on session_id, timestamp, message_uuid, tool_name, data_type, subtype
- Migration runner with schema_versions tracking and idempotent re-application
- WAL mode, busy_timeout(5s), synchronous=NORMAL, foreign_keys=ON configured at connection init
- DB path configurable via CLAUDE_HISTORY_DB_PATH env var with $HOME/.claude/.claude-history.db fallback

## Task Commits

Each task was committed atomically:

1. **Task 1: Create Cargo workspace with 3-crate structure and all dependencies** - `23762c9` (feat)
2. **Task 2: Create SQLite schema (all normalized tables) with embedded migration and WAL initialization** - `5ac0f0a` (feat)

## Files Created/Modified
- `Cargo.toml` - Workspace root with 3 members and shared dependency declarations
- `Cargo.lock` - Resolved dependency tree
- `.gitignore` - Excludes /target directory
- `crates/core/Cargo.toml` - Core library crate with serde, thiserror, tracing
- `crates/core/src/lib.rs` - Placeholder lib exporting empty module
- `crates/store/Cargo.toml` - Store library crate with rusqlite, tokio-rusqlite, core dependency
- `crates/store/src/lib.rs` - Exports pub mod db and pub mod schema
- `crates/store/src/db.rs` - init_db async function with pragma setup and migration call, DbError enum
- `crates/store/src/schema.rs` - Migration runner with include_str! embedding, SchemaError enum
- `crates/store/migrations/001_initial.sql` - DDL for all 13 tables and 10 indexes
- `crates/server/Cargo.toml` - Binary crate depending on core + store
- `crates/server/src/main.rs` - Entry point with tracing init and DB path resolution

## Decisions Made
- Pinned rusqlite to 0.37 (not 0.38) due to tokio-rusqlite 0.7.0 version conflict on libsqlite3-sys
- Removed fts5 feature flag — bundled SQLite includes FTS5 support without a separate feature
- Used HOME env var directly rather than adding dirs crate for DB path fallback
- SchemaError mapped to rusqlite::Error via ToSqlConversionFailure in conn.call closure

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 3 - Blocking] Downgraded rusqlite from 0.38 to 0.37**
- **Found during:** Task 1 (Cargo workspace creation)
- **Issue:** tokio-rusqlite 0.7.0 depends on rusqlite 0.37 via libsqlite3-sys 0.35; rusqlite 0.38 requires libsqlite3-sys 0.36 — Cargo cannot link two versions of the native sqlite3 library
- **Fix:** Pinned rusqlite to 0.37 in workspace dependencies
- **Impact:** Minor — rusqlite 0.37 vs 0.38 API is nearly identical for operations used
- **Committed in:** `23762c9` (Task 1 commit)

**2. [Rule 3 - Blocking] Removed fts5 feature flag from rusqlite dependency**
- **Found during:** Task 1 (Cargo workspace creation)
- **Issue:** rusqlite 0.37 does not expose fts5 as a standalone feature; bundled SQLite includes FTS5 by default
- **Fix:** Removed the fts5 feature from rusqlite dependency declaration
- **Impact:** None — FTS5 remains available via bundled SQLite
- **Committed in:** `23762c9` (Task 1 commit)

**3. [Rule 2 - Missing Critical] Added .gitignore**
- **Found during:** Task 1 (Cargo workspace creation)
- **Issue:** No .gitignore existed to exclude /target build artifacts
- **Fix:** Created .gitignore with /target exclusion
- **Impact:** None — standard Rust repository hygiene
- **Committed in:** `23762c9` (Task 1 commit)

---

**Total deviations:** 3 auto-fixed (1 missing critical, 2 blocking)
**Impact on plan:** All auto-fixes necessary for compilation or repository hygiene. No scope creep.

## Verification Results

- `cargo build`: pass
- Binary runs: pass
- `cargo test -p claude-history-store`: pass (1 test — WAL mode, foreign keys, 13 tables, 10 indexes, idempotency)
- Debug binary size: 6.6 MB (under 50 MB threshold)

## Issues Encountered
None — both tasks executed as planned after the auto-fixed deviations.

## User Setup Required
None - no external service configuration required.

## Next Phase Readiness
- Workspace compiles, binary runs, database initializes with full schema
- Ready for 01-02: serde types for JSONL record types and parser with byte-offset tracking
- All 13 tables exist for the decomposition engine (01-03) to write into
- sync_metadata table ready for incremental sync (01-04)

---
*Phase: 01-core-types-and-ingestion-pipeline*
*Plan: 01*
*Completed: 2026-02-20*
