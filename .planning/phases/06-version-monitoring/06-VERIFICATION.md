---
phase: 06-version-monitoring
verified: 2026-02-21T12:00:00Z
status: passed
score: 3/3 must-haves verified
re_verification: false
human_verification:
  - test: "Confirm 'schema_versions' in success criterion 3 maps to 'version_history' table"
    expected: "Roadmap SC-3 says 'logs them to schema_versions' but implementation uses version_history table (deliberate naming to avoid collision with migration tracker). Verify this naming decision is acceptable."
    why_human: "Roadmap text uses 'schema_versions'; codebase uses 'version_history'. The research doc and plans document the rename explicitly, but the roadmap wording was not updated. Human confirmation needed that the table name is acceptable as-is."
  - test: "Confirm 'periodic check loop' in SC-3 is satisfied by ingestion-triggered detection"
    expected: "SC-3 says 'periodic check loop'; implementation fires version checks on every file sync event (ingestion-triggered). The CONTEXT.md and RESEARCH.md both document this as an intentional design decision — 'detection is ingestion-only — no separate periodic polling loop'. Verify this satisfies the intent."
    why_human: "The roadmap says 'periodic check loop'; the implementation uses ingestion-triggered detection. The team explicitly locked this decision in CONTEXT.md and RESEARCH.md. No code can verify intent acceptability."
---

# Phase 6: Version Monitoring Verification Report

**Phase Goal:** The daemon actively tracks Claude Code version changes and provides actionable schema drift analysis across versions
**Verified:** 2026-02-21
**Status:** passed
**Re-verification:** No — initial verification

## Goal Achievement

### Observable Truths

| # | Truth | Status | Evidence |
|---|-------|--------|---------|
| 1 | `claude-history version-check` (and GET /v1/schema/versions) shows the detected Claude Code version and a history of version changes with timestamps | VERIFIED | `run_version_check()` in `main.rs:945` calls `version_history_enhanced()` in direct mode and `client.version_history()` in daemon mode. API handler `versions()` in `api/schema.rs:118` returns `VersionHistoryEntry` with `version`, `first_seen_at`, `last_seen_at`, `session_count`, `new_fields_count`. CLI formats as 5-column table via `output::print_version_history()`. |
| 2 | GET /v1/schema/drift shows overflow fields grouped by version, highlighting new fields that appeared when Claude Code updated | VERIFIED | `drift()` handler in `api/schema.rs:146` calls `drift_by_version()` returning `Vec<VersionDriftGroup>` grouped by version then record_type. Each field has `promotion_status` (promoted/extra_json/unhandled), `occurrence_count`, and `sample_value`. `?diff=true` on versions endpoint returns `VersionDiffEntry` with `new_fields` and `disappeared_fields`. |
| 3 | In daemon mode, a periodic check loop detects version changes and logs them to version_history (schema_versions in roadmap text) without requiring a restart | VERIFIED | `check_version_change()` in `watcher.rs:376` fires on every ingested file sync event — no restart required. After detecting a change, persists to `version_history` table via `INSERT INTO version_history ... ON CONFLICT DO UPDATE`. Startup backfill in `watcher_loop()` at `watcher.rs:505` populates historical versions via `INSERT OR IGNORE`. Roadmap uses name "schema_versions" but the table is deliberately named "version_history" to avoid collision with migration tracker (documented in CONTEXT.md, RESEARCH.md, and 06-01-SUMMARY.md). |

**Score:** 3/3 truths verified

---

### Required Artifacts

| Artifact | Expected | Status | Details |
|----------|---------|--------|---------|
| `crates/store/migrations/006_version_monitoring.sql` | DDL for version_history, ALTER TABLE messages and schema_drift_log, backfill queries, 7 view recreations | VERIFIED | 231 lines. Contains all 6 sections: version_history DDL, sessions backfill, drift correlation backfill (correlated subquery), messages ALTER TABLE (3 columns), schema_drift_log ALTER TABLE (2 columns), 7 DROP/CREATE view statements with `is_compact_summary = 0` filter. |
| `crates/store/src/schema.rs` | Migration 006 in MIGRATIONS array, tests for new table/columns/views | VERIFIED | `("006", include_str!("../migrations/006_version_monitoring.sql"))` at line 17. Six new tests: `migration_006_creates_version_history_table`, `migration_006_adds_messages_columns`, `migration_006_adds_schema_drift_log_columns`, `migration_006_views_are_queryable`, `migration_006_idempotent`, `migration_006_version_history_not_schema_versions`. |
| `crates/store/src/drift.rs` | INSERT ON CONFLICT with occurrence_count increment and last_seen_at update | VERIFIED | Lines 63-70 show `INSERT INTO schema_drift_log ... ON CONFLICT(field_name, record_type, version) DO UPDATE SET occurrence_count = schema_drift_log.occurrence_count + 1, last_seen_at = datetime('now')`. Two relevant tests: `test_log_overflow_idempotent` (verifies occurrence_count=2 after second call) and `test_log_overflow_occurrence_count`. |
| `crates/store/src/decompose.rs` | Compact summary field extraction, extra_json population | VERIFIED | `decompose_user()` lines 409-431: extracts `isCompactSummary` -> `is_compact_summary`, `sourceToolUseID` -> `source_tool_use_id`, removes promoted keys from overflow clone, serializes remainder to `extra_json`, UPDATEs messages row. `decompose_assistant()` lines 496-526: merges record-level and message-level overflow into combined `extra_json`. Four new tests including `test_decompose_user_compact_summary`, `test_decompose_user_extra_json`, `test_decompose_user_no_compact_summary`, `test_decompose_assistant_extra_json`. |
| `crates/server/src/watcher.rs` | Version persistence in check_version_change, startup backfill | VERIFIED | `check_version_change()` lines 413-435: after SSE emit, runs `INSERT INTO version_history ... ON CONFLICT DO UPDATE SET last_seen_at, session_count + 1`. Startup backfill lines 505-527 in `watcher_loop()`. Failure logged at warn level, does not block event delivery. |
| `crates/store/src/query.rs` | VersionHistoryEntry, version_history_enhanced(), VersionDriftGroup, drift_by_version() | VERIFIED | Structs defined: `VersionHistoryEntry` (6 fields), `DriftFieldEntry` (6 fields), `VersionDriftGroup`, `RecordTypeDriftGroup`, `VersionDiffEntry`. Functions: `version_history_enhanced()` queries version_history table; `version_history_with_diff()` computes new/disappeared fields; `drift_by_version()` groups by version+record_type with PRAGMA table_info promotion status. |
| `crates/server/src/api/schema.rs` | Enhanced versions handler with ?diff=true, enhanced drift handler | VERIFIED | `VersionsParams` struct with `diff: Option<bool>`. `VersionsResponse` untagged enum routing to `version_history_enhanced()` or `version_history_with_diff()`. `drift()` handler calls `drift_by_version()` with post-retrieval record_type filtering and field-count limiting. |
| `crates/server/src/main.rs` | Enhanced run_version_check and run_schema_drift | VERIFIED | `run_version_check()` at line 945: daemon mode calls `client.version_history()`, direct mode calls `version_history_enhanced()`. `run_schema_drift()` at line 998: daemon mode calls `client.schema_drift_grouped()`, direct mode calls `drift_by_version()`. Both call output formatting functions. |
| `crates/server/src/output.rs` | print_version_history, print_drift_grouped | VERIFIED | `print_version_history()` at line 369: 5-column table (VERSION, FIRST_SEEN, LAST_SEEN, SESSIONS, NEW_FIELDS). `print_drift_grouped()` at line 425: hierarchical format with Version headers, Record Type subheaders, FIELD/OCCURRENCES/STATUS/SAMPLE rows. |

---

### Key Link Verification

| From | To | Via | Status | Details |
|------|----|-----|--------|---------|
| `crates/store/src/schema.rs` | `crates/store/migrations/006_version_monitoring.sql` | `include_str!` in MIGRATIONS array | WIRED | Line 17: `("006", include_str!("../migrations/006_version_monitoring.sql"))` |
| `crates/server/src/watcher.rs` | `version_history` table | `INSERT INTO version_history` in `check_version_change` | WIRED | Lines 419-424: full INSERT ON CONFLICT statement with version, first_seen_at, last_seen_at, session_id, session_count fields |
| `crates/store/src/query.rs` | `version_history` table | `SELECT from version_history` in `version_history_enhanced` | WIRED | Line 1229: `SELECT version, first_seen_at, last_seen_at, session_id, session_count, new_fields_count FROM version_history ORDER BY first_seen_at ASC` |
| `crates/server/src/api/schema.rs` | `crates/store/src/query.rs` | `version_history_enhanced` and `drift_by_version` calls | WIRED | Lines 127, 133, 152: direct calls to `claude_history_store::query::version_history_with_diff`, `version_history_enhanced`, `drift_by_version` |
| `crates/server/src/main.rs` | `crates/store/src/query.rs` | Direct DB mode calls | WIRED | Lines 962, 1024: `claude_history_store::query::version_history_enhanced(conn)` and `claude_history_store::query::drift_by_version(conn)` |
| `crates/server/src/decompose.rs` | `messages.is_compact_summary` | UPDATE messages SET is_compact_summary | WIRED | Line 429: `UPDATE messages SET is_compact_summary = ?1, source_tool_use_id = ?2, extra_json = ?3 WHERE uuid = ?4` |
| `crates/store/src/drift.rs` | `schema_drift_log.occurrence_count` | ON CONFLICT DO UPDATE SET occurrence_count | WIRED | Lines 66-68: `ON CONFLICT(field_name, record_type, version) DO UPDATE SET occurrence_count = schema_drift_log.occurrence_count + 1, last_seen_at = datetime('now')` |
| `crates/server/src/daemon_client.rs` | API enhanced response types | `version_history()` returns `VersionHistoryEntry`, `schema_drift_grouped()` returns `VersionDriftGroup` | WIRED | Lines 34-36 import `VersionDriftGroup, VersionHistoryEntry`. `version_history()` at line 405, `schema_drift_grouped()` at line 414. |

---

### Requirements Coverage

| Requirement | Source Plans | Description | Status | Evidence |
|-------------|-------------|-------------|--------|---------|
| VER-01 | 06-03, 06-04 | Detect Claude Code version from most recent JSONL record | SATISFIED | `check_version_change()` queries `sessions.version` field (populated from JSONL during ingestion). `version_history_enhanced()` exposes this timeline. API and CLI both present version history. Detection is ingestion-triggered (CONTEXT.md documents this as intentional — no CLI/npm probing). |
| VER-02 | 06-01, 06-04 | Record version changes in schema_versions table | SATISFIED | Table is named `version_history` (deliberately, to avoid collision with `schema_versions` migration tracker). Watcher persists on each detected change. Migration 006 backfills from sessions. The naming divergence from the requirement text is documented in research, plan, and summary. |
| VER-03 | 06-02, 06-04 | Compare overflow field sets between versions to detect new fields | SATISFIED | `version_history_with_diff()` computes `new_fields` and `disappeared_fields` per version by cross-referencing `schema_drift_log` entries. `drift_by_version()` groups all drift fields by version. `GET /v1/schema/versions?diff=true` exposes field diffs. |
| VER-04 | 06-03 | Periodic version check loop in daemon mode | SATISFIED (redefined) | CONTEXT.md explicitly locks the decision: "Detection is ingestion-only — no separate periodic polling loop; next session reveals any version change." `check_version_change()` fires on every sync event in `watcher_loop()`, requiring no restart. Startup backfill ensures all historical versions are captured immediately. |

---

### Anti-Patterns Found

| File | Pattern | Severity | Impact |
|------|---------|----------|--------|
| None detected | — | — | — |

No placeholder returns, TODO/FIXME markers in phase-modified files, or stub implementations found. All artifacts contain substantive implementation. All 148 workspace tests pass.

---

### Human Verification Required

#### 1. Table name divergence in Roadmap SC-3

**Test:** Review success criterion 3 wording: "In daemon mode, a periodic check loop detects version changes and logs them to schema_versions without requiring a restart."
**Expected:** Confirm that the implemented table `version_history` is an acceptable substitution for `schema_versions` in the roadmap text, given: (1) `schema_versions` is already the name of the migration tracker table created in `schema.rs:35`; (2) the rename is documented in `06-CONTEXT.md`, `06-RESEARCH.md`, all four plan summaries, and the SQL file header comment.
**Why human:** Roadmap text was not updated to reflect the deliberate rename. Code is internally consistent and the naming reason is sound, but the roadmap remains out of sync. Only the user can decide if the roadmap text should be updated or left as a known deviation.

#### 2. Ingestion-triggered vs. periodic detection

**Test:** Review whether "periodic check loop" in SC-3 is satisfied by the ingestion-triggered implementation.
**Expected:** Confirm that firing `check_version_change()` on every sync event (which happens continuously during active Claude Code use) satisfies the intent of "periodic check loop" and "without requiring a restart."
**Why human:** The decision is documented and locked in `06-CONTEXT.md` ("Detection is ingestion-only — no separate periodic polling loop"), but the roadmap says "periodic." Whether the ingestion-triggered approach meets the spirit of the requirement is a judgment call.

---

### Gaps Summary

No gaps. All plan must-haves are verified in code:

- Migration 006 creates `version_history` with correct schema, backfills from sessions and schema_drift_log, promotes messages columns, enhances schema_drift_log with occurrence tracking, and recreates all 7 analytical views with `is_compact_summary = 0` filtering.
- `drift.rs` uses INSERT ON CONFLICT DO UPDATE for occurrence counting; tests verify behavior.
- `decompose_user` extracts `isCompactSummary` and `sourceToolUseID` to promoted columns, serializes remaining overflow to `extra_json`; promoted keys absent from `extra_json`.
- `decompose_assistant` merges record-level and message-level overflow into single `extra_json`.
- `check_version_change()` persists to `version_history` after SSE emission, with warn-level failure logging that does not block event delivery.
- `watcher_loop()` startup backfill via INSERT OR IGNORE is idempotent and executes on every daemon start.
- `version_history_enhanced()`, `version_history_with_diff()`, and `drift_by_version()` exist and are substantive (no stubs). Promotion status computed dynamically via PRAGMA table_info.
- GET /v1/schema/versions returns `VersionHistoryEntry` by default, `VersionDiffEntry` with `?diff=true`. GET /v1/schema/drift returns `Vec<VersionDriftGroup>` with promotion status and occurrence counts.
- CLI `version-check` shows 5-column table; `schema-drift` shows grouped output. Both work in daemon and direct DB modes.
- 148 tests pass across all crates (38 core + 110 store). Server crate compiles cleanly.

Two human verification items exist but neither is a code gap — both concern roadmap terminology vs. implementation choices that were explicitly decided and documented during the planning phase.

---

_Verified: 2026-02-21_
_Verifier: Claude (gsd-verifier)_
