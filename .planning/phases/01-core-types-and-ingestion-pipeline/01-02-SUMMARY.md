---
phase: 01-core-types-and-ingestion-pipeline
plan: 02
subsystem: core
tags: [rust, serde, jsonl, parser, byte-offset, camelCase, untagged-enum, tagged-enum, overflow-capture, thiserror, tempfile, tracing]

requires:
  - phase: 01-core-types-and-ingestion-pipeline
    provides: cargo workspace with 3-crate structure, core crate lib.rs module declarations
provides:
  - JSONLRecord tagged enum with 7 variants for all known JSONL record types
  - RecordBase struct with full-base fields (uuid, timestamp, sessionId, etc.)
  - MessageContent untagged enum handling string and block-array dual representation
  - ContentBlock tagged enum with 4 variants (text, thinking, tool_use, tool_result)
  - AssistantMessage with UsageStats and overflow capture
  - overflow HashMap on all variable-shape structs for unknown/future field capture
  - streaming JSONL parser with byte-offset tracking and incremental sync support
  - ParseResult/ParseWarning types for error isolation without halting
affects: [01-03, 01-04, phase-2, phase-5]

tech-stack:
  added: [tempfile 3 (dev-dependency)]
  patterns: [serde(tag = "type") for externally-tagged enums, serde(untagged) for dual-representation content, serde(flatten) overflow HashMap at outermost struct level only, serde(rename_all = "camelCase") for Claude Code fields vs snake_case for Anthropic API fields, BufReader::lines() with manual byte-offset tracking]

key-files:
  created: [crates/core/src/record.rs, crates/core/src/message.rs, crates/core/src/progress.rs, crates/core/src/system.rs]
  modified: [crates/core/src/lib.rs, crates/core/src/parser.rs, crates/core/Cargo.toml, Cargo.lock]

key-decisions:
  - "sourceToolAssistantUUID requires explicit serde(rename) because rename_all=camelCase transforms source_tool_assistant_uuid to sourceToolAssistantUuid (lowercase u in uuid) but the actual JSON field uses uppercase UUID"
  - "RecordBase has no overflow HashMap — only ONE overflow per struct at outermost level to avoid serde(flatten) ambiguity between nested flattened structs"
  - "ProgressRecord stores data as serde_json::Value rather than modeling 8+ data.type variants individually — too varied and low-priority for Phase 1"
  - "ContentBlock enum variants use Option fields (caller, is_error) instead of overflow since enum variants cannot have serde(flatten)"
  - "Parser clamps final offset to min(current_offset, file_length) for files without trailing newline — partial lines at EOF will be re-read on next sync"
  - "Line-level parse failures are warnings (ParseWarning) not errors (ParseError) — only file-level I/O failures are errors"

patterns-established:
  - "Serde rename convention: #[serde(rename_all = camelCase)] for Claude Code JSON fields; snake_case (default) for Anthropic API message fields (ContentBlock, UsageStats)"
  - "Overflow capture: exactly one #[serde(flatten)] pub overflow: HashMap<String, serde_json::Value> per outermost struct, never on RecordBase or enum variants"
  - "Parser error model: ParseError (thiserror) for file-level I/O; ParseWarning for line-level deserialization failures — malformed lines never halt parsing"
  - "Byte-offset tracking: line.as_bytes().len() + 1 for newline stripped by BufReader::lines(), clamped to file length at end"
  - "Test fixture pattern: tempfile::NamedTempFile for parser integration tests with computed expected byte offsets"

requirements-completed: [CORE-01, CORE-02, CORE-03, CORE-04, CORE-05, CORE-06, CORE-07]

duration: 6min
completed: 2026-02-20
---

# Phase 1 Plan 02: Serde Types + JSONL Parser Summary

**Serde type system for all 7 JSONL record types with overflow capture, plus streaming JSONL parser with byte-offset tracking and error isolation**

## Performance

- **Duration:** ~6 min (382 seconds)
- **Started:** 2026-02-20T02:37:04Z
- **Completed:** 2026-02-20T02:43:26Z
- **Tasks:** 2
- **Files created:** 4
- **Files modified:** 4

## Accomplishments
- Complete serde type model for all 7 JSONL record types (user, assistant, progress, system, queue-operation, summary, file-history-snapshot) with tagged enum discrimination
- MessageContent untagged enum handling the dual representation (~15% plain string, ~85% block array) discovered in empirical research
- ContentBlock tagged enum covering all 4 block types (text, thinking with optional signature, tool_use with polymorphic input, tool_result with polymorphic content)
- Overflow HashMap on all variable-shape structs to capture unknown/future fields without data loss
- Streaming JSONL parser reading from arbitrary byte offsets for incremental sync, producing ParseResult with records, warnings, and new_offset
- 38 total tests (28 type deserialization + 10 parser) with zero failures and zero warnings

## Task Commits

Each task was committed atomically:

1. **Task 1: Implement serde types for all 7 JSONL record types with overflow capture** - `ce57473` (feat)
2. **Task 2: Implement streaming JSONL parser with byte-offset tracking and error isolation** - `7ca4299` (feat)

## Files Created/Modified
- `crates/core/src/record.rs` - JSONLRecord enum with 7 variants, RecordBase struct, UserRecord, AssistantRecord, QueueOperationRecord, SummaryRecord, FileHistorySnapshotRecord; 10 deserialization tests
- `crates/core/src/message.rs` - MessageContent untagged enum, ContentBlock tagged enum (4 variants), AssistantMessage, UsageStats with overflow; 12 deserialization tests
- `crates/core/src/progress.rs` - ProgressRecord with serde_json::Value data field and overflow; 3 deserialization tests
- `crates/core/src/system.rs` - SystemRecord with subtype discrimination and named common fields, overflow for subtype-specific fields; 3 deserialization tests
- `crates/core/src/lib.rs` - Module declarations for record, message, progress, system, parser
- `crates/core/src/parser.rs` - parse_jsonl function with ParseResult, ParseWarning, ParseError types; byte-offset tracking; error isolation; 10 parser tests
- `crates/core/Cargo.toml` - Added tempfile dev-dependency for parser test fixtures
- `Cargo.lock` - Updated with tempfile and transitive dependencies

## Decisions Made
- sourceToolAssistantUUID required explicit `#[serde(rename)]` due to camelCase transform producing lowercase "uuid" instead of uppercase "UUID" — caught by test immediately, one-line fix
- RecordBase deliberately has no overflow to avoid serde(flatten) ambiguity when nested via flatten in record structs
- ProgressRecord data stored as raw JSON Value — 8+ data.type variants are too varied for typed modeling in Phase 1
- Parser uses thiserror for file-level errors, warnings for line-level failures — consistent with CORE-07 error isolation requirement

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] sourceToolAssistantUUID serde rename mismatch**
- **Found during:** Task 1 (serde type implementation)
- **Issue:** `rename_all = "camelCase"` transforms `source_tool_assistant_uuid` to `sourceToolAssistantUuid` (lowercase 'u' in uuid), but the actual JSON field uses uppercase `UUID`
- **Fix:** Added `#[serde(rename = "sourceToolAssistantUUID")]` explicit annotation
- **Verification:** Test passed immediately after fix
- **Committed in:** `ce57473` (Task 1 commit)

---

**Total deviations:** 1 auto-fixed (1 bug)
**Impact on plan:** Trivial one-line annotation fix. No scope creep.

## Verification Results

- `cargo test -p claude-history-core`: 38 tests, 0 failures, 0 warnings
- All 7 record type variants tested with representative JSON
- Overflow HashMap non-empty for records with simulated unknown fields
- MessageContent handles both string and block-array forms
- ContentBlock handles all 4 block types (text, thinking, tool_use, tool_result)
- UsageStats overflow captures unknown billing fields
- Parser byte offsets verified with computed expected values across multi-line files
- Parser error isolation: malformed lines produce warnings, surrounding valid records still parsed

## Issues Encountered
None — both tasks executed as planned after the single auto-fixed serde rename deviation.

## User Setup Required
None - no external service configuration required.

## Next Phase Readiness
- All 7 record types fully modeled and tested, ready for decomposition engine (01-03) to pattern-match against
- Parser with byte-offset returns the exact offset the sync engine (01-04) needs for incremental processing
- ParseWarning type provides the diagnostic structure the decomposition engine can surface
- Overflow HashMap on all variable-shape structs means schema drift logger (01-03) has fields to detect and record

---
*Phase: 01-core-types-and-ingestion-pipeline*
*Plan: 02*
*Completed: 2026-02-20*
