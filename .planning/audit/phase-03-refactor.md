# Phase 3 Audit: HTTP API & Daemon — Spec vs. Implementation

## Methodology

Every endpoint in the spec's section 4.1 API table was compared against the
actual router registration in `crates/server/src/api/mod.rs`, the handler
implementations, the store-layer query functions, and the response type structs.

---

## 1. Endpoint-by-Endpoint Comparison

### 1.1 Health

| Spec Endpoint | Impl Status | Notes |
|---|---|---|
| `GET /v1/health` | **Exists** | Response shape matches: `{ status, db_size, record_count, version }` |

**Deviations:** None.

---

### 1.2 Sessions

| Spec Endpoint | Spec Query Params | Impl Status | Impl Params |
|---|---|---|---|
| `GET /v1/sessions` | `?status=&project=&after=&before=&limit=` | **Exists** | `?project=&after=&before=&limit=` |
| `GET /v1/sessions/:id` | — | **Exists** | — |
| `GET /v1/sessions/:id/conversation` | `?include_thinking=&include_tool_io=` | **Exists** | `?include_thinking=&include_tool_io=&limit=&offset=` |
| `GET /v1/sessions/:id/tree` | — | **Exists** | — |
| `GET /v1/sessions/:id/agents` | — | **Exists** | — |
| `GET /v1/sessions/:id/summary` | — | **Exists** | — |

**Deviations:**

| # | Severity | Spec Says | Implementation Does | Change Needed |
|---|---|---|---|---|
| S-1 | **Minor** | `?status=` filter on sessions list | Not implemented — `SessionsParams` has no `status` field | Add optional `status` query param to `SessionsParams` and filter in `list_sessions()`. Requires defining what "status" means (likely active/completed based on whether new records are still being added). |
| S-2 | **Positive** | No pagination on conversation | Implementation adds `limit` and `offset` params | No change needed — this is a sensible enhancement beyond spec. |

**Files:** `crates/server/src/api/sessions.rs:34-43`, `crates/store/src/query.rs:202-273`

---

### 1.3 Messages

| Spec Endpoint | Spec Behavior | Impl Status | Notes |
|---|---|---|---|
| `POST /v1/messages/query` | Flexible query body (see 4.5) | **Exists** | Partial body fields |
| `GET /v1/messages/:uuid` | Single message | **Exists** | Returns `ExportMessage` with content blocks |

**Deviations (POST /v1/messages/query body):**

The spec (section 4.5) defines this composable query body:

```json
{
  "session_ids": ["abc-123"],         // plural, array
  "message_types": ["assistant"],     // plural, array
  "models": ["claude-opus-4-6"],      // plural, array
  "tool_names": ["Bash", "Read"],     // plural, array
  "content_contains": "git commit",
  "after": "2025-02-01T00:00:00Z",
  "before": "2025-03-01T00:00:00Z",
  "is_sidechain": false,
  "min_input_tokens": 1000,
  "limit": 50,
  "offset": 0
}
```

The implementation (`MessageQuery` struct) accepts:

```rust
pub struct MessageQuery {
    pub session_id: Option<String>,     // singular, not array
    pub message_type: Option<String>,   // singular, not array
    pub model: Option<String>,          // singular, not array
    pub tool: Option<String>,           // singular, not array (named "tool" not "tool_names")
    pub after: Option<String>,
    pub before: Option<String>,
    pub limit: Option<usize>,
    // MISSING: offset, content_contains, is_sidechain, min_input_tokens
}
```

| # | Severity | Spec Says | Implementation Does | Change Needed |
|---|---|---|---|---|
| M-1 | **Critical** | `session_ids: [String]` (array) | `session_id: Option<String>` (singular) | Change to `session_ids: Option<Vec<String>>` and use `IN (?)` SQL clause |
| M-2 | **Critical** | `message_types: [String]` (array) | `message_type: Option<String>` (singular) | Change to `message_types: Option<Vec<String>>` |
| M-3 | **Critical** | `models: [String]` (array) | `model: Option<String>` (singular) | Change to `models: Option<Vec<String>>` |
| M-4 | **Critical** | `tool_names: [String]` (array) | `tool: Option<String>` (singular) | Change to `tool_names: Option<Vec<String>>` |
| M-5 | **Moderate** | `content_contains: String` | Missing | Add FTS5 or LIKE-based content filtering |
| M-6 | **Moderate** | `is_sidechain: bool` | Missing | Add `is_sidechain` filter to query builder |
| M-7 | **Moderate** | `min_input_tokens: i64` | Missing | Add HAVING or JOIN filter against `token_usage` |
| M-8 | **Minor** | `offset: i64` | Missing | Add OFFSET clause to SQL |

**Files:** `crates/server/src/api/messages.rs:32-47`, `crates/store/src/query.rs:280-349`

---

### 1.4 Search

| Spec Endpoint | Spec Description | Impl Status | Notes |
|---|---|---|---|
| `GET /v1/search?q=` | FTS5 across all content | **Exists** | Implementation adds `limit` and `offset` params |

**Deviations:** None material. Implementation adds pagination which is a sensible addition.

---

### 1.5 Analytics

| Spec Endpoint | Spec Query Params | Impl Status | Impl Params |
|---|---|---|---|
| `GET /v1/analytics/tokens` | `?session_id=&after=&before=&group_by=` | **Exists** | `?group_by=&session_id=` |
| `GET /v1/analytics/tools` | `?session_id=&after=&before=` | **Exists** | No params |
| `GET /v1/analytics/models` | — | **Exists** | No params |

**Deviations:**

| # | Severity | Spec Says | Implementation Does | Change Needed |
|---|---|---|---|---|
| A-1 | **Moderate** | `analytics/tokens` accepts `?after=&before=` | Missing — `TokensParams` only has `group_by` and `session_id` | Add `after` and `before` params to `TokensParams`; add date range filtering to the three token stats query functions |
| A-2 | **Moderate** | `analytics/tools` accepts `?session_id=&after=&before=` | No query params accepted at all | Add `ToolsParams` struct with `session_id`, `after`, `before`; update `tool_frequency()` to accept filters |

**Files:** `crates/server/src/api/analytics.rs:29-35`, `crates/store/src/query.rs` (token/tool query functions)

---

### 1.6 Export

| Spec Endpoint | Spec Query Params | Impl Status |
|---|---|---|
| `GET /v1/export/:session_id` | `?format=json\|markdown\|csv` | **Exists** |

**Deviations:** None. Spec says "streamed file" — implementation writes to an in-memory buffer then returns. This is acceptable for typical session sizes. True streaming could be a future enhancement but is not a deviation from the contract.

---

### 1.7 Schema

| Spec Endpoint | Impl Status |
|---|---|
| `GET /v1/schema/versions` | **Exists** |
| `GET /v1/schema/drift` | **Exists** |

**Deviations:** None material. Implementation adds `record_type` and `limit` filter params which are sensible additions.

---

### 1.8 Events (SSE)

| Spec Endpoint | Spec Events | Impl Events |
|---|---|---|
| `GET /v1/events` | `record:added`, `session:started`, `schema:drift`, `version:changed`, `file:written`, `file:edited`, `git:commit` | All 7 event types defined |

**Deviations:**

| # | Severity | Spec Says | Implementation Does | Change Needed |
|---|---|---|---|---|
| E-1 | **Minor** | `record:added` data: `{session_id, uuid, type, timestamp}` | Data: `{session_id, records_synced, file_path}` — batch summary rather than per-record | Spec shows per-record semantics with uuid+type; implementation sends batch counts. Consider whether spec intent is per-record or batch. If per-record is desired, emit one event per record. |
| E-2 | **Minor** | `session:started` data: `{session_id, project_path, version}` | Data: `{session_id}` — missing `project_path` and `version` | Add `project_path` and `version` fields to `SessionStarted` variant |
| E-3 | **Minor** | `schema:drift` data: `{version, new_fields, type}` | Data: `{new_fields, session_id}` — `new_fields` is a count not array; missing `version` and `type` | Align data payload: change `new_fields` to `Vec<String>`, add `version` and `type` fields |
| E-4 | **Minor** | `file:written` data includes `operation` and `timestamp` | Data: `{session_id, file_path, message_uuid}` — missing `operation` and `timestamp` | Add `timestamp` field; `operation` is implicit from event name |
| E-5 | **Minor** | `file:edited` data includes `old_content` and `new_content` | Data: `{session_id, file_path, message_uuid}` | Add `old_content` and `new_content` if feasible (may be large) |

**Files:** `crates/server/src/events.rs`

---

### 1.9 Files (Artifact Layer)

| Spec Endpoint | Spec Query Params | Impl Status | Notes |
|---|---|---|---|
| `GET /v1/files` | `?session_id=&path=&after=&before=&limit=` | **Exists** | `?session_id=&path=&limit=` |
| `GET /v1/files/:file_id` | — | **Exists** | Returns `{file, operations}` |
| `GET /v1/files/:file_id/content` | `?at=<uuid>` | **Exists** | Point-in-time reconstruction works |
| `GET /v1/files/:file_id/diff` | — | **Exists** | Unified diff output |
| `GET /v1/files/search?q=` | — | **Exists** | FTS5 over file operation content |
| `POST /v1/files/query` | Full composable body (see 4.4) | **Exists** | Partial body fields |

**Deviations:**

| # | Severity | Spec Says | Implementation Does | Change Needed |
|---|---|---|---|---|
| F-1 | **Moderate** | `GET /v1/files` accepts `?after=&before=` date range filters | Missing — `FilesParams` lacks `after` and `before` | Add `after` and `before` to `FilesParams`; filter against `first_seen` or `last_modified` |
| F-2 | **Critical** | `POST /v1/files/query` body per spec 4.4: `{session_ids, file_paths, operation_types, content_contains, after, include_content, limit}` | Body: `{pattern, session_id, limit}` | Major gap — see detail below |

**POST /v1/files/query detail (F-2):**

Spec body (section 4.4):
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

Implementation body (`FileQueryBody`):
```rust
pub struct FileQueryBody {
    pub pattern: Option<String>,     // single glob, not array of paths
    pub session_id: Option<String>,  // singular, not array
    pub limit: Option<usize>,
    // MISSING: operation_types, content_contains, after, include_content
}
```

Additionally, the spec says the return type is `[FileOperation]` (operation-level results), but the implementation returns `[FileEntry]` (file-level entries). This is a semantic mismatch — the spec's composable query targets operations, not just files.

| Sub-deviation | What's Missing |
|---|---|
| F-2a | `session_ids` should be array, not singular |
| F-2b | `file_paths` should be array of globs (impl has single `pattern`) |
| F-2c | `operation_types` filter missing entirely |
| F-2d | `content_contains` missing |
| F-2e | `after` date filter missing |
| F-2f | `include_content` flag missing |
| F-2g | Return type should be `[FileOperation]`, not `[FileEntry]` |

**Files:** `crates/server/src/api/files.rs:63-71`, `crates/store/src/artifact_queries.rs`

---

### 1.10 Git

| Spec Endpoint | Spec Query Params | Impl Status | Impl Params |
|---|---|---|---|
| `GET /v1/git` | `?session_id=&type=commit&after=&before=` | **Exists** | `?session_id=&operation_type=&limit=` |
| `GET /v1/git/commits` | — | **Exists** | `?limit=` |
| `GET /v1/git/commits/:session_id` | — | **Exists** | — |

**Deviations:**

| # | Severity | Spec Says | Implementation Does | Change Needed |
|---|---|---|---|---|
| G-1 | **Minor** | Query param named `type=` | Named `operation_type=` | Non-breaking naming difference — `operation_type` is arguably clearer. Keep as-is unless strict spec compliance required. |
| G-2 | **Moderate** | `?after=&before=` date filters | Missing on `GitParams` | Add `after` and `before` to `GitParams`; update `list_git_operations()` |

**Files:** `crates/server/src/api/git.rs:29-36`, `crates/store/src/artifact_queries.rs:434-489`

---

### 1.11 Artifacts

| Spec Endpoint | Impl Status | Notes |
|---|---|---|
| `GET /v1/artifacts/:session_id` | **Exists** | Returns `SessionArtifacts` |
| `GET /v1/artifacts/:session_id/timeline` | **Exists** | Returns `[TimelineEntry]` |

**Deviations:**

| # | Severity | Spec Says | Implementation Does | Change Needed |
|---|---|---|---|---|
| AR-1 | **Moderate** | `SessionArtifacts` has `total_writes`, `total_edits`, `total_reads`, `total_git_commits` summary counts | Implementation returns raw lists with no summary counts: `{files, git_operations, tool_executions}` | Add aggregate count fields to `SessionArtifacts` struct |
| AR-2 | **Minor** | Spec `SessionArtifacts` has `files_touched: [FileEntry]` | Implementation field named `files: [FileEntry]` | Rename to `files_touched` for spec alignment, or accept the deviation |
| AR-3 | **Minor** | Spec timeline uses `ArtifactEvent` enum with `{kind: "file" \| "git"}` | Implementation uses `TimelineEntry` with `entry_type: String` and includes `tool_execution` as a third type | The `tool_execution` addition is valuable. The flat struct vs tagged enum is a serialization style difference. |

**Files:** `crates/store/src/artifact_queries.rs:99-105`, `crates/server/src/api/artifacts_api.rs`

---

### 1.12 Response Type Field Comparison (spec section 4.2)

**FileEntry:**

| Spec Field | Type | Impl Field | Match? |
|---|---|---|---|
| `file_id` | i64 | `id` | **Name mismatch** — `id` vs `file_id` |
| `session_id` | String | `session_id` | Yes |
| `file_path` | String | `file_path` | Yes |
| `first_seen_at` | String | `first_seen` | **Name mismatch** — missing `_at` suffix |
| `last_modified_at` | String | `last_modified` | **Name mismatch** — missing `_at` suffix |
| `operation_count` | i64 | `operation_count` | Yes |
| `operations` | Vec\<FileOperation\> | Not on FileEntry | Included in `FileDetailResponse.operations` only on `GET /files/:id` — acceptable |

**FileOperation:**

| Spec Field | Type | Impl Field | Match? |
|---|---|---|---|
| `operation_id` | i64 | `id` | **Name mismatch** — `id` vs `operation_id` |
| `operation_type` | String | `operation_type` | Yes |
| `timestamp` | String | `timestamp` | Yes |
| `message_uuid` | String | `message_uuid` | Yes |
| `tool_use_id` | String | `tool_use_id` | Yes |
| `content` | Option\<String\> | `content` | Yes |
| `old_content` | Option\<String\> | `old_content` | Yes |
| `command` | Option\<String\> | `command` | Yes |
| `result_summary` | Option\<String\> | Not in struct | **Missing** — `result_summary` and `is_error` fields absent from `FileOperation` |
| `is_error` | bool | Not in struct | **Missing** |

**GitOperation:**

| Spec Field | Type | Impl Field | Match? |
|---|---|---|---|
| `operation_type` | String | `operation_type` | Yes |
| `command` | String | `command` | Yes |
| `commit_message` | Option\<String\> | `commit_message` | Yes |
| `branch` | Option\<String\> | `branch` | Yes |
| `timestamp` | String | `timestamp` | Yes |
| `session_id` | String | `session_id` | Yes |
| `result_summary` | Option\<String\> | Not in struct | **Missing** |
| `is_error` | bool | Not in struct | **Missing** |

| # | Severity | Issue | Change Needed |
|---|---|---|---|
| T-1 | **Moderate** | `FileEntry.id` should be `file_id` per spec | Rename field or add `#[serde(rename = "file_id")]` |
| T-2 | **Minor** | `FileEntry.first_seen` should be `first_seen_at` | Rename or add serde rename |
| T-3 | **Minor** | `FileEntry.last_modified` should be `last_modified_at` | Rename or add serde rename |
| T-4 | **Moderate** | `FileOperation.id` should be `operation_id` | Rename or add serde rename |
| T-5 | **Moderate** | `FileOperation` missing `result_summary` and `is_error` | Add fields; query file_operations table columns that store these |
| T-6 | **Moderate** | `GitOperation` missing `result_summary` and `is_error` | Add fields; query git_operations table columns that store these |

**Files:** `crates/store/src/artifact_queries.rs:27-65`

---

## 2. Missing Endpoints / Architectural Concerns

### 2.1 No `GET /v1/projects` Endpoint

The spec mentions `?project=` filter on sessions but does not explicitly define a `/v1/projects` endpoint. The implementation also has no projects endpoint. The `project_path` on sessions serves this purpose implicitly. **No action needed** unless a dedicated projects list is desired.

### 2.2 Query Compiler Pattern

The spec (section 4.5) describes query bodies that "compile to parameterized SQL." The implementation uses dynamic WHERE clause building via `Vec<Box<dyn ToSql>>` and string-formatted SQL — this IS a form of query compilation. However, it handles singular values rather than arrays.

**Assessment:** The pattern is correct in spirit but needs extension to support array-valued filters (`IN` clauses) per the spec's composable query design.

### 2.3 Unix Domain Socket

**Exists and working.** `serve.rs` binds both TCP and UDS listeners with shared CancellationToken for graceful shutdown. Socket path resolution follows CLI arg > env var > default pattern. Stale socket cleanup is implemented.

### 2.4 Daemon Client (CLI-to-Daemon Routing)

**Exists and working.** `daemon_client.rs` implements HTTP-over-UDS with health check probe, connection mode detection, and fallback to direct DB. All major endpoints have corresponding client methods.

**Gap:** DaemonClient is missing methods for some artifact endpoints:
- No `file_detail()` method (GET /v1/files/:file_id)
- No `file_content()` method (GET /v1/files/:file_id/content)
- No `file_diff()` method (GET /v1/files/:file_id/diff)
- No `file_search()` method (GET /v1/files/search)
- No `file_query()` method (POST /v1/files/query)
- No `git_commits()` method (GET /v1/git/commits)
- No `session_timeline()` method (GET /v1/artifacts/:session_id/timeline)

These are needed for CLI subcommands that route through the daemon.

---

## 3. Prioritized Refactoring Plan

### Priority 1: Critical (API contract violations)

#### R-1: POST /v1/messages/query — array-valued filters [M-1 through M-4]

**Problem:** All filter fields are singular (`Option<String>`) instead of arrays (`Option<Vec<String>>`).

**Changes:**
1. `crates/server/src/api/messages.rs` — Change `MessageQuery` struct fields to:
   - `session_ids: Option<Vec<String>>`
   - `message_types: Option<Vec<String>>`
   - `models: Option<Vec<String>>`
   - `tool_names: Option<Vec<String>>`
2. `crates/store/src/query.rs:280-349` — Rewrite `query_messages()` to generate `IN (?, ?, ?)` clauses for array fields
3. `crates/server/src/daemon_client.rs:315-336` — Update `query_messages()` to send array-valued JSON body

**Backward compatibility:** Accept both singular and array form via serde `#[serde(alias)]` or custom deserializer during transition.

#### R-2: POST /v1/files/query — match spec body [F-2]

**Problem:** Body only has `pattern`, `session_id`, `limit`. Spec requires 7 fields and returns operations not file entries.

**Changes:**
1. `crates/server/src/api/files.rs:63-71` — Rewrite `FileQueryBody`:
   ```rust
   pub struct FileQueryBody {
       pub session_ids: Option<Vec<String>>,
       pub file_paths: Option<Vec<String>>,  // array of globs
       pub operation_types: Option<Vec<String>>,
       pub content_contains: Option<String>,
       pub after: Option<String>,
       pub include_content: Option<bool>,
       pub limit: Option<usize>,
   }
   ```
2. `crates/store/src/artifact_queries.rs` — Add new `query_file_operations_composable()` function that builds dynamic SQL with the full filter set
3. `crates/server/src/api/files.rs:255-293` — Rewrite `query_files` handler to return `Vec<FileOperation>` instead of `Vec<FileEntry>`

### Priority 2: Moderate (missing query params, missing fields)

#### R-3: POST /v1/messages/query — missing filter fields [M-5 through M-8]

**Changes:**
1. Add `content_contains`, `is_sidechain`, `min_input_tokens`, `offset` to `MessageQuery`
2. `content_contains` — use FTS5 or `LIKE %?%` against message_content
3. `is_sidechain` — add `m.is_sidechain = ?` filter
4. `min_input_tokens` — JOIN or subquery against `token_usage`
5. `offset` — add `OFFSET ?N` to SQL

#### R-4: Analytics date range filters [A-1, A-2]

**Changes:**
1. `crates/server/src/api/analytics.rs:29-35` — Add `after` and `before` to `TokensParams`
2. Add `ToolsParams` struct with `session_id`, `after`, `before`
3. `crates/store/src/query.rs` — Update `token_stats_by_model()`, `token_stats_by_session()`, `token_stats_by_day()`, `tool_frequency()` to accept optional date range and session_id params

#### R-5: Files date range filters [F-1]

**Changes:**
1. `crates/server/src/api/files.rs:35-42` — Add `after` and `before` to `FilesParams`
2. `crates/store/src/artifact_queries.rs:115-166` — Update `list_files()` to filter on `last_modified` or `first_seen`

#### R-6: Git date range filters [G-2]

**Changes:**
1. `crates/server/src/api/git.rs:29-36` — Add `after` and `before` to `GitParams`
2. `crates/store/src/artifact_queries.rs:434-489` — Update `list_git_operations()` to filter on `timestamp`

#### R-7: Response type field names [T-1 through T-6]

**Changes:**
1. `crates/store/src/artifact_queries.rs:27-35` `FileEntry` — Add serde renames: `id` -> `file_id`, `first_seen` -> `first_seen_at`, `last_modified` -> `last_modified_at`
2. `crates/store/src/artifact_queries.rs:39-51` `FileOperation` — Add serde rename: `id` -> `operation_id`. Add `result_summary: Option<String>` and `is_error: bool` fields; update query to select these columns.
3. `crates/store/src/artifact_queries.rs:54-65` `GitOperation` — Add serde rename: `id` -> `git_op_id`. Add `result_summary: Option<String>` and `is_error: bool` fields; update query to select these columns.

**Note:** These renames only affect JSON serialization via `#[serde(rename)]`; Rust code can continue using `id` internally.

#### R-8: SessionArtifacts summary counts [AR-1]

**Changes:**
1. `crates/store/src/artifact_queries.rs:99-105` — Add fields:
   ```rust
   pub total_writes: i64,
   pub total_edits: i64,
   pub total_reads: i64,
   pub total_git_commits: i64,
   ```
2. `crates/store/src/artifact_queries.rs:546-601` `query_session_artifacts()` — Compute counts from the existing query results

#### R-9: DaemonClient missing artifact methods

**Changes:**
1. `crates/server/src/daemon_client.rs` — Add methods:
   - `file_detail(file_id: i64)` -> GET /v1/files/{file_id}
   - `file_content(file_id: i64, at: Option<&str>)` -> GET /v1/files/{file_id}/content
   - `file_diff(file_id: i64)` -> GET /v1/files/{file_id}/diff
   - `file_search(q: &str, limit, offset)` -> GET /v1/files/search
   - `file_query(body)` -> POST /v1/files/query
   - `git_commits(session_id: Option<&str>, limit)` -> GET /v1/git/commits
   - `session_timeline(session_id, limit)` -> GET /v1/artifacts/{session_id}/timeline

### Priority 3: Minor (naming, SSE payloads)

#### R-10: SSE event data payload alignment [E-1 through E-5]

**Changes (each is small):**
1. `RecordAdded` — add `uuid`, `type`, `timestamp` fields (consider per-record emission vs batch)
2. `SessionStarted` — add `project_path` and `version` fields
3. `SchemaDrift` — change `new_fields` from `usize` to `Vec<String>`, add `version` and `type`
4. `FileWritten` — add `timestamp` field
5. `FileEdited` — add `old_content`, `new_content` if feasible (or add a note that large content is omitted)
6. Update `SseEvent::to_json_data()` for all affected variants

**Files:** `crates/server/src/events.rs`, `crates/server/src/watcher.rs` (where events are emitted)

#### R-11: Git query param naming [G-1]

**Assessment:** `operation_type` is clearer than `type` (which is a Rust keyword). Keep current naming. Document the deviation.

---

## 4. Summary Statistics

| Category | Count |
|---|---|
| Endpoints in spec | 28 |
| Endpoints implemented | 28 |
| Endpoints missing | 0 |
| Critical deviations | 6 (M-1 through M-4, F-2a, F-2g) |
| Moderate deviations | 12 |
| Minor deviations | 9 |
| Refactoring items | 11 (R-1 through R-11) |

**Overall assessment:** All 28 endpoints exist and are routable. The primary structural gaps are in the composable query body designs (POST /v1/messages/query and POST /v1/files/query), where the implementation uses singular-valued filters instead of the spec's array-valued composable pattern. The response type structs are close but have naming mismatches and missing fields (result_summary, is_error). The UDS infrastructure is solid. The SSE event payloads have minor data shape differences from the spec examples.

---

## Demo Requirements

After refactoring, `/gsd:demo-phase` must capture evidence for each item. Phase 3 demos validate the API handler layer, response shapes, DaemonClient routing, and date range filters. Phase 3 depends on Phase 2 (composable query compiler) and Phase 5 (artifact queries) completing first.

### Demo 1: All 28 endpoints return non-error HTTP status

**Validates:** All endpoints remain routable after refactor
**Category:** API curl

```
$ ./target/debug/claude-history serve &
$ sleep 2
$ for endpoint in \
    "/v1/health" \
    "/v1/sessions?limit=1" \
    "/v1/search?q=test" \
    "/v1/analytics/tokens" \
    "/v1/analytics/tools" \
    "/v1/analytics/models" \
    "/v1/schema/versions" \
    "/v1/schema/drift" \
    "/v1/files?limit=1" \
    "/v1/files/search?q=test" \
    "/v1/git?limit=1" \
    "/v1/git/commits?limit=1" \
    "/v1/events"; do
  STATUS=$(curl -s -o /dev/null -w "%{http_code}" http://localhost:7424$endpoint)
  echo "$endpoint: $STATUS"
done
→ all must return 200 (except /v1/events which is SSE stream)
```

**Observation target:** No endpoints broken by refactor. All return 200 or appropriate status.

### Demo 2: Response field names match spec section 4.2

**Validates:** T-1 through T-6 (FileEntry uses file_id/first_seen_at/last_modified_at, FileOperation uses operation_id, result_summary/is_error present)
**Category:** API curl

```
$ curl -s http://localhost:7424/v1/files?limit=1 | jq '.[0] | keys'
→ must contain "file_id", "first_seen_at", "last_modified_at" (not "id", "first_seen", "last_modified")

$ curl -s "http://localhost:7424/v1/files/1" | jq '.operations[0] | keys'
→ must contain "operation_id", "result_summary", "is_error" (not "id")

$ curl -s http://localhost:7424/v1/git?limit=1 | jq '.[0] | keys'
→ must contain "result_summary", "is_error"
```

**Observation target:** JSON field names in responses match spec section 4.2 type definitions exactly.

### Demo 3: SessionArtifacts includes aggregates and session_id

**Validates:** AR-1 (spec section 4.2 — total_writes, total_edits, total_reads, total_git_commits)
**Category:** API curl

```
$ curl -s http://localhost:7424/v1/artifacts/SESSION_ID | jq '{session_id, total_writes, total_edits, total_reads, total_git_commits}'
→ all 5 fields must be present and numeric
```

**Observation target:** SessionArtifacts response matches spec struct with computed aggregates.

### Demo 4: Analytics date range filters

**Validates:** A-1, A-2 (spec section 4.1 — analytics/tokens and analytics/tools accept after/before)
**Category:** API curl

```
$ curl -s "http://localhost:7424/v1/analytics/tokens?after=2026-02-20&group_by=model" | jq '. | length'
→ must return filtered results (fewer than unfiltered)

$ curl -s "http://localhost:7424/v1/analytics/tools?session_id=SESSION_ID" | jq '.[0]'
→ must return tool frequency filtered to one session
```

**Observation target:** Date range and session_id filters work on analytics endpoints.

### Demo 5: Files and git date range filters

**Validates:** F-1, G-2 (spec section 4.1 — files and git accept after/before)
**Category:** API curl

```
$ curl -s "http://localhost:7424/v1/files?after=2026-02-20&limit=5" | jq '. | length'
→ must return only files modified after the date

$ curl -s "http://localhost:7424/v1/git?after=2026-02-20&limit=5" | jq '. | length'
→ must return only git operations after the date
```

**Observation target:** Temporal filters work on artifact list endpoints.

### Demo 6: DaemonClient routes all artifact subcommands

**Validates:** R-9 (missing DaemonClient methods for 7 artifact endpoints)
**Category:** CLI exec

```
$ ./target/debug/claude-history serve &
$ sleep 2

$ ./target/debug/claude-history files --limit 3
→ must return file table (routed through daemon)

$ ./target/debug/claude-history git-log --limit 3
→ must return git operations (routed through daemon)

$ ./target/debug/claude-history artifacts SESSION_ID
→ must return combined view (routed through daemon)

$ kill %1
```

**Observation target:** CLI subcommands for files, git-log, and artifacts route through the daemon socket when daemon is running, rather than opening the DB directly.

### Demo 7: Sessions status filter

**Validates:** S-1 (spec section 4.1 — `?status=` filter on sessions list)
**Category:** API curl

```
$ curl -s "http://localhost:7424/v1/sessions?status=active&limit=3" | jq '. | length'
→ must return only active sessions (or empty if none match)
```

**Observation target:** The `?status=` query parameter is accepted and filters results.
