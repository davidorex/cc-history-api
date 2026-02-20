# Phase 5 (Artifact Layer) Audit: Spec vs Implementation

Audit date: 2026-02-21
Spec source: `/cc-history-api.md` sections 2.1, 2.3, 4.2, 4.3, 4.4
Implementation: `crates/store/migrations/003_artifacts.sql`, `crates/store/src/artifacts.rs`,
`crates/store/src/artifact_queries.rs`, `crates/store/src/fts.rs`,
`crates/server/src/api/files.rs`, `crates/server/src/api/git.rs`,
`crates/server/src/api/artifacts_api.rs`

---

## 1. Schema DDL Comparison

### 1.1 `files` table

| Column | Spec (section 2.1) | Implementation (003_artifacts.sql) | Match? |
|--------|--------------------|------------------------------------|--------|
| PK | `file_id INTEGER PRIMARY KEY AUTOINCREMENT` | `id INTEGER PRIMARY KEY AUTOINCREMENT` | **NO** — column renamed from `file_id` to `id` |
| session_id | `TEXT NOT NULL` FK to sessions | `TEXT NOT NULL REFERENCES sessions(session_id)` | YES |
| file_path | `TEXT NOT NULL` | `TEXT NOT NULL` | YES |
| first_seen_at | `DATETIME NOT NULL` | `first_seen TEXT NOT NULL` | **NO** — column renamed from `first_seen_at` to `first_seen`, type changed from DATETIME to TEXT |
| last_modified_at | `DATETIME NOT NULL` | `last_modified TEXT NOT NULL` | **NO** — column renamed from `last_modified_at` to `last_modified`, type changed from DATETIME to TEXT |
| operation_count | `INTEGER DEFAULT 1` | `INTEGER NOT NULL DEFAULT 0` | **NO** — default changed from 1 to 0, NOT NULL added |
| UNIQUE | `(session_id, file_path)` | `(session_id, file_path)` | YES |

**Severity: moderate** — Column name deviations (`file_id` vs `id`, `first_seen_at` vs `first_seen`, `last_modified_at` vs `last_modified`) propagate through all query functions and API response shapes. The TEXT vs DATETIME distinction is cosmetic in SQLite but the column name changes require API surface adjustment.

### 1.2 `file_operations` table

| Column | Spec (section 2.1) | Implementation (003_artifacts.sql) | Match? |
|--------|--------------------|------------------------------------|--------|
| PK | `operation_id INTEGER PRIMARY KEY AUTOINCREMENT` | `id INTEGER PRIMARY KEY AUTOINCREMENT` | **NO** — column renamed from `operation_id` to `id` |
| file_id | `INTEGER NOT NULL` FK to files | **ABSENT** — file_path stored directly | **NO** — spec uses file_id FK, impl uses denormalized file_path |
| session_id | `TEXT NOT NULL` | `TEXT NOT NULL` | YES |
| message_uuid | `TEXT NOT NULL` | `TEXT REFERENCES messages(uuid)` — nullable | **NO** — spec says NOT NULL, impl allows NULL |
| tool_use_id | `TEXT NOT NULL` | `TEXT` — nullable | **NO** — spec says NOT NULL, impl allows NULL |
| operation_type | `TEXT NOT NULL` | `TEXT NOT NULL` | YES (but impl adds bash_cp, bash_mv, bash_rm, bash_mkdir, bash_touch subtypes where spec uses just 'bash') |
| timestamp | `DATETIME NOT NULL` | `TEXT NOT NULL` | YES (TEXT vs DATETIME is cosmetic in SQLite) |
| content | `TEXT` | `TEXT` | YES |
| old_content | `TEXT` | `TEXT` | YES |
| command | `TEXT` | `TEXT` | YES |
| result_summary | `TEXT` | **ABSENT** | **NO** — spec has result_summary, impl does not |
| is_error | `BOOLEAN DEFAULT FALSE` | **ABSENT** | **NO** — spec has is_error, impl does not |
| UNIQUE | none explicit | `UNIQUE(tool_use_id)` | **DEVIATION** — impl adds UNIQUE on tool_use_id not in spec |
| FK to files | `FOREIGN KEY (file_id) REFERENCES files(file_id)` | **ABSENT** — no file_id FK | **NO** — denormalized, no FK |

**Severity: critical** — Two spec columns (`result_summary`, `is_error`) are completely missing from the implementation. The denormalized file_path (instead of file_id FK) is a significant structural deviation. The nullable message_uuid and tool_use_id contradict the spec's NOT NULL constraints.

### 1.3 `git_operations` table

| Column | Spec (section 2.1) | Implementation (003_artifacts.sql) | Match? |
|--------|--------------------|------------------------------------|--------|
| PK | `git_op_id INTEGER PRIMARY KEY AUTOINCREMENT` | `id INTEGER PRIMARY KEY AUTOINCREMENT` | **NO** — column renamed from `git_op_id` to `id` |
| session_id | `TEXT NOT NULL` | `TEXT NOT NULL REFERENCES sessions(session_id)` | YES |
| message_uuid | `TEXT NOT NULL` | `TEXT REFERENCES messages(uuid)` — nullable | **NO** — spec says NOT NULL, impl allows NULL |
| tool_use_id | `TEXT NOT NULL` | `TEXT` — nullable | **NO** — spec says NOT NULL, impl allows NULL |
| operation_type | `TEXT NOT NULL` | `TEXT NOT NULL` | YES |
| command | `TEXT NOT NULL` | `TEXT NOT NULL` | YES |
| commit_message | `TEXT` | `TEXT` | YES |
| branch | `TEXT` | `TEXT` | YES |
| timestamp | `DATETIME NOT NULL` | `TEXT NOT NULL` | YES |
| result_summary | `TEXT` | **ABSENT** | **NO** — spec has result_summary, impl does not |
| is_error | `BOOLEAN DEFAULT FALSE` | **ABSENT** | **NO** — spec has is_error, impl does not |
| UNIQUE | none explicit | `UNIQUE(tool_use_id, operation_type)` | **DEVIATION** — impl adds composite unique not in spec |

**Severity: critical** — Same missing columns as file_operations: `result_summary` and `is_error` are absent. Same nullable deviations for message_uuid and tool_use_id.

### 1.4 FTS5 virtual table

| Aspect | Spec (section 2.1) | Implementation (003_artifacts.sql) | Match? |
|--------|--------------------|------------------------------------|--------|
| Table name | `file_content_fts` | `fts_file_operations` | **NO** — renamed |
| Indexed columns | `content, old_content, command` | `content, old_content, command` | YES |
| Content table | `content='file_operations', content_rowid='operation_id'` | `content='file_operations', content_rowid='id'` | **NO** — rowid references `id` instead of `operation_id` (consistent with PK rename) |

**Severity: minor** — Table name difference is internal. The rowid change is consistent with the PK rename.

### 1.5 Indexes

| Spec Index | Implementation Index | Match? |
|-----------|---------------------|--------|
| `idx_files_session ON files(session_id)` | `idx_files_session_id ON files(session_id)` | naming deviation only |
| `idx_files_path ON files(file_path)` | `idx_files_file_path ON files(file_path)` | naming deviation only |
| `idx_fileops_file ON file_operations(file_id)` | **ABSENT** — no file_id column | **NO** — cannot exist without file_id |
| `idx_fileops_session ON file_operations(session_id)` | `idx_file_operations_session_id` | naming deviation only |
| `idx_fileops_type ON file_operations(operation_type)` | **ABSENT** | **NO** — missing index |
| `idx_fileops_timestamp ON file_operations(timestamp)` | `idx_file_operations_timestamp` | naming deviation only |
| `idx_fileops_path ON file_operations(file_id, timestamp)` | **ABSENT** — no file_id | **NO** — cannot exist without file_id; no compound file_path+timestamp index |
| `idx_gitops_session ON git_operations(session_id)` | `idx_git_operations_session_id` | naming deviation only |
| `idx_gitops_type ON git_operations(operation_type)` | `idx_git_operations_operation_type` | naming deviation only |
| `idx_gitops_timestamp ON git_operations(timestamp)` | `idx_git_operations_timestamp` | naming deviation only |

**Severity: minor** — Index naming deviations are cosmetic. Missing `operation_type` index on file_operations and the compound index are functional gaps.

---

## 2. Artifact Decomposer Comparison

### 2.1 Tool result matching (spec section 2.3 `match_tool_results`)

**Spec says:** `match_tool_results` function takes an `AssistantMessageRecord` and the next `UserMessageRecord`, links `tool_use` blocks to their `tool_result` blocks by `tool_use_id`, and populates `result_summary` + `is_error` fields on the operation rows.

**Implementation does:** `crates/store/src/artifacts.rs` has a comment at line 87: "Tool_result matching (ART-04) is handled separately." However, there is no `match_tool_results` function anywhere in the codebase. The `decompose_artifacts_retroactive` function (line 520) queries `tool_executions` for result_content and is_error but stores them only as `_result_content` and `_is_error` (prefixed with underscore = unused).

**Severity: critical** — Tool result matching is not implemented. The spec's core feature of linking tool_use to tool_result outcomes and storing `result_summary`/`is_error` on file_operations and git_operations rows is completely absent.

### 2.2 NotebookEdit handling

**Spec says (section 2.3, line 542):** `"NotebookEdit" => self.decompose_write(...)` — NotebookEdit should be treated as a write operation.

**Implementation does:** The match block in `decompose_assistant_artifacts` (artifacts.rs:109-150) handles Write, Edit, Read, Bash only. NotebookEdit is not handled.

**Severity: moderate** — NotebookEdit operations would be silently dropped during artifact extraction.

### 2.3 Bash file operations operation_type

**Spec says (section 2.3):** Bash file-touching commands produce `file_operation` rows with `operation_type = 'bash'`.

**Implementation does:** Bash file commands produce `operation_type = 'bash_cp'`, `'bash_mv'`, `'bash_rm'`, `'bash_mkdir'`, `'bash_touch'` (artifacts.rs:410 `format!("bash_{}", cmd_name)`).

**Severity: minor** — The granular bash subtypes are arguably more useful than the spec's flat 'bash' type. This is an improvement over spec, not a regression.

---

## 3. API Response Type Comparison

### 3.1 `FileEntry` (spec section 4.2)

| Field | Spec | Implementation (artifact_queries.rs:28-35) | Match? |
|-------|------|---------------------------------------------|--------|
| file_id | `i64` | `id: i64` | **NO** — field renamed |
| session_id | `String` | `session_id: String` | YES |
| file_path | `String` | `file_path: String` | YES |
| first_seen_at | `String` | `first_seen: String` | **NO** — field renamed |
| last_modified_at | `String` | `last_modified: String` | **NO** — field renamed |
| operation_count | `i64` | `operation_count: i64` | YES |
| operations | `Vec<FileOperation>` (included for single-file fetch) | **ABSENT** — separate FileDetailResponse wraps file + operations | **NO** — structural deviation; spec nests operations inside FileEntry |

**Severity: moderate** — Three field names differ from spec. The operations nesting is handled differently (separate wrapper struct in files.rs:74-78).

### 3.2 `FileOperation` (spec section 4.2)

| Field | Spec | Implementation (artifact_queries.rs:40-51) | Match? |
|-------|------|---------------------------------------------|--------|
| operation_id | `i64` | `id: i64` | **NO** — field renamed |
| operation_type | `String` | `operation_type: String` | YES |
| timestamp | `String` | `timestamp: String` | YES |
| message_uuid | `String` | `message_uuid: Option<String>` | **NO** — spec is non-optional, impl is Option |
| tool_use_id | `String` | `tool_use_id: Option<String>` | **NO** — spec is non-optional, impl is Option |
| content | `Option<String>` | `content: Option<String>` | YES |
| old_content | `Option<String>` | `old_content: Option<String>` | YES |
| command | `Option<String>` | `command: Option<String>` | YES |
| result_summary | `Option<String>` | **ABSENT** | **NO** — missing field |
| is_error | `bool` | **ABSENT** | **NO** — missing field |

**Severity: critical** — `result_summary` and `is_error` are missing from the response type, consistent with the schema gap. The Optional wrapping of message_uuid and tool_use_id is inconsistent with spec.

### 3.3 `GitOperation` (spec section 4.2)

| Field | Spec | Implementation (artifact_queries.rs:54-65) | Match? |
|-------|------|---------------------------------------------|--------|
| operation_type | `String` | `operation_type: String` | YES |
| command | `String` | `command: String` | YES |
| commit_message | `Option<String>` | `commit_message: Option<String>` | YES |
| branch | `Option<String>` | `branch: Option<String>` | YES |
| timestamp | `String` | `timestamp: String` | YES |
| session_id | `String` | `session_id: String` | YES |
| result_summary | `Option<String>` | **ABSENT** | **NO** — missing field |
| is_error | `bool` | **ABSENT** | **NO** — missing field |
| (not in spec) | — | `id: i64` | **EXTRA** — impl adds id |
| (not in spec) | — | `tool_use_id: Option<String>` | **EXTRA** — impl adds tool_use_id |
| (not in spec) | — | `message_uuid: Option<String>` | **EXTRA** — impl adds message_uuid |

**Severity: critical** — `result_summary` and `is_error` missing. Extra fields (id, tool_use_id, message_uuid) are additive and not harmful.

### 3.4 `SessionArtifacts` (spec section 4.2)

| Field | Spec | Implementation (artifact_queries.rs:100-105) | Match? |
|-------|------|-----------------------------------------------|--------|
| session_id | `String` | **ABSENT** | **NO** — missing |
| files_touched | `Vec<FileEntry>` | `files: Vec<FileEntry>` | **NO** — field renamed from `files_touched` to `files` |
| git_operations | `Vec<GitOperation>` | `git_operations: Vec<GitOperation>` | YES |
| total_writes | `i64` | **ABSENT** | **NO** — missing aggregate |
| total_edits | `i64` | **ABSENT** | **NO** — missing aggregate |
| total_reads | `i64` | **ABSENT** | **NO** — missing aggregate |
| total_git_commits | `i64` | **ABSENT** | **NO** — missing aggregate |
| (not in spec) | — | `tool_executions: Vec<ToolExecutionEntry>` | **EXTRA** — impl adds tool_executions |

**Severity: critical** — Four aggregate fields (`total_writes`, `total_edits`, `total_reads`, `total_git_commits`) are completely missing. The `session_id` field is missing. The `files_touched` field is renamed. `tool_executions` is an additive extra.

### 3.5 `ArtifactEvent` (spec section 4.2)

**Spec says:** Tagged enum with `#[serde(tag = "kind")]` using `"file"` and `"git"` variants wrapping `FileOperation` and `GitOperation`.

**Implementation does:** `TimelineEntry` struct with flat fields: `entry_type: String` (values: "file_operation", "git_operation", "tool_execution"). This is a flat struct, NOT a tagged enum.

**Severity: critical** — Completely different approach. Spec uses a discriminated union (tagged enum), implementation uses a flat struct with optional fields. The tag values differ too ("file"/"git" vs "file_operation"/"git_operation"/"tool_execution").

---

## 4. API Endpoint Comparison

### 4.1 POST /v1/files/query

**Spec (section 4.4):**
```json
{
  "session_ids": ["abc-123"],
  "file_paths": ["/src/**/*.rs"],
  "operation_types": ["write", "edit"],
  "content_contains": "async fn",
  "after": "2025-02-01T00:00:00Z",
  "include_content": true,
  "limit": 50
}
```

**Implementation (files.rs:63-71):**
```rust
pub struct FileQueryBody {
    pub pattern: Option<String>,
    pub session_id: Option<String>,
    pub limit: Option<usize>,
}
```

| Parameter | Spec | Implementation | Match? |
|-----------|------|----------------|--------|
| session_ids | `Vec<String>` (array) | `session_id: Option<String>` (single) | **NO** — spec allows multiple, impl allows one |
| file_paths | `Vec<String>` (array of globs) | `pattern: Option<String>` (single glob) | **NO** — spec allows multiple, impl allows one; renamed |
| operation_types | `Vec<String>` | **ABSENT** | **NO** — missing |
| content_contains | `String` | **ABSENT** | **NO** — missing |
| after | `String` (datetime) | **ABSENT** | **NO** — missing |
| include_content | `bool` | **ABSENT** | **NO** — missing |
| limit | `usize` | `limit: Option<usize>` | YES |

**Severity: critical** — POST /v1/files/query is severely underpowered compared to spec. It lacks operation_types filtering, content_contains search, temporal filtering (after), multi-session support, and include_content toggle.

### 4.2 GET /v1/git

**Spec:** `?session_id=&type=commit&after=&before=`

**Implementation (git.rs:28-36):** `session_id`, `operation_type`, `limit` query params. Missing: `after`, `before` temporal filters.

**Severity: moderate** — Temporal filtering missing from git endpoint.

### 4.3 GET /v1/artifacts/{session_id}

**Spec:** Returns `SessionArtifacts` with aggregate counts.
**Implementation:** Returns `SessionArtifacts` without aggregate counts (see 3.4 above).

**Severity: critical** — (Covered in section 3.4)

### 4.4 GET /v1/artifacts/{session_id}/timeline

**Spec:** Returns `[ArtifactEvent]` (tagged enum with "file"/"git" variants).
**Implementation:** Returns `[TimelineEntry]` (flat struct with "file_operation"/"git_operation"/"tool_execution" entry_type).

**Severity: critical** — (Covered in section 3.5)

---

## 5. File Content Reconstruction (spec section 4.3)

**Spec:** `reconstruct_file_at(db, file_id, at_message_uuid)` — takes `file_id` and `at_message_uuid`, queries by `file_id`.

**Implementation:** `reconstruct_file_content(conn, file_path, session_id, at_message_uuid)` — takes `file_path` + `session_id`, queries by file_path since file_operations has no file_id FK.

**Severity: moderate** — Functionally equivalent but API address space differs. The denormalized schema forces callers to provide file_path+session_id instead of file_id. The HTTP handler (files.rs:151-186) compensates by looking up file_path from the file entry.

---

## 6. Deviation Summary (Sorted by Severity)

### Critical (blocks spec compliance)

| # | Deviation | Spec | Implementation | Files to modify |
|---|-----------|------|----------------|-----------------|
| C1 | Missing `result_summary` and `is_error` on file_operations table | Columns exist in DDL | Absent from migration, absent from artifacts.rs, absent from artifact_queries.rs | 003_artifacts.sql (migration 004), artifacts.rs, artifact_queries.rs |
| C2 | Missing `result_summary` and `is_error` on git_operations table | Columns exist in DDL | Absent from migration, absent from artifacts.rs, absent from artifact_queries.rs | 003_artifacts.sql (migration 004), artifacts.rs, artifact_queries.rs |
| C3 | No `match_tool_results` implementation | Function described in spec 2.3 | Not implemented anywhere | New function needed in artifacts.rs or decompose.rs |
| C4 | SessionArtifacts missing aggregates | `total_writes`, `total_edits`, `total_reads`, `total_git_commits` | Not computed | artifact_queries.rs:546-601, artifacts_api.rs |
| C5 | SessionArtifacts missing `session_id` field | `session_id: String` | Absent | artifact_queries.rs:100-105 |
| C6 | POST /v1/files/query severely limited | 7 filter parameters | 3 filter parameters | files.rs:63-71, files.rs:255-293 |
| C7 | ArtifactEvent is flat struct, not tagged enum | `#[serde(tag = "kind")] enum` with "file"/"git" | `TimelineEntry` flat struct with string discriminant | artifact_queries.rs:69-85, artifacts_api.rs |

### Moderate (spec deviation but functionally works)

| # | Deviation | Spec | Implementation | Files to modify |
|---|-----------|------|----------------|-----------------|
| M1 | `files` PK renamed from `file_id` to `id` | `file_id` | `id` | 003_artifacts.sql, artifact_queries.rs |
| M2 | `files` column `first_seen_at` renamed to `first_seen` | `first_seen_at` | `first_seen` | 003_artifacts.sql, artifacts.rs, artifact_queries.rs |
| M3 | `files` column `last_modified_at` renamed to `last_modified` | `last_modified_at` | `last_modified` | 003_artifacts.sql, artifacts.rs, artifact_queries.rs |
| M4 | `file_operations` denormalized (file_path instead of file_id FK) | `file_id INTEGER NOT NULL` FK | `file_path TEXT NOT NULL` | 003_artifacts.sql, artifacts.rs, artifact_queries.rs |
| M5 | FileEntry field names differ (`id` vs `file_id`, `first_seen` vs `first_seen_at`, `last_modified` vs `last_modified_at`) | Spec names | Implementation names | artifact_queries.rs:28-35 |
| M6 | `files_touched` renamed to `files` in SessionArtifacts | `files_touched` | `files` | artifact_queries.rs:102 |
| M7 | NotebookEdit not handled in artifact decomposer | Should be treated as Write | Not matched | artifacts.rs:109-150 |
| M8 | message_uuid/tool_use_id nullable in impl, NOT NULL in spec | NOT NULL | nullable | 003_artifacts.sql |
| M9 | Missing `after`/`before` temporal filters on GET /v1/git | Spec has time filters | Not implemented | git.rs |

### Minor (cosmetic / arguably improvements)

| # | Deviation | Spec | Implementation | Files to modify |
|---|-----------|------|----------------|-----------------|
| m1 | FTS table renamed from `file_content_fts` to `fts_file_operations` | `file_content_fts` | `fts_file_operations` | 003_artifacts.sql (naming only) |
| m2 | Index naming convention differs | `idx_fileops_*` | `idx_file_operations_*` | 003_artifacts.sql |
| m3 | Bash operation_type uses subtypes (bash_cp, bash_mv, etc.) instead of flat 'bash' | `'bash'` | `'bash_cp'`, `'bash_mv'`, etc. | artifacts.rs:410 |
| m4 | operation_count default 0 instead of 1 | DEFAULT 1 | DEFAULT 0 | 003_artifacts.sql |
| m5 | Missing `idx_fileops_type` index on operation_type | Index specified | Not created | 003_artifacts.sql |
| m6 | Extra fields on GitOperation (id, tool_use_id, message_uuid) not in spec | Not in spec | Additive | artifact_queries.rs:54-65 |
| m7 | Extra `tool_executions` field on SessionArtifacts not in spec | Not in spec | Additive | artifact_queries.rs:104 |

---

## 7. Refactoring Plan

### Phase 5a: Schema migration (migration 004)

**New migration file:** `crates/store/migrations/004_artifact_columns.sql`

1. Add `result_summary TEXT` to `file_operations` (C1)
2. Add `is_error INTEGER DEFAULT 0` to `file_operations` (C1)
3. Add `result_summary TEXT` to `git_operations` (C2)
4. Add `is_error INTEGER DEFAULT 0` to `git_operations` (C2)
5. Add missing index: `CREATE INDEX idx_file_operations_operation_type ON file_operations(operation_type)` (m5)

Note: Column renames (M1-M3) and the denormalized file_path (M4) are deeply embedded in the implementation. Changing these would cascade through all query functions, all API handlers, and all tests. The cost-benefit of renaming columns to match spec names exactly versus keeping the working implementation should be a user decision.

### Phase 5b: Tool result matching (C3)

**File:** `crates/store/src/artifacts.rs`

1. Implement `match_tool_results` function per spec 2.3:
   - Takes `AssistantRecord` + next `UserRecord`
   - Iterates UserRecord's ToolResult content blocks
   - Builds HashMap<tool_use_id, ToolResultPair(summary, is_error)>
2. Integrate into `decompose_artifacts` pipeline:
   - Buffer previous assistant record
   - When user record arrives, call match_tool_results
   - UPDATE file_operations/git_operations SET result_summary, is_error for matched tool_use_ids
3. Update `decompose_artifacts_retroactive` to use the result_content and is_error from tool_executions (currently fetched but stored as unused `_result_content` and `_is_error`)

### Phase 5c: Artifact query layer updates (C4, C5, C7)

**File:** `crates/store/src/artifact_queries.rs`

1. Add `session_id: String` to `SessionArtifacts` struct (C5)
2. Add aggregate computation to `query_session_artifacts`:
   - `total_writes`: COUNT file_operations WHERE operation_type = 'write' AND session_id = ?
   - `total_edits`: COUNT file_operations WHERE operation_type = 'edit' AND session_id = ?
   - `total_reads`: COUNT file_operations WHERE operation_type = 'read' AND session_id = ?
   - `total_git_commits`: COUNT git_operations WHERE operation_type = 'commit' AND session_id = ?
3. Add `result_summary: Option<String>` and `is_error: bool` to `FileOperation` struct
4. Add `result_summary: Option<String>` and `is_error: bool` to `GitOperation` struct
5. Update all query functions that SELECT from file_operations and git_operations to include the new columns

### Phase 5d: ArtifactEvent tagged enum (C7)

**File:** `crates/store/src/artifact_queries.rs`

1. Create `ArtifactEvent` enum per spec:
   ```rust
   #[derive(Serialize)]
   #[serde(tag = "kind")]
   enum ArtifactEvent {
       #[serde(rename = "file")]
       File(FileOperation),
       #[serde(rename = "git")]
       Git(GitOperation),
   }
   ```
2. Update `query_session_timeline` to return `Vec<ArtifactEvent>` instead of `Vec<TimelineEntry>`
3. Option: Keep `TimelineEntry` as an internal type but convert to `ArtifactEvent` at the API boundary, or refactor timeline to build ArtifactEvent directly

**File:** `crates/server/src/api/artifacts_api.rs`
4. Update `session_timeline` handler to return `Vec<ArtifactEvent>`

### Phase 5e: POST /v1/files/query enrichment (C6)

**File:** `crates/server/src/api/files.rs`

1. Expand `FileQueryBody` to match spec:
   ```rust
   pub struct FileQueryBody {
       pub session_ids: Option<Vec<String>>,
       pub file_paths: Option<Vec<String>>,
       pub operation_types: Option<Vec<String>>,
       pub content_contains: Option<String>,
       pub after: Option<String>,
       pub include_content: Option<bool>,
       pub limit: Option<usize>,
   }
   ```
2. Update `query_files` handler to:
   - Support multiple session_ids
   - Support multiple glob patterns (file_paths)
   - Filter by operation_types
   - Search content via content_contains
   - Apply temporal filter (after)
   - Optionally strip content from results (include_content toggle)

**File:** `crates/store/src/artifact_queries.rs`
3. Add a new `query_file_operations_composable` function that accepts the full filter set and builds dynamic SQL

### Phase 5f: NotebookEdit support (M7)

**File:** `crates/store/src/artifacts.rs`

1. Add `"NotebookEdit"` to the match block in `decompose_assistant_artifacts` (line 109), delegating to `extract_write_operation` (per spec: NotebookEdit treated as write)
2. Extract `notebook_path` from input JSON (field name is `notebook_path`, not `file_path`)

### Phase 5g: Git endpoint temporal filters (M9)

**File:** `crates/server/src/api/git.rs`

1. Add `after: Option<String>` and `before: Option<String>` to `GitParams`
2. Pass through to `list_git_operations` (requires extending that function with temporal filter support)

**File:** `crates/store/src/artifact_queries.rs`
3. Extend `list_git_operations` to accept `after` and `before` parameters

---

## 8. Recommended Implementation Order

1. **Phase 5a** (migration 004) — unblocks all result_summary/is_error work
2. **Phase 5b** (match_tool_results) — populates the new columns
3. **Phase 5c** (query layer updates) — expose new columns + aggregates in API types
4. **Phase 5d** (ArtifactEvent enum) — fix timeline response shape
5. **Phase 5e** (POST /v1/files/query) — enrich the composable query endpoint
6. **Phase 5f** (NotebookEdit) — small addition
7. **Phase 5g** (git temporal filters) — small addition

---

## 9. Files Requiring Modification

| File | Phases | Changes |
|------|--------|---------|
| `crates/store/migrations/004_artifact_columns.sql` (NEW) | 5a | Add result_summary, is_error columns + index |
| `crates/store/src/schema.rs` | 5a | Register migration 004 |
| `crates/store/src/artifacts.rs` | 5b, 5f | match_tool_results, NotebookEdit handling |
| `crates/store/src/artifact_queries.rs` | 5c, 5d, 5e, 5g | Struct updates, aggregates, ArtifactEvent enum, composable query, temporal filters |
| `crates/server/src/api/files.rs` | 5e | FileQueryBody expansion, query_files handler rewrite |
| `crates/server/src/api/git.rs` | 5g | Temporal filter params |
| `crates/server/src/api/artifacts_api.rs` | 5c, 5d | SessionArtifacts response update, ArtifactEvent return type |
