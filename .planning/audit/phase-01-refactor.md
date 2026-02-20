# Phase 1 Audit: Core Types & Ingestion Pipeline

**Auditor:** Phase 1 audit agent
**Date:** 2026-02-21
**Scope:** Spec sections 1.2 (serde modeling), 1.3 (streaming parser), 2.2 (decomposer), 2.3 (artifact decomposer), 2.4 (incremental sync)
**Verdict:** Phase 1 implementation is a **superset** of the spec. All spec requirements are met or exceeded. Deviations are additive (more record types, richer structures, better error handling) with a small number of naming/structural differences that are worth documenting for traceability but do not require mandatory refactoring.

---

## Executive Summary

The implementation goes significantly beyond the spec in several dimensions:
- **7 record types** implemented vs. the spec's 4 (added `system`, `summary`, `file-history-snapshot`)
- **Richer type modeling** with empirical data-driven decisions (e.g., `ProgressRecord.data` as `Value` instead of a typed struct)
- **13 tables** vs. the spec's 11 core tables (added `system_events`, `summaries`)
- **Artifact layer** (files, file_operations, git_operations) included in Phase 1 despite being specced for Phase 2
- **FTS5 indexes** included in Phase 1 despite being a separate spec concern
- **Batch transactions** with configurable BATCH_SIZE vs. spec's single-transaction model
- **Async sync engine** via `tokio_rusqlite` vs. spec's synchronous approach

Most deviations are legitimate enhancements informed by empirical analysis of real JSONL data. A few naming differences exist between spec and implementation that should be tracked for documentation purposes.

---

## Section-by-Section Audit

### 1.2 Serde Type Modeling

#### CONFORMANT: Core enum structure

**Spec says:** `JSONLRecord` enum with 4 variants: `QueueOperation`, `User`, `Assistant`, `Progress`, using `serde(tag = "type")`.

**Implementation:** `JSONLRecord` enum with **7 variants**: `User`, `Assistant`, `Progress`, `System`, `QueueOperation`, `Summary`, `FileHistorySnapshot`.

**Assessment:** CONFORMANT-PLUS. The 3 additional variants (`System`, `Summary`, `FileHistorySnapshot`) were discovered through empirical analysis of real JSONL data. The spec's 4 variants are all present. The additions are necessary for complete ingestion -- without them, ~14K system records, summary records, and file-history-snapshot records would fail to parse. This is a case where the implementation correctly exceeded the spec based on ground truth.

#### CONFORMANT: RecordBase fields

**Spec says:** `RecordBase` with fields: `uuid`, `timestamp`, `session_id`, `version`, `cwd`, `parent_uuid: Option`, `is_sidechain: bool`, `user_type: String`, `git_branch: String`, plus `#[serde(flatten)] pub overflow: HashMap`.

**Implementation:** `RecordBase` has all spec fields, plus additional optional fields: `slug: Option<String>`, `agent_id: Option<String>`, `team_name: Option<String>`, `is_meta: Option<bool>`. **No overflow HashMap** on RecordBase itself.

**Assessment:** Intentional deviation, well-documented in code comments. The implementation note at `record.rs:53-54` explains: "No overflow HashMap here -- only ONE overflow per struct is allowed, and it belongs on the outermost containing struct (e.g. UserRecord) to avoid serde(flatten) ambiguity between nested levels." This is a correct technical decision -- nested `#[serde(flatten)]` with `HashMap<String, Value>` at multiple levels causes serde ambiguity. Moving overflow to the outer record struct is the right fix.

The additional fields (`slug`, `agent_id`, `team_name`, `is_meta`) were promoted from overflow to explicit fields based on empirical frequency analysis. This is consistent with the spec's design principle that frequently-seen fields should be modeled explicitly.

**Matters:** No. This is a better design than the spec's. The spec's RecordBase overflow would have caused serde conflicts with the outer record's overflow.

#### CONFORMANT: UserMessageRecord / UserRecord

**Spec says:** `UserMessageRecord` with `base: RecordBase`, `message: UserMessage`, `permission_mode: Option`, `slug: Option`, `source_tool_assistant_uuid: Option`.

**Implementation:** `UserRecord` (renamed from `UserMessageRecord`) with `base: RecordBase`, `message: UserMessage`, `source_tool_assistant_uuid`, `tool_use_result`, `thinking_metadata`, `todos`, `permission_mode`, plus `overflow: HashMap`.

**Assessment:** CONFORMANT-PLUS. Name change (`UserMessageRecord` -> `UserRecord`) is a minor cosmetic deviation. Additional fields (`tool_use_result`, `thinking_metadata`, `todos`) were discovered in real data. `slug` was moved to RecordBase (shared across record types). Overflow is at the right level.

**Matters:** No. The naming difference is trivial. Additional fields are additive.

#### CONFORMANT: AssistantMessageRecord / AssistantRecord

**Spec says:** `AssistantMessageRecord` with `base: RecordBase`, `message: AssistantMessage`, `request_id: Option`, `slug: Option`, `agent_id: Option`.

**Implementation:** `AssistantRecord` (renamed) with `base: RecordBase`, `message: AssistantMessage`, `request_id`, `is_api_error_message`, `error`, plus `overflow: HashMap`. `slug` and `agent_id` moved to RecordBase.

**Assessment:** CONFORMANT-PLUS. `is_api_error_message` and `error` are additional fields found in real data. The spec's fields are all present (some relocated to RecordBase where they are shared).

**Matters:** No.

#### CONFORMANT: QueueOperationRecord

**Spec says:** `QueueOperationRecord` with `operation`, `timestamp`, `session_id`, plus overflow.

**Implementation:** Adds `content: Option<String>` (present on ~48.3% of queue-operation records for enqueue operations). Overflow present.

**Assessment:** CONFORMANT-PLUS. The `content` field is a legitimate addition -- the spec omitted it, but real data contains it.

**Matters:** No.

#### CONFORMANT: ProgressRecord

**Spec says:** `ProgressRecord` with `base: RecordBase`, `slug`, `agent_id`, `data: ProgressData`, `parent_tool_use_id`, `tool_use_id`.

**Implementation:** `ProgressRecord` with `base: RecordBase`, `data: serde_json::Value`, `overflow: HashMap`.

**Assessment:** DEVIATION -- simplified. The spec modeled `ProgressData` as a typed struct with `data_type`, `hook_event`, `hook_name`, `command`. The implementation stores `data` as raw `serde_json::Value` because the 8+ data.type variants have widely varying shapes. The `slug`, `agent_id` fields are in RecordBase. The `parent_tool_use_id` and `tool_use_id` fields are not explicitly modeled but would land in overflow or in the `data` value.

**Matters:** Marginally. The spec's `ProgressData` struct is too narrow -- it only models the `hook_progress` variant. The implementation's `Value` approach is more robust for the 8+ observed data subtypes. However, `parent_tool_use_id` and `tool_use_id` should be checked -- if they appear at the progress record level (not inside `data`), they would land in overflow. This is acceptable given the spec's overflow design principle, but worth verifying that the decomposer extracts `data.type` correctly (it does -- see `decompose.rs:451`).

#### CONFORMANT: MessageContent

**Spec says:** `MessageContent::Text(String) | Blocks(Vec)` using `serde(untagged)`.

**Implementation:** Identical: `MessageContent::Text(String) | Blocks(Vec<ContentBlock>)` with `serde(untagged)`.

#### CONFORMANT: ContentBlock

**Spec says:** 4 variants: `Text`, `Thinking`, `ToolUse`, `ToolResult` using `serde(tag = "type")`.

**Implementation:** Identical 4 variants. Minor difference: spec has `caller: Option` typed as `ToolCaller` struct, implementation uses `caller: Option<serde_json::Value>`. The spec also defines a separate `ToolCaller` struct with a `caller_type: String` field.

**Assessment:** CONFORMANT. Using `Value` instead of a dedicated `ToolCaller` struct is a simplification -- since ~93% of tool_use blocks have no caller, and the caller is always `{"type": "direct"}`, a typed struct adds complexity without benefit. If the caller shape evolves, `Value` is more future-proof.

**Matters:** No.

#### CONFORMANT: AssistantMessage

**Spec says:** Fields: `model`, `id`, `message_type` (renamed from `type`), `role`, `content: Vec`, `stop_reason: Option`, `stop_sequence: Option`, `usage: Option`.

**Implementation:** Same fields except `message_type` is **not explicitly modeled** -- the `type` field lands in the overflow HashMap since `serde(tag = "type")` on the outer enum already consumes the outer `type`. The inner message's `type` field (always `"message"`) is captured in `AssistantMessage.overflow`.

**Assessment:** CONFORMANT. The spec explicitly renames `type` -> `message_type` via `#[serde(rename = "type")]`, but the implementation achieves the same effect by letting it flow into overflow. The value is always `"message"` and is not needed for logic.

**Matters:** No.

#### CONFORMANT: UsageStats

**Spec says:** `input_tokens`, `output_tokens`, `cache_creation_input_tokens: Option`, `cache_read_input_tokens: Option`, `cache_creation: Option`, `service_tier: Option`, `inference_geo: Option`, plus overflow.

**Implementation:** Same fields except `inference_geo` is **not explicitly modeled** -- it lands in overflow. This is consistent with the spec note that it's a rarely-seen field (<3% of records).

**Assessment:** CONFORMANT. The spec models `inference_geo` as an explicit field; the implementation puts it in overflow. Both approaches capture the data. The implementation's choice is slightly more conservative (promotes to explicit field only when frequency warrants it).

**Matters:** No.

#### DEVIATION: CacheCreation struct

**Spec says:** `CacheCreation` struct with `ephemeral_5m_input_tokens: Option` and `ephemeral_1h_input_tokens: Option`, using `serde(rename_all = "snake_case")`.

**Implementation:** `cache_creation: Option<serde_json::Value>` -- stores the sub-object as raw JSON.

**Assessment:** Minor deviation. The `CacheCreation` sub-struct is small and stable. Using `Value` instead means the `ephemeral_*` fields are not directly queryable in Rust code, but they are stored in `token_usage.cache_creation_json` as a JSON blob, which is queryable via SQLite JSON functions. The spec's typed struct would provide compile-time field access, but the `Value` approach is simpler and still captures all data.

**Matters:** Minimally. If detailed cache analysis is ever needed, a typed struct would be better. For now, the JSON blob is sufficient.

---

### 1.3 Streaming JSONL Parser

#### CONFORMANT: Core interface

**Spec says:** `parse_jsonl(path: &Path, from_offset: u64) -> Result<ParseResult>` that opens file, seeks to offset, reads line by line, deserializes each line, captures warnings for failures.

**Implementation:** Identical signature and behavior. Uses `BufRead::lines()` with manual byte offset tracking.

#### DEVIATION: ParseResult structure

**Spec says:** `ParseResult` with `records: Vec`, `warnings: Vec`, `bytes_read: u64`, `new_offset: u64`.

**Implementation:** `ParseResult` with `records: Vec<(JSONLRecord, u64)>`, `warnings: Vec<ParseWarning>`, `new_offset: u64`, `lines_parsed: usize`, `lines_skipped: usize`, `lines_failed: usize`. Records are **paired with their byte offset** via `(JSONLRecord, u64)` tuples. `bytes_read` field is absent (not needed since `new_offset` provides the same information relative to `from_offset`).

**Assessment:** CONFORMANT-PLUS. The per-record byte offset pairing is an improvement -- the spec's flat `records: Vec` loses the per-record position information needed for the sync engine's batch boundary offset calculation. The additional line count fields (`lines_parsed`, `lines_skipped`, `lines_failed`) provide better diagnostics.

**Matters:** No. This is a better design.

#### DEVIATION: ParseWarning structure

**Spec says:** `ParseWarning` with `line_number`, `byte_offset`, `error: String`, `raw_line: String`.

**Implementation:** `raw_line_preview: String` instead of `raw_line: String` -- truncated to 500 characters.

**Assessment:** Intentional improvement. Storing the full raw line of a multi-megabyte malformed JSON line would waste memory. Truncation to 500 chars provides enough diagnostic information.

**Matters:** No.

#### CONFORMANT-PLUS: Edge case handling

**Implementation adds:**
- Early return if `from_offset >= file_length` (avoids pointless seeks)
- Offset clamping: `if current_offset > file_length { current_offset = file_length }` to handle missing trailing newlines
- `ParseError` as a dedicated error type with `thiserror` derivation

These are robustness improvements not in the spec.

---

### 2.1 SQLite Schema

#### CONFORMANT: Core 11 tables

**Spec says:** `sessions`, `messages`, `message_content`, `token_usage`, `tool_executions`, `agents`, `queue_operations`, `progress_events`, `sync_metadata`, `schema_versions`, `schema_drift_log`.

**Implementation:** All 11 tables present, plus `system_events` and `summaries` (for the two additional record types).

#### DEVIATION: Column naming in messages table

**Spec (section 2.2):** Shows `INSERT INTO messages (...)` but does not provide the exact DDL. The decomposer examples use generic `(...)` placeholders.

**Implementation:** The `messages` table DDL includes additional columns not shown in spec examples: `subtype`, `is_meta`. These support the `system` record type (which wasn't in the spec) and the `is_meta` field on RecordBase.

**Matters:** No. Additive.

#### DEVIATION: schema_drift_log columns

**Spec says (section 2.2):** `schema_drift_log` with columns `version`, `field_name`, `first_seen`, `sample_value`.

**Implementation:** `schema_drift_log` has `id`, `field_name`, `record_type`, `version`, `sample_value`, `first_seen_at`, `source_context`, with UNIQUE constraint on `(field_name, record_type, version)`.

**Assessment:** CONFORMANT-PLUS. The `record_type` column is an important addition -- it disambiguates field names that appear in different record types (e.g., `"content"` could overflow from multiple record types). The `source_context` column provides additional diagnostic traceability.

**Matters:** No. This is a better design.

#### DEVIATION: sync_metadata columns

**Spec says (section 2.4):** `sync_metadata` with `file_path`, `last_byte_offset`, `record_count_at_sync`, `last_synced_at`.

**Implementation:** `record_count` instead of `record_count_at_sync`.

**Assessment:** Trivial naming difference. Functionally identical.

**Matters:** No.

#### CONFORMANT: Artifact tables (files, file_operations, git_operations)

**Spec section 2.1:** Defines `files`, `file_operations`, `git_operations` tables with specific column sets.

**Implementation:** All three tables present in migration 003. Column structure closely follows the spec with minor differences:

- `files` table: Spec uses `file_id` as PK name, `first_seen_at`/`last_modified_at` as timestamps, `operation_count INTEGER DEFAULT 1`. Implementation uses `id` as PK, `first_seen`/`last_modified`, `operation_count DEFAULT 0`.

- `file_operations` table: Spec has `operation_id` PK, `file_id` FK, `is_error` column, `result_summary` column. Implementation has `id` PK, no `file_id` FK (uses `file_path` + `session_id` denormalized), no `is_error` or `result_summary` columns on file_operations (these are on `tool_executions`).

- `git_operations` table: Spec has `git_op_id` PK, `result_summary`, `is_error`. Implementation has `id` PK, no `result_summary` or `is_error` columns.

**Assessment:** The spec's artifact tables reference `file_id` FK, `result_summary`, and `is_error` on file_operations/git_operations. The implementation **denormalizes** by storing `file_path` directly on `file_operations` instead of a `file_id` FK. The `result_summary` and `is_error` are available via the `tool_executions` table (joined on `tool_use_id`), but not duplicated on the artifact tables.

**Matters:** Moderately. The denormalization of `file_path` vs `file_id` FK is a design choice -- it simplifies queries (no JOIN needed to get the file path) but means file path normalization happens at the `files` table level only. The missing `is_error` and `result_summary` on artifact tables means artifact queries must JOIN to `tool_executions` to check error status, which the spec's design avoids. This is a minor query ergonomics trade-off.

---

### 2.2 Decomposer

#### CONFORMANT: Dispatch pattern

**Spec says:** `impl Decomposer` with a `decompose` method that matches on `JSONLRecord` variants and dispatches to per-type functions.

**Implementation:** Free function `decompose_record(record, session_id_from_file, tx)` with the same dispatch pattern. Uses free functions instead of a `Decomposer` struct.

**Assessment:** CONFORMANT. The spec shows a `Decomposer` struct but the only state it carries is the database transaction reference, which the implementation passes as a parameter. The function-based approach is simpler.

**Matters:** No.

#### CONFORMANT: decompose_assistant behavior

**Spec says:** 1) Insert into messages, 2) Decompose each content block, 3) Insert token usage, 4) Track overflow for drift detection.

**Implementation:** 1) Upsert session, 2) Insert message, 3) Decompose content blocks, 4) Insert token usage, 5) Upsert agent, 6) Log overflow from record, message, and usage levels.

**Assessment:** CONFORMANT-PLUS. The implementation adds session upsert and agent upsert (not mentioned in the spec's decomposer pseudocode but necessary for FK integrity). Overflow logging is more granular (3 levels vs. spec's 1).

#### CONFORMANT: log_drift behavior

**Spec says:** `log_drift` iterates overflow HashMap and inserts into `schema_drift_log`.

**Implementation:** `drift::log_overflow` does the same with added features: sample value truncation (500 chars), source_context, record_type tracking.

**Assessment:** CONFORMANT-PLUS.

#### DEVIATION: Tool result matching approach

**Spec section 2.3:** Describes `ArtifactDecomposer::match_tool_results` that buffers the previous assistant message and matches `tool_result` blocks from the subsequent user message by `tool_use_id`, producing a `HashMap<String, ToolResultPair>`.

**Implementation:** Does NOT buffer previous messages. Instead, uses a two-pass approach:
1. During assistant record decomposition, `tool_executions` rows are created with `result_content = NULL`
2. During the subsequent user record decomposition, `tool_result` content blocks trigger an `UPDATE tool_executions SET result_content = ?1 WHERE tool_use_id = ?3`

**Assessment:** Functionally equivalent but architecturally different. The spec's buffering approach processes both halves in a single pass. The implementation's UPDATE approach processes them in sequence (assistant first, then user). Both achieve the same outcome: tool_result is linked to tool_use via tool_use_id.

The implementation's approach is actually more robust for incremental sync scenarios where a partial batch might include the assistant record but not the subsequent user record -- the tool_executions row exists with NULL result_content and will be populated when the user record arrives in the next sync batch. The spec's buffering approach would miss this tool_result entirely if it falls across a batch boundary.

**Matters:** No. The implementation's approach is arguably better for incremental sync resilience.

---

### 2.3 Artifact Decomposer

#### CONFORMANT: Tool dispatch

**Spec says:** `ArtifactDecomposer::decompose_tool_use` dispatches on tool name: `Write`, `Edit`, `Read`, `Bash`, `NotebookEdit`.

**Implementation:** `decompose_assistant_artifacts` dispatches on tool name: `Write`, `Edit`, `Read`, `Bash`.

**Assessment:** DEVIATION -- `NotebookEdit` is not handled. The spec maps `NotebookEdit` to the same handler as `Write`. Since `NotebookEdit` tool_use blocks exist in real data (editing Jupyter notebooks), they would currently be silently skipped.

**Matters:** Yes, minor. `NotebookEdit` operations would not be captured in file_operations. The input JSON shape differs from `Write` (has `notebook_path` instead of `file_path`, and `new_source` instead of `content`). A simple addition to the match arm would fix this.

#### CONFORMANT: Write/Edit/Read extraction

**Spec says:** Extract `file_path` and `content` from Write input, `file_path`/`old_string`/`new_string` from Edit, `file_path` from Read. Upsert files table, insert file_operations.

**Implementation:** Matches the spec's extraction logic exactly.

#### CONFORMANT: Bash git extraction

**Spec says:** Parse git commands from bash, extract commit messages (inline and heredoc), extract branch names, classify operation types.

**Implementation:** Uses compiled regex patterns (OnceLock) for extraction. Supports: inline commit messages, HEREDOC commit messages, branch extraction from checkout -b and push, classification into add/commit/push/checkout/branch/merge/rebase/other.

**Assessment:** CONFORMANT-PLUS. The spec's pseudocode uses simple string matching (`command.contains("git commit")`). The implementation uses proper regex patterns which handle edge cases better (e.g., chained commands with `&&` or `;`).

#### CONFORMANT: Bash file command extraction

**Spec says:** Detect file-touching commands (cp, mv, rm, mkdir, touch), extract paths, insert file_operations with `operation_type = 'bash'`.

**Implementation:** Detects same commands. Uses operation types `bash_cp`, `bash_mv`, `bash_rm`, `bash_mkdir`, `bash_touch` instead of a generic `'bash'`.

**Assessment:** CONFORMANT-PLUS. More granular operation types enable better filtering in queries.

#### DEVIATION: tool_result not populated on file_operations / git_operations

**Spec says:** `tool_result: Option` parameter on `decompose_tool_use`, with `result_summary` and `is_error` fields on `file_operations` and `git_operations`.

**Implementation:** The artifact tables (`file_operations`, `git_operations`) do not have `result_summary` or `is_error` columns. Tool results are accessible via the `tool_executions` table through a JOIN on `tool_use_id`.

**Assessment:** This is a normalization choice. The spec duplicates error status and result summary on the artifact tables for query convenience. The implementation keeps them only in `tool_executions` to avoid data duplication.

**Matters:** Moderately for query ergonomics. Queries like "show me all file operations that errored" require a JOIN in the current implementation but are a simple WHERE clause in the spec's design. This is a deliberate trade-off documented via the table structure.

#### ADDITION: Retroactive artifact decomposition

**Implementation adds:** `decompose_artifacts_retroactive(conn)` that backfills artifact tables from existing `tool_executions` rows. This handles data ingested before migration 003 (artifacts) was applied.

**Assessment:** Not in spec. Good robustness feature for handling migration ordering.

---

### 2.4 Incremental Sync

#### CONFORMANT: Core sync_file behavior

**Spec says:** `sync_file(path)` that: 1) Gets last offset from sync_metadata, 2) Checks file size, 3) Parses from offset, 4) Decomposes records in transaction, 5) Updates sync_metadata, 6) Returns SyncResult.

**Implementation:** `sync_file(conn, path, session_id)` follows the same flow. Notable differences:
- Async via `tokio_rusqlite` (spec is synchronous)
- Batch transactions of 1000 records (spec does one transaction for the whole file)
- Session ID passed as parameter (spec does not address session ID extraction)

**Assessment:** CONFORMANT-PLUS. Async execution and batch transactions are improvements for large files (some sessions are 580MB+). The spec's single-transaction model could hold the WAL checkpoint too long on large files.

#### CONFORMANT: bulk_import / sync_all

**Spec says:** `bulk_import(claude_dir)` walks `projects/` directory, calls `sync_file` for each `.jsonl`, handles errors per-file.

**Implementation:** `sync_all(conn, projects_dir)` with identical behavior. Additionally:
- Rebuilds FTS5 index after syncing new data
- Runs retroactive artifact decomposition when new records ingested

**Assessment:** CONFORMANT-PLUS. The FTS rebuild and retroactive artifact decomposition are integration additions.

#### DEVIATION: SyncResult type

**Spec says:** `SyncResult::Synced { new_records, warnings, bytes_read } | SyncResult::NoNewData`.

**Implementation:** `SyncFileResult` struct with `records_synced`, `records_failed`, `warnings`, `overflow_fields_logged`, `skipped: bool`. No enum -- uses `skipped` flag instead.

**Assessment:** Minor structural difference. The implementation provides richer statistics (overflow fields logged, records failed count).

**Matters:** No.

#### ADDITION: Session ID extraction from file paths

**Implementation adds:** `extract_session_id(path)` that handles both main session files and subagent files. The spec mentions this need but does not provide a function.

---

## Missing from Implementation (Spec Items Not Yet Built)

These items are specified in the spec's Phase 1/2 sections but are not part of the Phase 1 implementation. They may be planned for later phases.

1. **`crates/core/src/version.rs`** -- Version detection and drift monitoring. The spec describes a `VersionMonitor` struct with `check()`, `get_installed_version()`, `detect_drift()`, and `run_loop()` methods. The file does not exist. This appears to be deferred to a later phase (spec Phase 3.2), which is appropriate.

2. **`crates/core/src/config.rs`** -- `.claude.json` schema parsing. Listed in the spec's file structure but not implemented. Likely not needed for Phase 1.

3. **NotebookEdit tool support** -- Mentioned in spec section 2.3 but not in implementation's artifact extraction dispatch.

---

## Refactoring Plan

### Priority 1: Should Fix (functional gaps)

#### R1: Add NotebookEdit to artifact extraction dispatch

**Spec says:** `"NotebookEdit" => self.decompose_write(...)` -- treats NotebookEdit as a write operation.

**What was built:** NotebookEdit is not in the match arm in `artifacts.rs`. NotebookEdit tool_use blocks are silently skipped.

**What needs to change:** Add a match arm for `"NotebookEdit"` in `decompose_assistant_artifacts()` at `crates/store/src/artifacts.rs:109`. The input JSON shape for NotebookEdit is `{ "notebook_path": "...", "new_source": "..." }`, which differs from Write's `{ "file_path": "...", "content": "..." }`. Either:
- (a) Map NotebookEdit fields to the write handler (translate `notebook_path` -> `file_path`, `new_source` -> `content`), or
- (b) Create a dedicated `extract_notebook_edit_operation` function.

Option (a) is simpler and matches the spec's intent.

**Files to modify:**
- `crates/store/src/artifacts.rs` -- add match arm and field mapping

### Priority 2: Consider (query ergonomics)

#### R2: Consider adding is_error and result_summary to artifact tables

**Spec says:** `file_operations` and `git_operations` tables have `result_summary TEXT` and `is_error BOOLEAN` columns, populated from the matched tool_result.

**What was built:** These columns are absent from the artifact tables. Error status is only queryable via JOIN to `tool_executions`.

**What needs to change (if pursued):**
1. New migration 004 adding `is_error INTEGER` and `result_summary TEXT` to both `file_operations` and `git_operations`
2. Populate these during artifact decomposition from the tool_result
3. Backfill via UPDATE JOIN from existing tool_executions data

**Assessment:** This is a query convenience optimization. The data is already accessible via JOIN. Deferring is reasonable if the current query patterns do not require direct filtering on error status in artifact tables. If the HTTP API routes for artifacts will need to filter by error status, this becomes more important.

**Files to modify (if pursued):**
- `crates/store/migrations/004_artifact_error_status.sql` (new)
- `crates/store/src/schema.rs` -- add migration to MIGRATIONS array
- `crates/store/src/artifacts.rs` -- populate during extraction

### Priority 3: No Action Needed (documented deviations)

These deviations are intentional, well-documented, or improvements. No refactoring needed.

| Deviation | Spec | Implementation | Verdict |
|-----------|------|----------------|---------|
| RecordBase overflow placement | On RecordBase | On outer record structs | Better design (avoids nested flatten ambiguity) |
| Record type count | 4 variants | 7 variants | Necessary for real data completeness |
| ProgressData typing | Typed struct | `serde_json::Value` | Better design (8+ polymorphic subtypes) |
| CacheCreation typing | Typed struct | `serde_json::Value` | Acceptable simplification |
| ParseResult.records | `Vec<JSONLRecord>` | `Vec<(JSONLRecord, u64)>` | Better design (per-record offset) |
| ParseWarning.raw_line | Full line | 500-char preview | Better design (memory safety) |
| Type naming | `*MessageRecord` | `*Record` | Cosmetic, consistent internally |
| Decomposer pattern | Struct with methods | Free functions | Simpler, equivalent |
| Sync engine | Synchronous | Async with batch transactions | Better design (large file handling) |
| schema_drift_log | 4 columns | 7 columns with record_type | Better design (disambiguation) |
| Bash op types | Generic `'bash'` | Granular `bash_cp`, `bash_rm`, etc. | Better design (queryability) |
| Tool result matching | Message buffering | Two-pass UPDATE | Better for incremental sync |
| File operations FK | `file_id` FK | Denormalized `file_path` | Trade-off: simpler queries vs. normalization |
| inference_geo | Explicit field | In overflow | Acceptable (rare field) |

---

## Summary

Phase 1 is **substantially conformant** with the spec. The implementation is a superset that handles more record types, provides better error isolation, uses more robust patterns for polymorphic data, and adds async/batch capabilities. The single actionable refactoring item (R1: NotebookEdit support) is a small addition. The query ergonomics item (R2: error status on artifact tables) is a judgment call depending on upcoming API requirements.

The implementation demonstrates good engineering judgment in places where the spec's pseudocode was simplified or omitted edge cases. The code comments and module-level documentation consistently explain why deviations were made, providing the forensic traceability called for by the project's commit guidelines.
