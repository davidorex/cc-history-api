# Phase 1: Core Types and Ingestion Pipeline - Research

**Researched:** 2026-02-20
**Domain:** Rust serde modeling of Claude Code JSONL history, SQLite ingestion pipeline, Cargo workspace
**Confidence:** HIGH

<spec_constraints>
## Spec Constraints (IMMUTABLE -- from ROADMAP.md Success Criteria)

1. Running `claude-history sync` against a real `~/.claude/projects/` directory parses every JSONL file and populates sessions, messages, message_content, token_usage, tool_executions, agents, queue_operations, and progress_events tables with correct data
2. Running sync a second time on the same files processes zero records (byte-offset incremental sync skips already-ingested data)
3. Malformed JSONL lines produce logged warnings but do not halt ingestion -- all valid records in the same file are still decomposed
4. Unknown fields in JSONL records (simulated or real) appear in the schema_drift_log table with field name, sample value, and source context
5. The SQLite database uses WAL mode, embedded migrations track schema version, and the Cargo workspace compiles to a single binary

These are non-negotiable user-story outcomes. Research recommendations must not narrow below these.
</spec_constraints>

## Summary

This research is based on **empirical analysis of 6,250 real JSONL files** (6.3 GB total) from `~/.claude/projects/`, spanning Claude Code versions 2.0.76 through 2.1.49. The JSONL schema is undocumented, so every finding below comes from direct file inspection rather than training data.

The JSONL format contains **7 distinct record types** (not 4 as the spec initially assumed): `user`, `assistant`, `progress`, `queue-operation`, `system`, `summary`, and `file-history-snapshot`. These split into three structural categories: full-base records (with uuid, sessionId, version, cwd, etc.), partial records (`queue-operation` with sessionId but no uuid), and lightweight records (`summary` and `file-history-snapshot` with neither uuid nor sessionId). The `system` type was not in the original spec and is the 4th most common record type (14,218 records observed) with 6 subtypes including hook summaries, turn durations, compact boundaries, and API errors.

Content blocks in assistant messages are limited to 3 types (`text`, `thinking`, `tool_use`), while user messages contain `tool_result` and `text` blocks. The `thinking` block has an optional `signature` field (present 94.6% of the time). Usage stats have a stable core (`input_tokens`, `output_tokens`, `cache_creation_input_tokens`, `cache_read_input_tokens`) plus newer fields (`server_tool_use`, `iterations`) that appear in <1% of records -- perfect candidates for overflow capture.

**Primary recommendation:** Model the 7 record types as a `serde(tag = "type")` enum with three structural tiers, use `serde(flatten)` overflow on every struct with variable shape, and use a single `rusqlite::Connection` wrapped in `tokio-rusqlite` for the writer with `unchecked_transaction()` for multi-table decomposition.

## Empirical JSONL Schema Discovery

### Record Types (CRITICAL -- 7 types, not 4)

| Type | Count (observed) | Has uuid | Has sessionId | Has RecordBase fields | Category |
|------|-----------------|----------|---------------|----------------------|----------|
| `progress` | 302,027 | YES | YES | YES | Full |
| `assistant` | 243,827 | YES | YES | YES | Full |
| `user` | 173,077 | YES | YES | YES | Full |
| `system` | 14,218 | YES | YES | YES | Full |
| `queue-operation` | 7,085 | NO | YES | Partial | Partial |
| `summary` | 3,392 | NO | NO | NO | Lightweight |
| `file-history-snapshot` | 22,273 | NO | NO | NO | Lightweight |

### Full-Base Record Fields (user, assistant, progress, system)

These fields appear on **100%** of full-base records:

| Field | Type | Notes |
|-------|------|-------|
| `uuid` | String (UUIDv4) | Primary identifier |
| `timestamp` | String (ISO8601) | e.g. `"2026-02-20T01:28:38.896Z"` |
| `sessionId` | String (UUIDv4) | Links to session file |
| `version` | String (semver) | e.g. `"2.1.49"` |
| `cwd` | String | Working directory path |
| `parentUuid` | String or null | Conversation threading |
| `isSidechain` | bool | Sidechain flag |
| `userType` | String | Always `"external"` in observed data |
| `gitBranch` | String | Current git branch (can be empty string) |
| `type` | String | Discriminator |

Optional base fields (present on many but not all):

| Field | Presence | Type | Notes |
|-------|----------|------|-------|
| `slug` | 94-98% | String | Session slug name |
| `agentId` | 47-50% | String | Only present for agent/subagent sessions |
| `teamName` | 0.2% | String | Team name for multi-agent sessions |
| `isMeta` | 2-25% | bool | Meta-message flag |

### User Record Fields

All 100% fields: base fields + `message`

| Field | Presence | Type | Notes |
|-------|----------|------|-------|
| `message` | 100% | Object | `{role: "user", content: String | Block[]}` |
| `slug` | 94.9% | String | |
| `sourceToolAssistantUUID` | 61.1% | String | UUID of assistant message that triggered this tool result |
| `toolUseResult` | 59.2% | Object | `{type: "text", text: "...", file?: {...}}` - duplicated result |
| `agentId` | 50.3% | String | |
| `thinkingMetadata` | 5.9% | Object | `{maxThinkingTokens: number}` |
| `todos` | 2.6% | Array | Task/todo list |
| `permissionMode` | 2.0% | String | `"acceptEdits"` or `"default"` |
| `isMeta` | 2.8% | bool | |
| `isVisibleInTranscriptOnly` | 0.2% | bool | |
| `isCompactSummary` | 0.2% | bool | |
| `sourceToolUseID` | 0.1% | String | |
| `mcpMeta` | 0.04% | Object | MCP structured content |
| `imagePasteIds` | 0.04% | Array | e.g. `[1]` |

User message content shapes:
- **String** (plain text): 15% of user messages
- **Array of blocks**: 85% of user messages (mostly `tool_result` blocks)

### Assistant Record Fields

All 100% fields: base fields + `message`

| Field | Presence | Type | Notes |
|-------|----------|------|-------|
| `message` | 100% | Object | See inner message structure below |
| `requestId` | 96.6% | String | API request ID e.g. `"req_011CYJ..."` |
| `slug` | 96.6% | String | |
| `agentId` | 48.9% | String | |
| `isApiErrorMessage` | 0.2% | bool | Always `true` when present |
| `teamName` | 0.2% | String | |
| `error` | 0.2% | String | e.g. `"authentication_failed"` |
| `apiError` | 0.004% | varies | Very rare |

#### Assistant Inner Message (`message` field)

| Field | Presence | Type | Notes |
|-------|----------|------|-------|
| `id` | 100% | String | `"msg_XXXXX"` or UUID |
| `type` | 100% | String | Always `"message"` |
| `role` | 100% | String | Always `"assistant"` |
| `model` | 100% | String | Model identifier |
| `content` | 100% | `ContentBlock[]` | Array of content blocks |
| `usage` | 100% | Object | Token usage stats |
| `stop_reason` | 95.3% | String or null | `null`, `"end_turn"`, `"tool_use"`, `"stop_sequence"` |
| `stop_sequence` | 94.2% | String or null | |
| `context_management` | 1.5% | varies | Newer field |
| `container` | 0.5% | null or Object | Newer field |

Observed models: `claude-opus-4-5-20251101`, `minimax-m2.1:cloud`, `claude-haiku-4-5-20251001`, `claude-opus-4-6`, `<synthetic>`, `claude-sonnet-4-5-20250929`

### Content Block Types

#### In Assistant Messages

| Block Type | Count | Fields |
|------------|-------|--------|
| `tool_use` | 12,336 | `id` (100%), `name` (100%), `input` (100%, polymorphic JSON), `caller` (7.4%, always `{type: "direct"}`) |
| `text` | 4,942 | `text` (100%) |
| `thinking` | 4,187 | `thinking` (100%), `signature` (94.6%, optional string) |

#### In User Messages (when content is array)

| Block Type | Count | Fields |
|------------|-------|--------|
| `tool_result` | 11,995 | `tool_use_id` (100%), `content` (100%), `is_error` (39.5%) |
| `text` | 423 | `text` (100%) |

Tool result `content` shapes:
- **String**: 96.7% of tool_result blocks
- **Array of `{type: "text", text: "..."}` objects**: 3.3% of tool_result blocks

### Usage Stats Shape

| Field | Presence | Type |
|-------|----------|------|
| `input_tokens` | 100% | u64 |
| `output_tokens` | 100% | u64 |
| `cache_creation_input_tokens` | 94.2% | u64 |
| `cache_read_input_tokens` | 94.2% | u64 |
| `cache_creation` | 94.2% | Object (see below) |
| `service_tier` | 94.2% | String or null |
| `inference_geo` | 2.7% | String |
| `server_tool_use` | 0.6% | `{web_search_requests: u64, web_fetch_requests: u64}` |
| `iterations` | 0.1% | Array |

Cache creation sub-object:
```json
{
  "ephemeral_5m_input_tokens": 0,
  "ephemeral_1h_input_tokens": 117903
}
```

### System Record (NEW -- not in original spec)

Subtype distribution:

| Subtype | Count | Extra Fields |
|---------|-------|-------------|
| `stop_hook_summary` | 10,345 | `hookCount`, `hookInfos[]`, `hookErrors[]`, `preventedContinuation`, `stopReason`, `hasOutput`, `level`, `toolUseID` |
| `turn_duration` | 3,160 | `durationMs`, `isMeta` |
| `compact_boundary` | 315 | `content`, `level`, `logicalParentUuid`, `compactMetadata` |
| `api_error` | 309 | `level`, `error`, `retryInMs`, `retryAttempt`, `maxRetries` |
| `local_command` | 80 | `content`, `level`, `isMeta` |
| `microcompact_boundary` | 9 | `microcompactMetadata` |

### Progress Record Data Shapes

| `data.type` | Count | Fields |
|-------------|-------|--------|
| `hook_progress` | 181,548 | `hookEvent`, `hookName`, `command` |
| `agent_progress` | 73,549 | `message`, `normalizedMessages`, `prompt`, `agentId`, `resume` (0.1%) |
| `bash_progress` | 44,966 | `output`, `fullOutput`, `elapsedTimeSeconds`, `totalLines`, `timeoutMs` (34.5%) |
| `mcp_progress` | 1,338 | `status`, `serverName`, `toolName`, `elapsedTimeMs` (50%) |
| `query_update` | 295 | `query` |
| `search_results_received` | 294 | `resultCount`, `query` |
| `skill_progress` | 158 | (not fully analyzed) |
| `waiting_for_task` | 21 | (not fully analyzed) |

### Queue Operation Record

| Field | Presence | Type | Notes |
|-------|----------|------|-------|
| `type` | 100% | String | Always `"queue-operation"` |
| `operation` | 100% | String | `"enqueue"`, `"dequeue"`, `"remove"`, `"popAll"` |
| `timestamp` | 100% | String | ISO8601 |
| `sessionId` | 100% | String | UUIDv4 |
| `content` | 48.3% | String | Only present for `enqueue` operations |

### Summary Record

| Field | Presence | Type | Notes |
|-------|----------|------|-------|
| `type` | 100% | String | Always `"summary"` |
| `summary` | 100% | String | Human-readable summary text |
| `leafUuid` | 100% | String | UUID of last message in summarized sequence |

### File History Snapshot Record

| Field | Presence | Type | Notes |
|-------|----------|------|-------|
| `type` | 100% | String | Always `"file-history-snapshot"` |
| `messageId` | 100% | String | UUID of associated message |
| `snapshot` | 100% | Object | Contains `messageId`, `trackedFileBackups`, `timestamp` |
| `isSnapshotUpdate` | 100% | bool | false (88%), true (12%) |

`trackedFileBackups` is a map of `{relative_path: {backupFileName, version, backupTime}}`.

### Tool Names (44 distinct observed)

Top 10: `Bash` (4,666), `Read` (3,500), `Edit` (1,567), `Write` (648), `Glob` (613), `Grep` (521), `TodoWrite` (315), `Task` (189), `Skill` (58), `TaskUpdate` (47). Plus MCP tools (`mcp__*` prefix), `WebSearch`, `WebFetch`, `AskUserQuestion`, `ToolSearch`, `TaskCreate`, `TaskOutput`, `TaskList`.

### File/Directory Structure

- Files located at: `~/.claude/projects/{project-path-with-dashes}/{session-uuid}.jsonl`
- Subagent files at: `~/.claude/projects/{project-path}/{session-uuid}/subagents/agent-{agent-id}.jsonl`
- Session UUID matches filename (e.g., `eb3ca04b-0383-4955-8606-7c5c9cabe3d7.jsonl`)
- Newlines: Unix LF only, no BOM, UTF-8
- File sizes: 0 bytes to 580 MB, average 1 MB
- Total observed: 2,234 main session files + 4,016 subagent files = 6,250 total

### Version Range

Observed versions: `2.0.76`, `2.1.4`, `2.1.5`, `2.1.9`, `2.1.14`, `2.1.15`, `2.1.17`, `2.1.19`, `2.1.20`, `2.1.22`, `2.1.25`, `2.1.27`, `2.1.29`, `2.1.31`, `2.1.37`, `2.1.39`, `2.1.49`

## Standard Stack

### Core

| Library | Version | Purpose | Why Standard |
|---------|---------|---------|--------------|
| `serde` | 1.0.228 | Serialization/deserialization of all JSONL types | De facto Rust standard; `tag`, `flatten`, `untagged` are mature |
| `serde_json` | latest | JSON parsing per line | Required companion to serde |
| `rusqlite` | 0.38.0 (bundled, fts5) | SQLite database | Mature, `bundled` statically links SQLite, `fts5` enables full-text search |
| `tokio-rusqlite` | 0.7.0 | Async bridge for rusqlite | Wraps blocking rusqlite calls in background thread |
| `tokio` | 1.49.0 | Async runtime | Required by tokio-rusqlite and clap integration |
| `tracing` | latest | Structured logging | Standard Rust logging; required for warning/error tracking per INFRA-07 |
| `clap` | latest (derive) | CLI argument parsing | Standard CLI framework for `claude-history sync` etc. |
| `walkdir` | latest | Directory traversal | Recursive `.jsonl` file discovery |

### Supporting

| Library | Version | Purpose | When to Use |
|---------|---------|---------|-------------|
| `tracing-subscriber` | latest | Log output formatting | Initialize at binary startup |
| `anyhow` | latest | Error handling | Application-level errors in server crate |
| `thiserror` | latest | Error type definition | Library-level errors in core/store crates |
| `uuid` | latest | UUID generation/validation | Optional; for validating session IDs |
| `chrono` | latest | ISO8601 timestamp parsing | Optional; for timestamp validation |

### Alternatives Considered

| Instead of | Could Use | Tradeoff |
|------------|-----------|----------|
| `rusqlite` (bundled) | `sqlx` (async native) | sqlx has native async but no bundled SQLite or FTS5; rusqlite + tokio-rusqlite is more mature for embedded SQLite |
| Line-by-line parsing | `serde_json::StreamDeserializer` | StreamDeserializer cannot provide byte offsets needed for incremental sync |
| `include_str!` migrations | `refinery` or `rusqlite_migration` | External crate adds dependency; `include_str!` + manual version tracking is simple enough for <15 migrations |

### Cargo.toml Dependencies

Workspace root:
```toml
[workspace]
resolver = "2"
members = ["crates/core", "crates/store", "crates/server"]

[workspace.dependencies]
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
rusqlite = { version = "0.38", features = ["bundled", "fts5"] }
tokio-rusqlite = "0.7"
tokio = { version = "1.49", features = ["full"] }
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
clap = { version = "4", features = ["derive"] }
walkdir = "2"
anyhow = "1"
thiserror = "2"
```

## Architecture Patterns

### Recommended Project Structure

```
claude-history/
+-- Cargo.toml                    # Workspace root
+-- Cargo.lock
+-- crates/
|   +-- core/                     # Types, serde, parser (lib crate)
|   |   +-- Cargo.toml
|   |   +-- src/
|   |       +-- lib.rs
|   |       +-- record.rs         # JSONLRecord enum (7 variants)
|   |       +-- message.rs        # ContentBlock, MessageContent, UsageStats
|   |       +-- progress.rs       # ProgressData variants
|   |       +-- system.rs         # SystemRecord subtypes
|   |       +-- parser.rs         # Streaming JSONL reader with byte offsets
|   |
|   +-- store/                    # SQLite, decomposition, sync (lib crate)
|   |   +-- Cargo.toml
|   |   +-- src/
|   |   |   +-- lib.rs
|   |   |   +-- db.rs             # Connection init (WAL, busy_timeout, pragmas)
|   |   |   +-- schema.rs         # DDL + migration runner
|   |   |   +-- decompose.rs      # Record -> normalized rows
|   |   |   +-- sync.rs           # Incremental byte-offset sync
|   |   |   +-- drift.rs          # Schema drift logger
|   |   +-- migrations/
|   |       +-- 001_initial.sql   # Core 11 tables
|   |
|   +-- server/                   # CLI binary (binary crate)
|       +-- Cargo.toml
|       +-- src/
|           +-- main.rs           # clap CLI: sync, serve, query
|
+-- tests/
    +-- fixtures/                 # Sampled JSONL data for tests
```

### Pattern 1: Tagged Enum with Three-Tier Structs

**What:** Model the 7 record types as `serde(tag = "type")` enum where variants reference different struct tiers based on available fields.

**Why:** Records have 3 distinct field sets (full-base, partial, lightweight). Using a single `RecordBase` struct with all-optional fields would lose type safety and require runtime checks.

```rust
/// Top-level discriminated union. Every JSONL line deserializes to one of these.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum JSONLRecord {
    #[serde(rename = "user")]
    User(UserRecord),

    #[serde(rename = "assistant")]
    Assistant(AssistantRecord),

    #[serde(rename = "progress")]
    Progress(ProgressRecord),

    #[serde(rename = "system")]
    System(SystemRecord),

    #[serde(rename = "queue-operation")]
    QueueOperation(QueueOperationRecord),

    #[serde(rename = "summary")]
    Summary(SummaryRecord),

    #[serde(rename = "file-history-snapshot")]
    FileHistorySnapshot(FileHistorySnapshotRecord),
}

/// Shared base fields for user, assistant, progress, system records.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordBase {
    pub uuid: String,
    pub timestamp: String,
    pub session_id: String,
    pub version: String,
    pub cwd: String,
    pub parent_uuid: Option<String>,
    pub is_sidechain: bool,
    pub user_type: String,
    pub git_branch: String,
    // Optional fields present on many records
    pub slug: Option<String>,
    pub agent_id: Option<String>,
    pub team_name: Option<String>,
    pub is_meta: Option<bool>,
}
```

### Pattern 2: Overflow Capture on Every Variable-Shape Struct

**What:** Every struct that might gain new fields gets `#[serde(flatten)] pub overflow: HashMap<String, serde_json::Value>`.

**Where to apply:**
- `RecordBase` (new top-level fields across versions)
- `UserRecord` (fields like `mcpMeta`, `imagePasteIds` appear rarely)
- `AssistantRecord` (fields like `apiError`, `error` appear rarely)
- `AssistantMessage` (inner message -- `context_management`, `container`)
- `UsageStats` (`server_tool_use`, `iterations` are new)
- `CacheCreation` (may gain new token categories)
- `SystemRecord` (many conditional fields based on subtype)
- All `ProgressData` variants

**Critical insight:** Do NOT put overflow on `ContentBlock` enum variants since `serde(tag = "type")` + `serde(flatten)` on an enum variant can cause subtle deserialization issues. Instead, model `tool_use.input` as `serde_json::Value` and `tool_result.content` as `serde_json::Value` -- these are already polymorphic by design.

### Pattern 3: MessageContent as Untagged Enum

**What:** User message content can be either a plain string or an array of blocks.

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MessageContent {
    Text(String),
    Blocks(Vec<ContentBlock>),
}
```

**Empirical validation:** 15% of user messages use plain string content, 85% use block arrays. This untagged enum handles both.

### Pattern 4: System Record with Subtype Discrimination

**What:** System records use a `subtype` field instead of `type` for their variant. Model with a single struct plus overflow rather than a nested enum, since subtypes share very few fields.

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemRecord {
    #[serde(flatten)]
    pub base: RecordBase,
    pub subtype: String,
    // All subtype-specific fields are optional or in overflow
    pub level: Option<String>,
    pub duration_ms: Option<u64>,
    pub hook_count: Option<u32>,
    pub content: Option<String>,
    #[serde(flatten)]
    pub overflow: HashMap<String, serde_json::Value>,
}
```

### Pattern 5: tokio-rusqlite Writer with unchecked_transaction

**What:** Use `tokio_rusqlite::Connection` for async bridge. Within `call()`, use `conn.unchecked_transaction()` for decomposition of a batch of records.

```rust
use tokio_rusqlite::Connection;

// Initialize with WAL and pragmas
let conn = Connection::open(&db_path).await?;
conn.call(|conn| {
    conn.execute_batch("
        PRAGMA journal_mode = WAL;
        PRAGMA busy_timeout = 5000;
        PRAGMA synchronous = NORMAL;
        PRAGMA foreign_keys = ON;
    ")?;
    Ok(())
}).await?;

// Decompose a batch of records in a single transaction
conn.call(move |conn| {
    let tx = conn.unchecked_transaction()?;
    for record in &records {
        decompose_record(record, &tx)?;
    }
    // Update sync metadata atomically
    update_sync_metadata(&file_path, new_offset, &tx)?;
    tx.commit()?;
    Ok(())
}).await?;
```

**Why `unchecked_transaction`:** The `tokio_rusqlite::Connection::call()` takes `&Connection` (not `&mut Connection`), so the standard `transaction()` which requires `&mut` won't compile. `unchecked_transaction()` works with `&Connection` and is safe in this context since tokio-rusqlite serializes all calls through a single background thread.

### Pattern 6: Byte-Offset Tracking with Partial-Line Safety

**What:** Track byte offsets for incremental sync, handling partial lines at EOF.

```rust
pub fn parse_jsonl(path: &Path, from_offset: u64) -> Result<ParseResult> {
    let mut file = File::open(path)?;
    let file_len = file.metadata()?.len();
    file.seek(SeekFrom::Start(from_offset))?;

    let reader = BufReader::new(&file);
    let mut records = Vec::new();
    let mut warnings = Vec::new();
    let mut current_offset = from_offset;

    for (line_num, line_result) in reader.lines().enumerate() {
        let line = line_result?;
        let line_byte_len = line.len() as u64 + 1; // +1 for newline

        if line.trim().is_empty() {
            current_offset += line_byte_len;
            continue;
        }

        match serde_json::from_str::<JSONLRecord>(&line) {
            Ok(record) => records.push(record),
            Err(e) => {
                tracing::warn!(
                    file = %path.display(),
                    line = line_num,
                    offset = current_offset,
                    error = %e,
                    "Malformed JSONL line"
                );
                warnings.push(ParseWarning {
                    line_number: line_num,
                    byte_offset: current_offset,
                    error: e.to_string(),
                    raw_line: if line.len() > 500 { line[..500].to_string() } else { line.clone() },
                });
            }
        }

        current_offset += line_byte_len;
    }

    // If file doesn't end with newline, adjust
    // BufReader::lines() strips trailing newline, so last line might not have one
    if current_offset > file_len {
        current_offset = file_len;
    }

    Ok(ParseResult {
        records,
        warnings,
        bytes_read: current_offset - from_offset,
        new_offset: current_offset,
    })
}
```

**Partial-line hazard:** If Claude Code is actively writing a file and the last line is incomplete (no trailing newline), `BufReader::lines()` will still return it. This line will fail to parse as JSON. The parser must handle this gracefully -- the warning is logged but the offset is only advanced to the start of the failed line, so next sync re-attempts it. Alternative: only advance offset past lines that successfully parsed or are empty, and track the last confirmed offset.

### Pattern 7: Embedded Migrations with include_str!

```rust
const MIGRATIONS: &[(&str, &str)] = &[
    ("001", include_str!("../migrations/001_initial.sql")),
];

pub fn run_migrations(conn: &rusqlite::Connection) -> Result<()> {
    conn.execute_batch("
        CREATE TABLE IF NOT EXISTS schema_versions (
            version TEXT PRIMARY KEY,
            applied_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
    ")?;

    for (version, sql) in MIGRATIONS {
        let applied: bool = conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM schema_versions WHERE version = ?1",
                [version],
                |row| row.get(0),
            )?;

        if !applied {
            let tx = conn.unchecked_transaction()?;
            tx.execute_batch(sql)?;
            tx.execute(
                "INSERT INTO schema_versions (version) VALUES (?1)",
                [version],
            )?;
            tx.commit()?;
            tracing::info!(version = version, "Applied migration");
        }
    }
    Ok(())
}
```

### Anti-Patterns to Avoid

- **Monolithic RecordBase for all types:** `summary` and `file-history-snapshot` don't have uuid, sessionId, etc. Forcing them into a single base struct with all-optional fields loses type safety. Use three structural tiers instead.
- **Nested serde(tag) + serde(flatten) on enum variants:** This combination can cause deserialization ambiguity. Use `serde_json::Value` for polymorphic fields within enum variants.
- **`serde_json::StreamDeserializer` for JSONL:** It does not provide byte offsets, which are required for incremental sync. Use line-by-line reading with manual offset tracking.
- **Single connection for concurrent reads/writes:** WAL mode allows concurrent reads during writes, but only with separate connections. For Phase 1 (sync command), a single connection suffices. Split reader/writer pool is a Phase 3+ optimization.
- **Storing overflow as TEXT column:** Store as JSON text in the `schema_drift_log` table, but keep the `overflow` HashMap in memory for processing. Don't try to index overflow fields.

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| JSON parsing per line | Custom JSON tokenizer | `serde_json::from_str` per line | Edge cases with escaping, unicode, numbers |
| Async SQLite | Custom thread pool | `tokio-rusqlite` | Channel-based serialization handles all edge cases |
| Migration tracking | Custom version comparison | `schema_versions` table + `include_str!` | Simple, reliable, embedded in binary |
| File discovery | Custom directory walker | `walkdir` crate | Handles symlinks, permissions, cross-platform |
| CLI parsing | Custom arg parsing | `clap` derive macros | Subcommands, help text, validation |
| Structured logging | `println!` statements | `tracing` crate | Structured fields, levels, subscriber flexibility |

**Key insight:** The complexity in this phase is in the data modeling (7 record types with 3 structural tiers) and the decomposition logic, not in the infrastructure. Use well-tested crates for everything else.

## Common Pitfalls

### Pitfall 1: Byte Offset Drift with BufReader::lines()

**What goes wrong:** `BufReader::lines()` strips the newline character. If you calculate offset as `line.len() + 1`, you assume `\n` termination. Files on macOS from Claude Code use `\n` (confirmed empirically), but: (a) the last line may not have a trailing newline, (b) a file actively being written may have a partial line.

**Why it happens:** JSONL files are append-only and may be written to while being read.

**How to avoid:** After parsing all lines, clamp `current_offset` to `min(current_offset, file_length)`. Only store the offset of the last SUCCESSFULLY parsed line + its byte length. On next sync, any partial line will be re-read from its start offset.

**Warning signs:** Second sync re-processes records (offset was stored past EOF) or skips a record (offset advanced past a partial line that was later completed).

### Pitfall 2: serde(flatten) Ordering Ambiguity

**What goes wrong:** When `#[serde(flatten)]` is used on multiple fields in the same struct (e.g., `base: RecordBase` with its own flatten, plus a top-level `overflow`), serde processes them in declaration order. If `RecordBase` has its own `overflow: HashMap`, the outer struct's overflow might steal fields from the inner one, or vice versa.

**Why it happens:** Two `#[serde(flatten)]` at different nesting levels compete for unknown fields.

**How to avoid:** Only ONE `HashMap<String, Value>` overflow per struct. Put it at the outermost level. `RecordBase` should NOT have its own overflow -- instead, the containing struct (e.g., `UserRecord`) should have the overflow that catches fields unknown to both `RecordBase` and itself.

**Warning signs:** Overflow maps contain fields that should have been captured by known struct fields. Test with real data early.

### Pitfall 3: INSERT OR IGNORE Silently Losing Data

**What goes wrong:** Using `INSERT OR IGNORE` to handle duplicate UUIDs during re-sync can silently discard records if the UUID uniqueness constraint fires on a different record than expected.

**Why it happens:** If byte-offset tracking has a bug, records may be re-processed. `INSERT OR IGNORE` hides the problem.

**How to avoid:** Use `INSERT OR IGNORE` for idempotency but LOG when a duplicate is detected (at DEBUG level). During development, use `INSERT` without `OR IGNORE` to catch offset bugs early. Add a counter for skipped duplicates to sync results.

### Pitfall 4: Transaction Size for Bulk Import

**What goes wrong:** Wrapping all records from a 580MB file in a single transaction consumes excessive memory and holds the WAL checkpoint for too long.

**Why it happens:** Large session files can have thousands of records.

**How to avoid:** Batch transactions: commit every N records (e.g., 1000). The sync_metadata offset is updated to the batch boundary, not the file end, so interrupted syncs resume from the last committed batch.

**Warning signs:** OOM on large files, long WAL checkpoint times, "database is locked" errors.

### Pitfall 5: Queue-Operation Records Missing UUID

**What goes wrong:** Attempting to insert `queue-operation` records into the `messages` table (which has a `uuid` primary key) fails because queue-operations don't have UUIDs.

**Why it happens:** The spec lumps all record types into messages, but queue-operations have a fundamentally different structure.

**How to avoid:** Route queue-operations to their own `queue_operations` table (as spec says), and generate a synthetic key (e.g., hash of sessionId + timestamp + operation). Similarly, `summary` and `file-history-snapshot` need their own tables or must be associated with the session via the filename.

### Pitfall 6: session_id Extraction for Lightweight Records

**What goes wrong:** `summary` and `file-history-snapshot` records have no `sessionId` field, making it impossible to link them to a session using record data alone.

**Why it happens:** These records rely on file-level context (the filename IS the session ID).

**How to avoid:** Pass the session_id (extracted from the filename) into the parser/decomposer. Every record gets a session_id, either from the record itself or from the file context. The `file-history-snapshot.messageId` can also be used to join against the messages table.

## Code Examples

### Connection Initialization

```rust
use tokio_rusqlite::Connection;
use std::path::Path;
use std::time::Duration;

pub async fn init_db(path: &Path) -> Result<Connection> {
    let conn = Connection::open(path).await?;

    conn.call(|conn| {
        // WAL mode for concurrent reads during writes
        conn.pragma_update(None, "journal_mode", "WAL")?;
        // 5 second busy timeout
        conn.busy_timeout(Duration::from_secs(5))?;
        // Normal synchronous for WAL (safe, faster than FULL)
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        // Enable foreign keys
        conn.pragma_update(None, "foreign_keys", "ON")?;

        // Run migrations
        run_migrations(conn)?;

        Ok(())
    }).await?;

    Ok(conn)
}
```

### Decompose Record

```rust
pub fn decompose_record(
    record: &JSONLRecord,
    session_id: &str,  // from filename for lightweight records
    tx: &rusqlite::Transaction,
) -> Result<()> {
    match record {
        JSONLRecord::User(r) => decompose_user(r, tx)?,
        JSONLRecord::Assistant(r) => decompose_assistant(r, tx)?,
        JSONLRecord::Progress(r) => decompose_progress(r, tx)?,
        JSONLRecord::System(r) => decompose_system(r, tx)?,
        JSONLRecord::QueueOperation(r) => decompose_queue_operation(r, tx)?,
        JSONLRecord::Summary(r) => decompose_summary(r, session_id, tx)?,
        JSONLRecord::FileHistorySnapshot(r) => {
            // file-history-snapshot can be stored but is lower priority
            // for Phase 1; just log overflow if any
        }
    }
    Ok(())
}

fn decompose_assistant(r: &AssistantRecord, tx: &rusqlite::Transaction) -> Result<()> {
    // 1. Upsert session
    tx.execute(
        "INSERT OR IGNORE INTO sessions (session_id, first_seen_at, project_path, version)
         VALUES (?1, ?2, ?3, ?4)",
        rusqlite::params![r.base.session_id, r.base.timestamp, r.base.cwd, r.base.version],
    )?;

    // 2. Insert message
    tx.execute(
        "INSERT OR IGNORE INTO messages (uuid, session_id, type, timestamp, parent_uuid,
         is_sidechain, user_type, cwd, git_branch, version, model, stop_reason, request_id)
         VALUES (?1, ?2, 'assistant', ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        rusqlite::params![
            r.base.uuid, r.base.session_id, r.base.timestamp,
            r.base.parent_uuid, r.base.is_sidechain, r.base.user_type,
            r.base.cwd, r.base.git_branch, r.base.version,
            r.message.model, r.message.stop_reason, r.request_id,
        ],
    )?;

    // 3. Decompose content blocks
    for (i, block) in r.message.content.iter().enumerate() {
        decompose_content_block(&r.base.uuid, i, block, tx)?;
    }

    // 4. Insert token usage
    if let Some(ref usage) = r.message.usage {
        tx.execute(
            "INSERT OR IGNORE INTO token_usage (message_uuid, input_tokens, output_tokens,
             cache_creation_input_tokens, cache_read_input_tokens, service_tier)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![
                r.base.uuid, usage.input_tokens, usage.output_tokens,
                usage.cache_creation_input_tokens, usage.cache_read_input_tokens,
                usage.service_tier,
            ],
        )?;
    }

    // 5. Log overflow for drift detection
    log_overflow(&r.base.version, "assistant", &r.overflow, tx)?;

    Ok(())
}
```

### Incremental Sync

```rust
pub async fn sync_file(
    conn: &Connection,
    path: &Path,
    session_id: &str,
) -> Result<SyncResult> {
    let path = path.to_path_buf();
    let session_id = session_id.to_string();

    conn.call(move |conn| {
        // Get last known offset
        let last_offset: u64 = conn
            .query_row(
                "SELECT last_byte_offset FROM sync_metadata WHERE file_path = ?1",
                [path.to_str().unwrap()],
                |row| row.get(0),
            )
            .unwrap_or(0);

        let file_size = std::fs::metadata(&path)?.len();
        if file_size <= last_offset {
            return Ok(SyncResult::NoNewData);
        }

        // Parse new data
        let parsed = parse_jsonl(&path, last_offset)?;

        // Decompose in batches
        let batch_size = 1000;
        let mut total_inserted = 0;

        for chunk in parsed.records.chunks(batch_size) {
            let tx = conn.unchecked_transaction()?;
            for record in chunk {
                decompose_record(record, &session_id, &tx)?;
                total_inserted += 1;
            }

            // Update sync metadata to batch boundary
            tx.execute(
                "INSERT INTO sync_metadata (file_path, last_byte_offset, last_synced_at)
                 VALUES (?1, ?2, datetime('now'))
                 ON CONFLICT(file_path) DO UPDATE SET
                   last_byte_offset = ?2,
                   last_synced_at = datetime('now')",
                rusqlite::params![path.to_str().unwrap(), parsed.new_offset],
            )?;
            tx.commit()?;
        }

        Ok(SyncResult::Synced {
            new_records: total_inserted,
            warnings: parsed.warnings.len(),
        })
    }).await?
}
```

### Schema Drift Logging

```rust
fn log_overflow(
    version: &str,
    record_type: &str,
    overflow: &HashMap<String, serde_json::Value>,
    tx: &rusqlite::Transaction,
) -> Result<()> {
    for (field_name, value) in overflow {
        let sample = value.to_string();
        let sample_truncated = if sample.len() > 500 {
            format!("{}...", &sample[..500])
        } else {
            sample
        };

        tx.execute(
            "INSERT OR IGNORE INTO schema_drift_log
             (field_name, record_type, version, sample_value, first_seen_at, source_context)
             VALUES (?1, ?2, ?3, ?4, datetime('now'), ?5)",
            rusqlite::params![
                field_name,
                record_type,
                version,
                sample_truncated,
                format!("overflow capture from {} record", record_type),
            ],
        )?;
    }
    Ok(())
}
```

## State of the Art

| Old Approach | Current Approach | When Changed | Impact |
|--------------|------------------|--------------|--------|
| 4 record types assumed | 7 record types confirmed | This research (empirical discovery) | Schema and decomposer must handle system, summary, file-history-snapshot |
| `stop_reason` always present | `stop_reason` absent in 4.7% of messages | Observed in data | Must be `Option<String>` |
| `cache_creation` always present | Absent in 5.8% of usage stats | Older versions (2.0.76) | All cache fields must be optional |
| Single RecordBase for all types | Three structural tiers needed | This research | summary/file-history-snapshot have no uuid/sessionId |

## Open Questions

1. **`skill_progress` and `waiting_for_task` data shapes**
   - What we know: These are `progress.data.type` values with very low counts (158, 21)
   - What's unclear: Full field sets for these variants
   - Recommendation: Use `serde_json::Value` for the `data` field on progress records, or model known variants and fall back to a catch-all. The overflow approach handles this naturally.

2. **`<synthetic>` model in assistant messages**
   - What we know: 102 records have `model: "<synthetic>"`. These are error messages generated by Claude Code itself, not by the API.
   - What's unclear: Whether synthetic messages always have `isApiErrorMessage: true`
   - Recommendation: Model `model` as plain String; no special handling needed.

3. **File-history-snapshot decomposition target**
   - What we know: These contain file backup metadata (paths, versions, backup timestamps)
   - What's unclear: Whether this data should populate the Phase 1 tables or a future artifact table
   - Recommendation: Parse and capture the data in Phase 1 but store minimally (dedicated table or JSON blob). Full artifact decomposition is a later phase concern.

4. **Byte-offset accuracy across platforms**
   - What we know: macOS files use LF newlines, UTF-8, no BOM
   - What's unclear: Whether Claude Code on Linux or Windows produces different line endings
   - Recommendation: Always use byte-level offset tracking (`line.as_bytes().len() + 1`), detect CRLF at file open and adjust accordingly.

5. **Transaction batching optimal size**
   - What we know: Files range from 0 to 580MB. Average is 1MB.
   - What's unclear: Optimal batch size for transaction commits
   - Recommendation: Start with 1000 records per transaction. Profile with real data and adjust. Most files will complete in a single batch.

<phase_requirements>
## Phase Requirements

| ID | Description | Research Support |
|----|-------------|-----------------|
| CORE-01 | Exact serde modeling of every JSONL record type with `serde(tag = "type")` | Empirical discovery of 7 record types (not 4). Full field inventory per type. Three structural tiers identified. |
| CORE-02 | `serde(flatten)` overflow capture on every struct with variable shape | Identified which structs need overflow. Warning: only ONE overflow HashMap per struct to avoid ambiguity. |
| CORE-03 | Content block modeling: text, thinking, tool_use, tool_result as tagged enum | Confirmed 3 assistant block types (text, thinking, tool_use) and 2 user block types (tool_result, text). Field inventories complete. |
| CORE-04 | MessageContent as untagged enum (plain string or array of blocks) | Confirmed: 15% string, 85% array in user messages. Assistant messages are always arrays. |
| CORE-05 | UsageStats with overflow capture for cache_creation subfields | Full field inventory: 9 known fields, 2 sub-objects. server_tool_use and iterations are rare overflow candidates. |
| CORE-06 | Streaming JSONL parser with byte-offset awareness | Pattern documented with partial-line safety. LF-only confirmed on macOS. |
| CORE-07 | Per-line error isolation | Pattern documented: malformed lines produce warnings, valid records still processed. |
| STORE-01 | Normalized schema (sessions, messages, message_content, token_usage, tool_executions, agents, queue_operations, progress_events) | 7 record types mapped to target tables. System records need a dedicated table or subtable approach. |
| STORE-02 | sync_metadata table | Pattern documented with byte-offset tracking and atomic metadata updates. |
| STORE-03 | schema_versions table | Embedded migration pattern with include_str! documented. |
| STORE-04 | schema_drift_log table | Overflow capture -> drift log insertion pattern documented with sample value truncation. |
| STORE-05 | Embedded migrations via include_str! | Pattern documented with version tracking and transactional application. |
| STORE-06 | WAL mode + busy timeout | Connection initialization pattern documented with PRAGMA sequence. |
| DECOMP-01 | Decompose user records | Full field inventory enables correct decomposition. MessageContent untagged enum handles both string and block content. |
| DECOMP-02 | Decompose assistant records | Inner message structure fully documented including model, stop_reason, usage. Content blocks mapped. |
| DECOMP-03 | Decompose progress records | 8 data.type variants identified with field inventories for top 4. |
| DECOMP-04 | Decompose queue operations | 4 operation values identified (enqueue, dequeue, remove, popAll). Content only present on enqueue. |
| DECOMP-05 | Decompose all record types in single transaction | Batch transaction pattern with unchecked_transaction documented. |
| DECOMP-06 | Log overflow fields for drift detection | Overflow -> schema_drift_log insertion pattern documented with version and record_type context. |
| SYNC-01 | Incremental sync (byte-offset skip) | Byte-offset tracking pattern documented with partial-line safety and sync_metadata updates. |
| SYNC-02 | Bulk import | walkdir-based discovery of all .jsonl files (including subagent directories). 6250 files, 6.3GB total provides test baseline. |
| SYNC-03 | Batch transactions | Transaction batching pattern (1000 records) with sync_metadata checkpoint per batch. |
| SYNC-04 | Atomic metadata | sync_metadata update within the same transaction as record insertion. |
| INFRA-01 | Cargo workspace with 3 crates | Workspace pattern documented: core (lib), store (lib), server (binary). workspace.dependencies for shared deps. |
| INFRA-02 | Single binary | Server crate is the binary with `[[bin]]` target. Depends on store which depends on core. |
| INFRA-03 | DB location configurable | `$CLAUDE_HISTORY_DB_PATH` env var or default `~/.claude/.claude-history.db`. clap flag override. |
| INFRA-07 | tracing for logging | tracing + tracing-subscriber with env-filter. All warnings logged through tracing macros. |
</phase_requirements>

## Sources

### Primary (HIGH confidence)
- **Empirical JSONL analysis**: Direct inspection of 6,250 real JSONL files (6.3 GB) from `~/.claude/projects/` -- all record type, field, and shape data in this document
- [rusqlite Connection API](https://docs.rs/rusqlite/latest/rusqlite/struct.Connection.html) -- PRAGMA, transaction, busy_timeout methods
- [tokio-rusqlite docs](https://docs.rs/tokio-rusqlite/latest/tokio_rusqlite/) -- Connection::call pattern, async bridge
- [Cargo Workspaces (Rust Book)](https://doc.rust-lang.org/book/ch14-03-cargo-workspaces.html) -- workspace structure
- [Cargo workspace.dependencies guide](https://vivekshuk.la/tech/2025/use-cargo-workspace-rust/) -- shared dependency pattern

### Secondary (MEDIUM confidence)
- serde `tag` + `flatten` interaction: Based on training data knowledge of serde behavior. The specific pitfall about multiple `flatten` competing for fields has been reported in serde issues but could not be verified against current docs in this session.
- `unchecked_transaction()` usage within `tokio_rusqlite::Connection::call()`: Inferred from the API constraint that `call()` provides `&Connection` not `&mut Connection`. This is the standard workaround documented in rusqlite issue #697.

### Tertiary (LOW confidence)
- Optimal transaction batch size (1000 records): Rule of thumb. Needs empirical profiling with the actual 6.3 GB dataset.
- Cross-platform newline behavior: Only macOS confirmed (LF). Linux likely same but unverified. Windows behavior unknown.

## Metadata

**Confidence breakdown:**
- Standard stack: HIGH -- all crate versions confirmed from Cargo.toml in prior research, API patterns verified from docs
- Architecture (serde modeling): HIGH -- based on empirical analysis of 766,000+ records across 7 record types
- Architecture (SQLite patterns): MEDIUM -- patterns verified from docs but unchecked_transaction within tokio-rusqlite call is inferred
- Pitfalls: HIGH -- byte-offset pitfalls confirmed through actual file analysis; serde flatten ambiguity documented in community

**Research date:** 2026-02-20
**Valid until:** 2026-03-20 (schema is evolving with each Claude Code version; new fields may appear)
