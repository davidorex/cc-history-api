---
phase: 05-artifact-layer
plan: 05
subsystem: database
tags: [sqlite, fts5, full-text-search, file-operations, bm25, snippet]

requires:
  - phase: 05-01
    provides: "migration 003 creating fts_file_operations FTS5 external-content virtual table"
provides:
  - "rebuild_fts_file_operations function for periodic FTS5 index rebuild"
  - "search_file_operations function with BM25 ranking, snippet extraction, and pagination"
  - "FileOperationSearchResult struct with Serialize/Deserialize for API use"
affects: [05-06, 05-08]

tech-stack:
  added: []
  patterns:
    - "FTS5 external-content rebuild command pattern reused from message content index"
    - "Double-quote query sanitization for FTS5 MATCH (same pattern as search_messages)"
    - "snippet() function for context extraction with configurable markers and token count"

key-files:
  created: []
  modified:
    - "crates/store/src/fts.rs"
    - "crates/store/migrations/003_artifacts.sql"

key-decisions:
  - "Fixed migration 003 FTS5 column names from content_col/old_content_col/command_col to content/old_content/command -- FTS5 external-content rebuild reads source table columns using FTS column names, so they must match. Same pattern as migration 002 (text_content matches message_content.text_content)."

patterns-established:
  - "PAT-037: FTS5 file_operations search uses same sanitization and ranking pattern as message content search -- both wrap user input in double quotes and ORDER BY rank"

requirements-completed: [FTS-02]

duration: 3min
completed: 2026-02-20
---

# Phase 5 Plan 5: FTS5 File Operations Search Summary

**FTS5 rebuild and search functions for file_operations content index -- BM25-ranked search over written files, edit strings, and bash commands with snippet extraction**

## Performance

- **Duration:** 3 min
- **Started:** 2026-02-20T13:24:00Z
- **Completed:** 2026-02-20T13:27:00Z
- **Tasks:** 1
- **Files modified:** 2

## Accomplishments
- rebuild_fts_file_operations function issues FTS5 'rebuild' command against fts_file_operations external-content virtual table, intended for periodic use alongside existing rebuild_fts_index
- search_file_operations performs FTS5 MATCH query joining fts_file_operations with file_operations on rowid, using snippet() for context extraction and BM25 ranking with pagination support
- FileOperationSearchResult struct with id, session_id, file_path, operation_type, snippet, timestamp, and rank fields, deriving Serialize/Deserialize for API use
- Fixed migration 003_artifacts.sql FTS5 column name mismatch (content_col/old_content_col/command_col -> content/old_content/command) that caused "no such column: T.content_col" errors during rebuild
- Three tests covering empty-table rebuild, result-returning search with field verification, and empty-result for non-matching queries

## Task Commits

1. **Task 1: Add rebuild_fts_file_operations and search_file_operations to fts.rs** - `6cbe431` (feat)

## Files Created/Modified
- `crates/store/src/fts.rs` - Added FileOperationSearchResult struct, rebuild_fts_file_operations, search_file_operations, 3 tests, updated module doc comment (+272 lines)
- `crates/store/migrations/003_artifacts.sql` - Fixed FTS5 column names from _col suffixed aliases to match source table column names (+3/-3 lines)

## Decisions Made
- [05-05-D1]: Fixed migration 003 FTS5 column names from content_col/old_content_col/command_col to content/old_content/command -- FTS5 external-content rebuild reads source table columns using FTS column names, so they must match. Same pattern as migration 002 (text_content matches message_content.text_content).

## Deviations from Plan

Migration 003_artifacts.sql modified to fix FTS5 column name mismatch. The original column aliases (content_col, old_content_col, command_col) did not match the source table column names (content, old_content, command), causing "no such column: T.content_col" errors on FTS5 rebuild. This was a pre-existing defect in the migration from plan 05-01.

**Total deviations:** 1 (migration fix for pre-existing column name mismatch)
**Impact on plan:** Necessary for FTS5 rebuild to function. The column renaming aligns with the existing pattern in migration 002.

## Issues Encountered
None beyond the migration fix noted above.

## User Setup Required
None -- no external service configuration required.

## Next Phase Readiness
- FTS5 search for file_operations is now available via `claude_history_store::fts::search_file_operations` and `rebuild_fts_file_operations`
- API handler plan (05-06) can wire POST /v1/files/search endpoint to search_file_operations
- Watcher integration plan (05-08) can call rebuild_fts_file_operations periodically alongside the existing rebuild_fts_index

---
*Phase: 05-artifact-layer*
*Completed: 2026-02-20*
