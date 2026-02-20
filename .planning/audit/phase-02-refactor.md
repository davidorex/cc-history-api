# Phase 2 Audit: Deviations & Refactoring Plan

## Audit Scope

Compared the spec (`cc-history-api.md` sections 2.1, 4.5, 4.7) against the actual implementation in:
- `crates/store/src/fts.rs`
- `crates/store/src/query.rs`
- `crates/server/src/main.rs` (CLI)
- `crates/server/src/api/messages.rs` (HTTP API)
- `crates/store/migrations/002_fts5.sql`
- `crates/store/migrations/003_artifacts.sql`

---

## Deviation Summary

The implementation built a working FTS5 search layer and CLI with 14 subcommands. However, the core architectural intent of the spec's `POST /v1/messages/query` endpoint -- a composable query compiler accepting plural arrays and advanced filters -- was reduced to fixed single-value parameters. The CLI `query` subcommand mirrors this reduction. Several spec-described fields are entirely missing from both the API request body and the underlying `query_messages()` function.

---

## Deviations

### DEV-01: `session_ids` (plural array) reduced to `session_id` (singular string)
- **Severity:** CRITICAL
- **Spec says:** `"session_ids": ["abc-123"]` -- an array of session IDs for cross-session querying
- **Implementation does:** `session_id: Option<String>` -- single session ID, exact match
- **Files affected:**
  - `crates/store/src/query.rs` line 281: `query_messages()` takes `session_id: Option<&str>`
  - `crates/server/src/api/messages.rs` line 34: `MessageQuery` has `session_id: Option<String>`
  - `crates/server/src/main.rs` line 114: CLI `Query` has `session_id: Option<String>`
- **Change needed:** Accept `session_ids: Option<Vec<String>>` in all three locations. The query builder should generate `session_id IN (?, ?, ...)` when multiple IDs are provided. For backward compatibility, the HTTP API can accept both `session_id` (singular, deprecated) and `session_ids` (plural, canonical). The CLI should accept `--session-id` repeated or comma-separated.

### DEV-02: `message_types` (plural array) reduced to `message_type` (singular string)
- **Severity:** MODERATE
- **Spec says:** `"message_types": ["assistant"]` -- array of message types
- **Implementation does:** `message_type: Option<String>` -- single type filter
- **Files affected:**
  - `crates/store/src/query.rs` line 283: `message_type: Option<&str>`
  - `crates/server/src/api/messages.rs` line 36: `message_type: Option<String>`
  - `crates/server/src/main.rs` line 117: `--type` accepts single value
- **Change needed:** Accept `message_types: Option<Vec<String>>`. Generate `type IN (?, ?, ...)`. Current data has only "user" and "assistant" so the practical impact is limited, but the spec intends array semantics.

### DEV-03: `models` (plural array) reduced to `model` (singular string)
- **Severity:** MODERATE
- **Spec says:** `"models": ["claude-opus-4-6"]` -- array of model names
- **Implementation does:** `model: Option<String>` -- single model filter
- **Files affected:**
  - `crates/store/src/query.rs` line 284: `model: Option<&str>`
  - `crates/server/src/api/messages.rs` line 38: `model: Option<String>`
  - `crates/server/src/main.rs` line 119: `--model` accepts single value
- **Change needed:** Accept `models: Option<Vec<String>>`. Generate `model IN (?, ?, ...)`.

### DEV-04: `tool_names` (plural array) reduced to `tool` (singular string)
- **Severity:** MODERATE
- **Spec says:** `"tool_names": ["Bash", "Read"]` -- array of tool names, field named `tool_names`
- **Implementation does:** `tool: Option<String>` -- single tool name, field named `tool`
- **Files affected:**
  - `crates/store/src/query.rs` line 285: `tool: Option<&str>`
  - `crates/server/src/api/messages.rs` line 40: `tool: Option<String>` (named `tool`, not `tool_names`)
  - `crates/server/src/main.rs` line 122: `--tool` accepts single value
- **Change needed:** Rename to `tool_names: Option<Vec<String>>` in the API. Generate `EXISTS (SELECT 1 FROM tool_executions te WHERE te.message_uuid = m.uuid AND te.tool_name IN (?, ?, ...))`. The CLI can accept `--tool` repeated or comma-separated.

### DEV-05: `content_contains` field missing entirely
- **Severity:** CRITICAL
- **Spec says:** `"content_contains": "git commit"` -- text search filter within the query endpoint
- **Implementation does:** No `content_contains` field in `MessageQuery` or `query_messages()`. FTS search is only available via the separate `GET /v1/search` endpoint.
- **Files affected:**
  - `crates/store/src/query.rs`: `query_messages()` has no content search capability
  - `crates/server/src/api/messages.rs`: `MessageQuery` has no `content_contains` field
- **Change needed:** Add `content_contains: Option<String>` to the query body. When present, add a JOIN or subquery against `fts_message_content` or `message_content` to filter messages whose content blocks match the text. This integrates FTS into the composable query compiler rather than keeping it isolated.

### DEV-06: `is_sidechain` filter missing entirely
- **Severity:** MODERATE
- **Spec says:** `"is_sidechain": false` -- filter by sidechain status
- **Implementation does:** No `is_sidechain` field in `MessageQuery` or `query_messages()`. The `messages` table has an `is_sidechain` column (stored as integer), but `query_messages()` does not expose it as a filter.
- **Files affected:**
  - `crates/store/src/query.rs`: `query_messages()` has no is_sidechain filter
  - `crates/server/src/api/messages.rs`: `MessageQuery` has no `is_sidechain` field
- **Change needed:** Add `is_sidechain: Option<bool>` to the query body. When present, add `m.is_sidechain = ?N` (converting bool to 0/1 for SQLite) to the WHERE clause.

### DEV-07: `min_input_tokens` filter missing entirely
- **Severity:** MODERATE
- **Spec says:** `"min_input_tokens": 1000` -- filter for messages with at least N input tokens
- **Implementation does:** No `min_input_tokens` field in `MessageQuery` or `query_messages()`.
- **Files affected:**
  - `crates/store/src/query.rs`: `query_messages()` has no token filter
  - `crates/server/src/api/messages.rs`: `MessageQuery` has no `min_input_tokens` field
- **Change needed:** Add `min_input_tokens: Option<i64>` to the query body. When present, add a JOIN or EXISTS against `token_usage` table: `EXISTS (SELECT 1 FROM token_usage tu WHERE tu.message_uuid = m.uuid AND tu.input_tokens >= ?N)`.

### DEV-08: `offset` field missing from query endpoint
- **Severity:** MINOR
- **Spec says:** `"offset": 0` -- pagination offset
- **Implementation does:** `query_messages()` has no offset parameter. Only `limit` is supported.
- **Files affected:**
  - `crates/store/src/query.rs` line 281: no `offset` parameter
  - `crates/server/src/api/messages.rs`: `MessageQuery` has no `offset` field
- **Change needed:** Add `offset: Option<usize>` (defaulting to 0) to both the query function and the API body. Add `OFFSET ?N` to the SQL.

### DEV-09: FTS virtual table naming diverges from spec
- **Severity:** MINOR
- **Spec says:** `message_content_fts` and `file_content_fts`
- **Implementation does:** `fts_message_content` and `fts_file_operations`
- **Files affected:**
  - `crates/store/migrations/002_fts5.sql`: creates `fts_message_content`
  - `crates/store/migrations/003_artifacts.sql`: creates `fts_file_operations`
  - `crates/store/src/fts.rs`: references `fts_message_content` and `fts_file_operations`
- **Change needed:** This is a naming convention difference. The spec used `{table}_fts` while the implementation uses `fts_{table}`. The implementation naming is arguably more consistent (prefix-grouped). This deviation is cosmetic and could remain as-is unless strict spec compliance is required. If changed, it requires migration versioning to rename the virtual tables.

### DEV-10: CLI `query` command uses `--tool` (singular) instead of spec's `--tool` being plural-capable
- **Severity:** MINOR
- **Spec says:** `claude-history query --session <id> [--type assistant] [--tool Bash] [--contains "text"]`
- **Implementation does:** `--tool` accepts single value, `--contains` is missing entirely
- **Files affected:**
  - `crates/server/src/main.rs` line 122: `--tool` is `Option<String>`
- **Change needed:** The CLI `--tool` could remain singular (spec example shows singular), but `--contains` is missing entirely (maps to `content_contains` from DEV-05). Add `--contains` flag to the CLI Query subcommand.

### DEV-11: `query_messages()` architecture -- fixed parameters vs. composable query struct
- **Severity:** CRITICAL (architectural)
- **Spec says:** POST /v1/messages/query accepts a composable JSON body that "compiles to parameterized SQL against the normalized tables. Consumers never write SQL."
- **Implementation does:** `query_messages()` is a function with 8 positional `Option` parameters. Adding new filters requires changing the function signature everywhere it's called (query.rs, messages.rs, main.rs, daemon_client.rs).
- **Files affected:**
  - `crates/store/src/query.rs` lines 280-288: function signature
  - All callers (at least 3 call sites)
- **Change needed:** Refactor to accept a `MessageQueryParams` struct:
  ```rust
  pub struct MessageQueryParams {
      pub session_ids: Option<Vec<String>>,
      pub message_types: Option<Vec<String>>,
      pub models: Option<Vec<String>>,
      pub tool_names: Option<Vec<String>>,
      pub content_contains: Option<String>,
      pub after: Option<String>,
      pub before: Option<String>,
      pub is_sidechain: Option<bool>,
      pub min_input_tokens: Option<i64>,
      pub limit: usize,
      pub offset: usize,
  }
  ```
  This is the core architectural change that enables all the other deviations (DEV-01 through DEV-08) to be resolved cleanly.

### DEV-12: Artifact layer tables have structural differences from spec schema
- **Severity:** MINOR
- **Spec says:** `files` table has `file_id INTEGER PRIMARY KEY`, `file_operations` has `operation_id INTEGER PRIMARY KEY`, `file_operations` has `file_id` foreign key referencing `files`.
- **Implementation does:** `files` table uses `id` instead of `file_id`; `file_operations` uses `id` instead of `operation_id`; `file_operations` has `file_path TEXT` directly instead of a `file_id` FK to `files`. There is no `result_summary` or `is_error` column in `file_operations`.
- **Files affected:**
  - `crates/store/migrations/003_artifacts.sql`
- **Change needed:** The denormalized `file_path` in `file_operations` is arguably simpler and avoids JOIN overhead, but diverges from the spec's normalized design with `file_id` FK. The missing `result_summary` and `is_error` columns mean tool result data is not tracked in file operations. This should be evaluated for whether the spec's normalized design or the implementation's denormalized design better serves actual use cases. The missing `result_summary`/`is_error` is a feature gap.

---

## Refactoring Plan (Ordered by Priority)

### Step 1: Create `MessageQueryParams` struct (resolves DEV-11)
**File:** `crates/store/src/query.rs`

Create a new struct that holds all query parameters as a single composable unit:
```rust
#[derive(Debug, Default, Deserialize)]
pub struct MessageQueryParams {
    pub session_ids: Option<Vec<String>>,
    pub message_types: Option<Vec<String>>,
    pub models: Option<Vec<String>>,
    pub tool_names: Option<Vec<String>>,
    pub content_contains: Option<String>,
    pub after: Option<String>,
    pub before: Option<String>,
    pub is_sidechain: Option<bool>,
    pub min_input_tokens: Option<i64>,
    pub limit: Option<usize>,  // defaults to 100
    pub offset: Option<usize>, // defaults to 0
}
```

Refactor `query_messages()` to accept `&MessageQueryParams` instead of 8 positional parameters. The dynamic WHERE clause builder stays the same pattern but handles arrays via `IN (?, ?, ...)` clauses.

### Step 2: Update `query_messages()` SQL builder (resolves DEV-01 through DEV-08)
**File:** `crates/store/src/query.rs`

Modify the dynamic WHERE clause builder to:
- Generate `m.session_id IN (?, ?, ...)` for `session_ids` arrays
- Generate `m.type IN (?, ?, ...)` for `message_types` arrays
- Generate `m.model IN (?, ?, ...)` for `models` arrays
- Generate `EXISTS (SELECT 1 FROM tool_executions te WHERE te.message_uuid = m.uuid AND te.tool_name IN (?, ...))` for `tool_names` arrays
- JOIN `fts_message_content` via `message_content` for `content_contains`
- Add `m.is_sidechain = ?N` for `is_sidechain`
- Add `EXISTS (SELECT 1 FROM token_usage tu WHERE tu.message_uuid = m.uuid AND tu.input_tokens >= ?N)` for `min_input_tokens`
- Add `OFFSET ?N` for pagination

### Step 3: Update HTTP API request body (resolves DEV-01 through DEV-08 at API layer)
**File:** `crates/server/src/api/messages.rs`

Replace the current `MessageQuery` struct with one that matches the spec:
```rust
#[derive(Debug, Deserialize)]
pub struct MessageQuery {
    pub session_ids: Option<Vec<String>>,
    pub message_types: Option<Vec<String>>,
    pub models: Option<Vec<String>>,
    pub tool_names: Option<Vec<String>>,
    pub content_contains: Option<String>,
    pub after: Option<String>,
    pub before: Option<String>,
    pub is_sidechain: Option<bool>,
    pub min_input_tokens: Option<i64>,
    pub limit: Option<usize>,
    pub offset: Option<usize>,
}
```

Convert this to `MessageQueryParams` in the handler and pass to the updated `query_messages()`.

### Step 4: Update CLI `query` subcommand (resolves DEV-10)
**File:** `crates/server/src/main.rs`

- Add `--contains <text>` flag for content_contains
- Keep `--session-id`, `--type`, `--model`, `--tool` as singular for CLI ergonomics (most CLI use is single-value)
- Add `--offset` flag
- Add `--sidechain` / `--no-sidechain` flags for is_sidechain filtering
- Add `--min-input-tokens <N>` flag

The CLI handler constructs a `MessageQueryParams` from these flags, wrapping singular values in `vec![value]` where needed.

### Step 5: Update daemon_client query routing
**File:** `crates/server/src/daemon_client.rs`

Update `query_messages()` method to send the new JSON body shape when routing through the daemon. The daemon already receives POST /v1/messages/query, so the request body just needs to match the new `MessageQuery` struct.

### Step 6 (Optional): FTS virtual table renaming (resolves DEV-09)
**Files:** New migration SQL, `crates/store/src/fts.rs`

Only if strict spec naming compliance is required. Would need a migration 004 that drops and recreates the FTS virtual tables with spec-compliant names. Low priority -- the current naming is consistent and functional.

### Step 7 (Optional): Artifact table normalization (resolves DEV-12)
**Files:** New migration SQL, `crates/store/src/artifact_queries.rs`

Only if the normalized file_id FK design is preferred over the current denormalized file_path design. Would also add `result_summary` and `is_error` columns to `file_operations`. This is a larger schema migration.

---

## Files to Modify (Steps 1-5)

| File | Change |
|------|--------|
| `crates/store/src/query.rs` | Add `MessageQueryParams` struct; refactor `query_messages()` to accept it; implement array IN clauses, content_contains JOIN, is_sidechain filter, min_input_tokens filter, offset |
| `crates/server/src/api/messages.rs` | Replace `MessageQuery` with spec-compliant struct; convert to `MessageQueryParams` in handler |
| `crates/server/src/main.rs` | Add `--contains`, `--offset`, `--sidechain`/`--no-sidechain`, `--min-input-tokens` to Query subcommand; update `run_query()` to build `MessageQueryParams` |
| `crates/server/src/daemon_client.rs` | Update `query_messages()` method to use new request body shape |

---

## Severity Summary

| Severity | Count | Deviation IDs |
|----------|-------|---------------|
| CRITICAL | 3 | DEV-01, DEV-05, DEV-11 |
| MODERATE | 4 | DEV-02, DEV-03, DEV-04, DEV-06, DEV-07 |
| MINOR | 4 | DEV-08, DEV-09, DEV-10, DEV-12 |

The critical deviations (DEV-01, DEV-05, DEV-11) represent the gap between the spec's intent of a composable query compiler and the implementation's fixed-parameter function approach. Steps 1-3 of the refactoring plan resolve all critical and moderate deviations in a single cohesive change.
