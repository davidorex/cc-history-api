## Context

The `JSONLRecord` enum at `crates/core/src/record.rs:23-46` is a `#[serde(tag = "type")]` discriminated union over seven known variants. Each variant struct also carries `#[serde(flatten)] pub overflow: HashMap<String, Value>` to absorb unknown **fields** without failing deserialization, paired with a drift-tracking system in `crates/store/src/drift.rs` that records every unknown field to `schema_drift_log`.

The discriminator value itself, however, has no fallback. When Claude Code emits a JSONL line whose `type` is not one of the seven known names, serde's tagged-enum deserializer rejects the entire object. The parser at `crates/core/src/parser.rs:125-154` catches the error as a `ParseWarning`, emits a `tracing::warn!` with the byte offset, advances past the line, and writes nothing to disk. Unlike unknown fields, **unknown record-type values are silently lost from the database**.

This investigation was triggered by 5 such warnings in `~/Library/Logs/claude-history.err.log` at 2026-05-07T21:44:16Z naming an `attachment` variant. Surveying every JSONL file under `~/.claude/projects/` reveals the loss is not isolated to that one record type or those five lines.

## Evidence

The five logged offsets in `/Users/david/.claude/projects/-Users-david-Projects-cc-history-api/0af98296-b623-4f60-a96e-4610d9a0c4e6.jsonl` were extracted byte-by-byte (Python `f.seek` + read-until-LF) and parsed as JSON. All five parse cleanly as JSON; serde rejects them only because of the discriminator. Three of the five are emitted by `version: "2.1.126"` and carry `entrypoint: "cli"`, `cwd: "/Users/david/Projects/cc-history-api"`, full `sessionId`, full `uuid`, and `timestamp` — i.e. they have all the envelope fields a `user`/`assistant`/`system` record would have, plus an `attachment` body object.

A full survey of all 9,113 JSONL files under `~/.claude/projects/` (Python walk + `json.loads` per line, filtering on `type` not in the seven-variant set) produced these counts:

| Unknown `type` value | Total occurrences |
|----------------------|------------------:|
| `attachment`         | 10,085            |
| `last-prompt`        | 1,371             |
| `custom-title`       | 884               |
| `permission-mode`    | 868               |
| `agent-name`         | 296               |
| `ai-title`           | 22                |

Six unknown record-type variants total; **13,526 record lines silently dropped** across the corpus to date. `attachment` is the dominant one but not the only one.

## Schema of `attachment` records

`attachment` records share the full-base envelope (uuid, timestamp, sessionId, version, cwd, userType, entrypoint, gitBranch, slug, parentUuid, isSidechain) and add a single `attachment` object whose internal shape is itself discriminated by an inner `attachment.type`. Across 10,085 records, twenty-two distinct inner `attachment.type` values were observed:

| inner `attachment.type`    | count |
|----------------------------|------:|
| `hook_success`             | 9,079 |
| `task_reminder`            |   439 |
| `edited_text_file`         |    80 |
| `todo_reminder`            |    70 |
| `deferred_tools_delta`     |    69 |
| `skill_listing`            |    66 |
| `hook_permission_decision` |    58 |
| `mcp_instructions_delta`   |    54 |
| `plan_mode`                |    30 |
| `plan_mode_exit`           |    30 |
| `auto_mode`                |    27 |
| `date_change`              |    25 |
| `plan_mode_reentry`        |    14 |
| `nested_memory`            |    11 |
| `file`                     |     8 |
| `command_permissions`      |     7 |
| `queued_command`           |     7 |
| `auto_mode_exit`           |     4 |
| `compact_file_reference`   |     2 |
| `plan_file_reference`      |     2 |
| `agent_mention`            |     2 |
| `invoked_skills`           |     1 |

### Hook-success attachment (90% of records)

Verbatim, line at offset 5571335 (this file):

```json
{
  "parentUuid": "6915d400-2f8d-42c0-aa48-90adfc9406d1",
  "isSidechain": false,
  "attachment": {
    "type": "hook_success",
    "hookName": "PreToolUse:Bash",
    "toolUseID": "toolu_01LyutjnMHRcYiNvb1t9e8fL",
    "hookEvent": "PreToolUse",
    "content": "",
    "stdout": "{}\n",
    "stderr": "",
    "exitCode": 0,
    "command": "python3 ${CLAUDE_PLUGIN_ROOT}/hooks/pretooluse.py",
    "durationMs": 44
  },
  "type": "attachment",
  "uuid": "62e8edcf-f666-405b-a40f-888519024ff6",
  "timestamp": "2026-05-07T21:43:48.596Z",
  "userType": "external",
  "entrypoint": "cli",
  "cwd": "/Users/david/Projects/cc-history-api",
  "sessionId": "0af98296-b623-4f60-a96e-4610d9a0c4e6",
  "version": "2.1.126",
  "gitBranch": "main",
  "slug": "curious-napping-koala"
}
```

Observed fields on the `attachment.hook_success` body: `type`, `hookName`, `toolUseID`, `hookEvent`, `content` (string, often empty), `stdout`, `stderr`, `exitCode` (int), `command` (the spawned shell command), `durationMs` (int). The `hookName`/`hookEvent` pair tells which lifecycle stage the hook ran at (PreToolUse, PostToolUse, UserPromptSubmit, PermissionRequest, …). The `toolUseID` correlates the hook record back to a `tool_use` block in an assistant message, providing real ground truth about which hooks fired around which tool calls — *inference* on semantics, but the foreign-key shape (`toolUseID`) is observed.

### Hook-permission-decision attachment

Verbatim, offset 5578548:

```json
{
  "parentUuid": "11470aa9-fef3-4e32-9822-4456731f512b",
  "isSidechain": false,
  "attachment": {
    "type": "hook_permission_decision",
    "decision": "allow",
    "toolUseID": "toolu_01CLGG3rZW2pggChcT12e43g",
    "hookEvent": "PermissionRequest"
  },
  "type": "attachment",
  "uuid": "8beb42dc-1e64-4710-840a-610f1f8f4e4b",
  "timestamp": "2026-05-07T21:44:15.400Z",
  ...
}
```

This is the per-tool-invocation permission outcome. `decision` ∈ {"allow", …}; observed only "allow" in the sampled records, but other values are plausible (*inference*). 58 occurrences corpus-wide.

### Other inner subtypes (selected)

- **`deferred_tools_delta`** (69 occ): carries `addedNames: [string]` listing tools that became available. A direct record of tool-availability evolution within a session.
- **`mcp_instructions_delta`** (54 occ): `addedNames: [string]` and `addedBlocks: [string]` — full MCP server instruction text injected into the conversation. High-information-density text content.
- **`skill_listing`** (66 occ): `content: string` — the full skill manifest text that was delivered to the model. A complete record of what skills the model saw.
- **`plan_mode`** / **`plan_mode_exit`** / **`plan_mode_reentry`**: carry `planFilePath`, `planExists`, optionally `reminderType` and `isSubAgent`. Plan-mode lifecycle events.
- **`task_reminder`** / **`todo_reminder`**: `content: [todo objects]` and `itemCount: int` — todo state snapshots injected into the conversation as reminders.
- **`edited_text_file`** (80 occ): `filename: string`, `snippet: string` — captures user-edited file contents that were attached to a turn (e.g. paste-replace flow).
- **`date_change`**: `newDate: "YYYY-MM-DD"`. Marks a wall-clock day boundary inside a long session.
- **`nested_memory`** (11 occ): `path: string`, `content: { path, type, content }` — a CLAUDE.md from a nested project directory loaded into context.
- **`auto_mode`** / **`auto_mode_exit`**, **`command_permissions`** (`allowedTools: []`), **`compact_file_reference`**, **`plan_file_reference`**, **`agent_mention`**, **`queued_command`**, **`file`**, **`invoked_skills`**: all low-volume; each is a discrete conversation-meta event.

The variation across `attachment.type` is large enough that one Rust struct cannot model it without itself being an enum. The **outer envelope is stable** (full-base shape); **only the `attachment` payload varies** by inner discriminator.

## Scope of impact

- **Files affected:** 79 of 9,113 JSONL files (≈0.87%).
- **Distinct project directories affected:** 25 (out of ~hundreds).
- **Total `attachment` records dropped:** 10,085.
- **Total dropped across all six unknown record-type values:** 13,526.

Concentration is uneven. Top projects by attachment count:

| Project dir                                                                                          | count |
|------------------------------------------------------------------------------------------------------|------:|
| `-Users-david-Projects-workflowsPiExtension`                                                          | 5,949 |
| `-Users-david-Projects-nanoclaw-next`                                                                 |   643 |
| `-Users-david-Projects-dot-claude`                                                                    |   603 |
| `-Users-david-Projects-nanoclaw`                                                                      |   406 |
| `-Users-david-Projects-workflowsPiExtension--claude-worktrees-modest-franklin-ebc8ff`                 |   404 |
| `-Users-david-Projects-MUSE-SYNTH`                                                                    |   401 |
| `-Users-david-Projects-cc-history-api`                                                                |   374 |

By month: 5,411 in 2026-04 and 4,680 in 2026-05 (the data does not extend earlier than April in projects that have attachments — *observed*, not inferred). The records are concentrated in **recent** files; this is a new-and-growing emission, not a historical artifact.

By Claude Code version, attachments appeared on:

| version  | count |
|----------|------:|
| 2.1.121  | 4,272 |
| 2.1.116  | 1,334 |
| 2.1.100  | 1,069 |
| 2.1.101  | 1,050 |
| 2.1.119  |   775 |
| 2.1.126  |   374 |
| 2.1.113  |   354 |
| 2.1.112  |   188 |
| (others) | <200 ea. |
| 2.1.91   |    33 |

The earliest version observed emitting `attachment` is **2.1.91**. This means the emission predates the 2.1.126 version reported in the err-log warnings — those five new warnings are simply the most recent occurrences in the live ingest, not the first emissions.

## Parser behavior on malformed lines

The full parser body lives at `crates/core/src/parser.rs:84-174`. Quoting the relevant block (`parser.rs:125-156`):

```rust
match serde_json::from_str::<JSONLRecord>(&line) {
    Ok(record) => {
        records.push((record, line_start_offset));
    }
    Err(e) => {
        tracing::warn!(
            file = %path.display(),
            line = line_num,
            offset = line_start_offset,
            error = %e,
            "Malformed JSONL line"
        );
        let preview = if line.len() > 500 { /* truncate at char boundary */ } else { line.clone() };
        warnings.push(ParseWarning { line_number: line_num, byte_offset: line_start_offset, error: e.to_string(), raw_line_preview: preview });
        lines_failed += 1;
    }
}

current_offset += line_byte_len;
```

Properties of this code path:

1. **The byte offset advances past every line, parsed or not** (`parser.rs:156`, unconditional `current_offset += line_byte_len`). So a malformed line is consumed, never retried by the next sync.
2. **The warning lives only in process memory** (returned as `ParseWarning` in `ParseResult.warnings`, `parser.rs:146-151`). It is never persisted to any SQLite table. The `tracing::warn!` line at `parser.rs:130-136` writes to whatever subscriber is attached (in production: launchd's `claude-history.err.log` file).
3. **Sync metadata treats malformed lines as consumed**, alongside successful records. In `crates/store/src/sync.rs`, both the empty-records path (`sync.rs:172-191`) and the batched path (`sync.rs:228-262`) update `sync_metadata.last_byte_offset` to `parsed.new_offset` (or to a chunk-boundary offset). `parsed.new_offset` is the parser's post-loop `current_offset`, which already advanced past every malformed line. Re-syncing the file will not re-read those bytes.
4. **`SyncFileResult.records_failed` is updated** with `parsed.lines_failed` (`sync.rs:168`) and added to the aggregate `SyncAllResult.records_errored` only via the `sync_file` error path; the per-line malformed counter is folded into `records_failed` (`sync.rs:286`) but does not cause the file-level result to be flagged. Lines failed are carried as a `usize` count, not as content.
5. **No `tx.execute` is invoked for the failure branch.** The success branch eventually calls `decompose::decompose_record` (`sync.rs:207`), which executes many `tx.execute` statements and calls `drift::log_overflow` for each overflow HashMap (`crates/store/src/decompose.rs:439, 487, 532, 538, 569, 601, 657, 686, 716`). The failure branch reaches none of these. Specifically: nothing analogous to `log_overflow` exists for unknown record-type values — `crates/store/src/drift.rs:107-156` (`log_record_overflow`) only switches over the seven known `JSONLRecord` variants and would not be reachable for an unknown type because the enum cannot construct an unknown variant in the first place.

**Confirmation that no row is written for malformed lines:** the only persistence in the malformed-line path is the eventual `sync_metadata` update (`sync.rs:174-181` or `sync.rs:254-262`), which records *that the parser advanced past the line* but contains no content from the line. Nothing lands in `schema_drift_log`, `messages`, `message_content`, `system_events`, or any other table.

**Re-sync semantics:** because `last_byte_offset` advances past malformed lines, fixing the parser later does not retroactively recover dropped lines from the existing DB. A bytewise re-ingestion of those files (resetting their `sync_metadata.last_byte_offset = 0` or rebuilding the DB) is required to recover the data.

## Resolution paths

### Path A: catch-all `Unknown(Value)` variant on `JSONLRecord`

**Serde mechanics.** `#[serde(tag = "type")]` does not support `#[serde(other)]` on a tuple variant. `#[serde(other)]` is documented as working only for unit variants on internally-tagged enums (it lets you map any unknown discriminator to a unit variant — but you lose the payload). To preserve the full original JSON, the options are:

1. **Manual `Deserialize` impl on `JSONLRecord`.** Deserialize first into `serde_json::Value`, inspect the `type` field, dispatch to the appropriate typed variant via `serde_json::from_value`, and fall back to `JSONLRecord::Unknown(Value)` if the type is not one of the seven. This is the most flexible option and preserves the entire original record. Cost: a hand-rolled `Deserialize` impl of ~30-50 lines, plus its own unit tests.
2. **Two-pass parse in `parser.rs`.** Try `from_str::<JSONLRecord>` first; on failure, try `from_str::<Value>`, check whether the `type` discriminator is a string at all (vs. truly malformed JSON), and if so build a synthetic `JSONLRecord::Unknown { type_name, raw }` shape. Cost: parser logic gets a second branch and warning behavior splits between truly-malformed-JSON and unknown-discriminator. Slightly more parser change but no manual `Deserialize` impl.
3. **`#[serde(other)] Unknown` unit variant** plus storing the raw JSON via a separate parallel parse to `Value`. Hybrid of (1) and (2). Loses simplicity.

**Decomposer routing.** `crates/store/src/decompose.rs:54-69` `decompose_record` would gain a new arm:

```rust
JSONLRecord::Unknown { type_name, raw } => decompose_unknown(type_name, raw, session_id_from_file, tx)?,
```

`decompose_unknown` would write to a new `record_type_drift_log` table (mirroring `schema_drift_log`'s shape: `field_name → type_name`, with `record_type = "<unknown>"`) and optionally to a `dropped_records` archival table holding the full JSON for forensic recovery. Drift logging via `drift::log_record_overflow` would extend with a new arm (`crates/store/src/drift.rs:107-156`).

**Migration sketch (007).** Following the `00N_descriptor.sql` pattern in `crates/store/migrations/`:

```sql
-- 007_record_type_drift.sql
CREATE TABLE record_type_drift_log (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    type_name       TEXT NOT NULL,
    version         TEXT,
    sample_value    TEXT,
    first_seen_at   TEXT NOT NULL DEFAULT (datetime('now')),
    last_seen_at    TEXT,
    occurrence_count INTEGER DEFAULT 1,
    UNIQUE(type_name, version)
);
-- optional, for forensic recovery:
CREATE TABLE dropped_records (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    file_path       TEXT NOT NULL,
    byte_offset     INTEGER NOT NULL,
    type_name       TEXT,
    raw_json        TEXT NOT NULL,
    captured_at     TEXT NOT NULL DEFAULT (datetime('now'))
);
```

**Files that would change.**
- `crates/core/src/record.rs` — add `Unknown` variant; add manual `Deserialize` impl OR refactor to two-pass in parser.
- `crates/core/src/parser.rs` — possibly the two-pass branch (path 2 only).
- `crates/store/src/decompose.rs` — add `JSONLRecord::Unknown` arm to dispatcher and new `decompose_unknown` fn.
- `crates/store/src/drift.rs` — add `log_unknown_record` fn or extend `log_record_overflow`.
- `crates/store/migrations/007_record_type_drift.sql` — new file.
- `crates/store/src/schema.rs` — register migration 007 in the migration runner (find via grep on existing `00X_*.sql` registrations).
- CLI surface (`crates/cli/src/commands/`, surface unverified for this report): a new `record-type-drift` subcommand mirroring `schema-drift`. Likely also extend `version-check` to surface dropped record-type counts.

**Risk of breaking existing functionality.** Low if the manual `Deserialize` impl for `JSONLRecord` is carefully written: the seven typed variants must continue to deserialize identically. Test coverage in `crates/core/src/record.rs:177-524` already exercises every variant; running those tests against the new impl is the regression net.

**Preserves the no-data-loss invariant?** Yes — fully, if `dropped_records` is included. Without `dropped_records`, only the discriminator name and a sample are preserved (analogous to `schema_drift_log`). Either is strictly more than today.

**LOC estimate:** ~150-250 LOC including the manual `Deserialize` impl (~50), decomposer arm (~30), drift table schema + Rust insert helper (~40), CLI surfacing (~40), and tests (~50-100).

### Path B: explicitly model `attachment` (and the other five) as known variants

**Variant struct sketch (attachment).** Given the full-base envelope plus a polymorphic body:

```rust
#[serde(rename = "attachment")]
Attachment(AttachmentRecord),

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AttachmentRecord {
    #[serde(flatten)]
    pub base: RecordBase,
    pub attachment: AttachmentBody,
    #[serde(flatten)]
    pub overflow: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AttachmentBody {
    HookSuccess { hook_name: String, tool_use_id: Option<String>, hook_event: String,
                  stdout: String, stderr: String, exit_code: i64, command: Option<String>,
                  duration_ms: i64, content: Option<String>,
                  #[serde(flatten)] overflow: HashMap<String, serde_json::Value> },
    HookPermissionDecision { decision: String, tool_use_id: Option<String>, hook_event: String,
                  #[serde(flatten)] overflow: HashMap<String, serde_json::Value> },
    DateChange { new_date: String,
                  #[serde(flatten)] overflow: HashMap<String, serde_json::Value> },
    PlanMode { reminder_type: Option<String>, is_sub_agent: Option<bool>,
                  plan_file_path: Option<String>, plan_exists: Option<bool>,
                  #[serde(flatten)] overflow: HashMap<String, serde_json::Value> },
    PlanModeExit { plan_file_path: Option<String>, plan_exists: Option<bool>,
                  #[serde(flatten)] overflow: HashMap<String, serde_json::Value> },
    PlanModeReentry { /* similar */ },
    SkillListing { content: String, #[serde(flatten)] overflow: HashMap<String, serde_json::Value> },
    DeferredToolsDelta { added_names: Vec<String>, /* possibly removed_names */
                  #[serde(flatten)] overflow: HashMap<String, serde_json::Value> },
    McpInstructionsDelta { added_names: Vec<String>, added_blocks: Vec<String>,
                  #[serde(flatten)] overflow: HashMap<String, serde_json::Value> },
    TaskReminder { content: serde_json::Value, item_count: i64,
                  #[serde(flatten)] overflow: HashMap<String, serde_json::Value> },
    TodoReminder { content: serde_json::Value, item_count: i64,
                  #[serde(flatten)] overflow: HashMap<String, serde_json::Value> },
    EditedTextFile { filename: String, snippet: String,
                  #[serde(flatten)] overflow: HashMap<String, serde_json::Value> },
    NestedMemory { path: String, content: serde_json::Value,
                  #[serde(flatten)] overflow: HashMap<String, serde_json::Value> },
    AutoMode { /* fields TBD from corpus sampling */ },
    AutoModeExit { /* … */ },
    CommandPermissions { allowed_tools: Vec<String> },
    QueuedCommand { /* … */ },
    CompactFileReference { /* … */ },
    PlanFileReference { /* … */ },
    AgentMention { /* … */ },
    InvokedSkills { /* … */ },
    File { /* … */ },
    // catch-all for new attachment subtypes:
    #[serde(other)]  // unit-only — loses payload on unknown subtype
    UnknownSubtype,
}
```

The 22 inner subtypes mean `AttachmentBody` is large and its catch-all (`#[serde(other)]`) is unit-only, so attachment-subtype drift would still need a fallback (a manual `Deserialize` impl on `AttachmentBody`, mirroring Path A's pattern but at the inner level).

**Decomposer routing.** Several reasonable shapes:

1. **A new `attachments` table** keyed by uuid, storing `(uuid, session_id, parent_uuid, timestamp, attachment_type, attachment_json)` plus extracted columns for hot fields like `hook_name`, `tool_use_id`, `hook_event`, `exit_code`, `duration_ms`. This is the closest analog to `system_events` (which has its own table at `system_events.session_id`/`subtype` per the existing schema).
2. **Merge into existing `messages` with `msg_type = 'attachment'`**: less work, but `messages` carries content-block expectations that don't fit attachment payloads. `messages` is also already filtered analytically as user/assistant turns; mixing in attachments distorts the "conversation message" semantics already encoded in views like `v_session_cost`.
3. **Separate per-subtype tables** (`hook_executions`, `plan_mode_events`, …): higher fidelity, much more migration work. Some of these (hook_success especially) are operationally rich enough that this is a reasonable long-term shape, but it's more than one migration's worth.

**Semantic value.** From the field shapes (*partial inference*):
- `hook_success` records carry **ground truth about hook execution** (which hooks ran, how long, what they output). Currently, hook execution is inferred only via `system_events` of subtype `stop_hook_summary`. With 9,079 records, this is a substantial dataset for hook-behavior analysis that is being silently lost.
- `mcp_instructions_delta` and `skill_listing` records carry **the full text of context injected into the model**. This is conversation-shaping information that is otherwise completely invisible in the database.
- `task_reminder` / `todo_reminder` records carry **structured todo state at conversation time** — a complement to the `TodoWrite` tool calls that are already captured in `tool_executions`.
- `edited_text_file` records carry **user-pasted file content** that the model saw — primary input that is otherwise unrepresented.
- `plan_mode*` records carry **plan-mode lifecycle events**, complementing `system.subtype = "stop_hook_summary"`.

**Files that would change.**
- `crates/core/src/record.rs` — add `Attachment` variant + `AttachmentRecord` + `AttachmentBody` enum (~150 lines) + tests for each subtype (~200 lines).
- `crates/store/src/decompose.rs` — add `decompose_attachment` (~60-200 lines depending on whether per-subtype routing happens).
- `crates/store/src/drift.rs` — extend `log_record_overflow` with attachment arms.
- `crates/store/migrations/007_attachments.sql` — new tables and indexes.
- `crates/store/src/schema.rs` — register migration.
- CLI `query` / `search_messages` etc. — decide whether attachment content is searchable via FTS5. `crates/store/src/fts.rs` would need to know whether to index attachment text content (e.g. `skill_listing.content`, `mcp_instructions_delta.added_blocks`).
- The five other unknown record-type values (`last-prompt`, `custom-title`, `permission-mode`, `agent-name`, `ai-title`) are smaller in volume but Path B does not address them unless each gets its own variant. Their shapes are simple key-value lookups (single field per type beyond sessionId), so a generic `session_metadata` table keyed by `(session_id, key)` could absorb all five. That is a separate small migration.

**Risk of breaking existing functionality.** Moderate. Adding a variant requires updating every match on `JSONLRecord` in the codebase (the dispatcher in `decompose.rs:59-69`, `drift.rs:111-155`, plus any pattern matches in tests and CLI). Compiler will surface non-exhaustive matches. The bigger risk is in `AttachmentBody`: 22 subtypes is enough surface area that getting any single subtype's struct shape wrong on first encounter will silently dump the record into the unknown-subtype catch-all (or fail, depending on impl).

**Preserves the no-data-loss invariant?** Yes for known subtypes. For unknown attachment subtypes (i.e. future ones not yet seen), depends on the catch-all. With `#[serde(other)] UnknownSubtype` you preserve discriminator-name only; with a manual `Deserialize` impl you can preserve full body. Either is strictly more than today.

**LOC estimate:** ~600-1000 LOC including schema, struct definitions, decomposer per-subtype routing, drift extensions, FTS integration decisions, and tests. Substantially more than Path A.

### Path C: do nothing, accept the data loss

**What is currently lost.** Per the table above: 13,526 records across six unknown record-type values. The largest category by far (10,085 of 13,526 = 75%) is `attachment` with `hook_success` as the dominant inner subtype. Operational interpretation:

- `attachment.hook_success` records (9,079) are **operational ops chatter** in one sense — they record whether each hook ran successfully, with stdout/stderr/duration. But they are also the only direct ground truth about hook activity. Whether this matters depends on whether hook behavior analysis is a target of the store.
- `attachment.skill_listing` (66), `attachment.mcp_instructions_delta` (54), `attachment.deferred_tools_delta` (69), `attachment.edited_text_file` (80), `attachment.nested_memory` (11) are **conversation-shaping content** that influences model behavior and is otherwise unrepresented. These are arguably analytically important.
- `attachment.plan_mode*` (74 across three subtypes), `attachment.auto_mode*` (31), `attachment.task_reminder`/`todo_reminder` (509) are **conversation-state transitions** that complement existing system-event captures but are not redundant with them.
- `last-prompt` (1,371), `custom-title` (884), `ai-title` (22), `permission-mode` (868), `agent-name` (296) are session-level metadata that is **mostly redundant** with what already lands via other records (sessions table, system events). These are the lowest-value of the six; doing nothing for them is defensible.

**Less-than-architectural mitigations.**
- A **preprocessor pass** that rewrites `"type":"attachment"` → `"type":"system"` (or to a sentinel) before ingestion would coerce serde to accept the line, and the attachment body would land in `system.overflow`. Crude, distorts the system-event table, and triples down on the schema_drift_log tail. Not recommended even as a stopgap.
- A **side-channel writer** at the parser layer: when `parse_jsonl` sees a malformed line whose JSON is well-formed and whose `type` is a string not in the known set, write the raw line to `~/.claude/.claude-history-dropped.jsonl` for later batch reprocessing. This buys time without DB schema work. ~30 LOC. Loses queryability but preserves the bytes.
- **Simply not advancing the byte offset past unknown-discriminator lines** would cause infinite re-read loops on every sync. Not viable.

**Risk.** Continued silent data loss, growing at roughly the rate of the 2026-04→2026-05 trajectory (~4,500-5,000 attachments/month at recent volume). The bytes stay in the source files (so retroactive recovery via reset-and-resync is always possible later), but the live DB lacks the data and warnings continue to accumulate in launchd's err.log.

**Preserves the no-data-loss invariant?** No — explicitly violates it. The drift system was built around the assumption that no field is ever silently dropped; this is the only known site where an entire record is silently dropped.

## Cross-cutting observations

### Existing unknown-variant handling in the codebase

- **`MessageContent` (`crates/core/src/message.rs:22-27`)** uses `#[serde(untagged)]` with two arms (string vs block array). It does not have a fallback — a third shape would also fail. No precedent here.
- **`ContentBlock` (`message.rs:38-71`)** uses `#[serde(tag = "type")]` with four variants (text, thinking, tool_use, tool_result). No catch-all variant; an unknown content-block `type` would cause the entire record to fail to deserialize. This is a **second blind spot** in the same architectural shape: if Claude Code emits a new content-block type (e.g. `image`, `video`), every assistant or user record containing it gets dropped at the parser. The same resolution paths apply in miniature.
- **No `#[serde(other)]` annotations** exist in `crates/core/src/`. Confirmed by grep.
- **No `#[serde(deny_unknown_fields)]`** annotations exist in `crates/core/src/`. Confirmed by grep. This is the field-level analog of what's missing at the variant level — and the codebase already chose the permissive direction for fields. The same direction at the variant level is consistent.

So Path A's catch-all approach is **architecturally consistent with the existing design philosophy** (overflow capture for fields), just extended to the variant level. There is no precedent in the codebase for this particular pattern, but the design intent of `schema_drift_log` plus `serde(flatten) overflow` strongly implies the absence is an oversight rather than an intentional choice.

### Other unknown record-type values lurking

Already enumerated in the Evidence section. To recap, beyond `attachment`:

- `last-prompt` (1,371): `{type, lastPrompt, sessionId}` — last prompt cache for session resume.
- `custom-title` (884): `{type, customTitle, sessionId}` — user-set session title.
- `permission-mode` (868): `{type, permissionMode, sessionId}` — current permission mode.
- `agent-name` (296): `{type, agentName, sessionId}` — agent display name.
- `ai-title` (22): `{type, aiTitle, sessionId}` — auto-generated session title.

All five are flat session-metadata records with one or two scalar fields plus `sessionId`. They could be absorbed by a single `session_metadata (session_id, key, value, observed_at)` table without per-record-type variant work. Path A handles them by-default (each lands in `Unknown(Value)` and the discriminator goes to `record_type_drift_log`). Path B would need to model each.

### Which versions emit unknown records — and the implication for forward-compat

The `attachment` variant goes back to **Claude Code 2.1.91** in this corpus. The drift-system was built against the assumption that field-level evolution was the only forward-compat axis; the existence of *six* unknown record-type discriminators in production data (some appearing for thousands of records) suggests Claude Code's record-type space evolved in parallel, and the cc-history-api's deserializer did not. A defensive resolution (Path A) closes that axis going forward; a complete resolution (Path A + Path B for the high-value subset) closes it and recovers the semantic content.

## Open questions

1. **Are there `attachment` records older than 2.1.91 that I missed?** The corpus is a snapshot of the user's local `~/.claude/projects/`. If older sessions were pruned or exported and re-imported, the version distribution above is biased toward retained data. I cannot tell from the local data whether 2.1.91 is the true first version or merely the earliest still on disk.
2. **Do `attachment` records ever omit fields the typed envelope requires?** I sampled 22 inner subtypes from the corpus but did not validate that every single one of the 10,085 records carries the full base envelope (uuid, timestamp, sessionId, version, cwd, userType, gitBranch, parentUuid, isSidechain). A ~5,000-record sweep would establish the floor on envelope completeness before committing to a typed `AttachmentRecord` struct.
3. **Is `attachment.hook_success.toolUseID` always a tool_use id reachable via existing `tool_executions.tool_use_id`?** Some samples show `toolUseID: "116ad80a-a43d-418a-80b3-c34c3d3a22d6"` (UUID-shaped, not the `toolu_…` Anthropic-API-tool-id form) for `UserPromptSubmit` hooks, which indicates `toolUseID` is being used as a synthetic correlation id outside tool-use boundaries. A FK in the new schema would break for those.
4. **Will `ContentBlock` see unknown variants too, and on what timeline?** Untested. Worth a similar grep across all assistant records' content blocks before committing to a JSONL-record-only fix.
5. **Recovery strategy for already-dropped records.** Once a fix is deployed, the existing 13,526 dropped records can be recovered by resetting `sync_metadata.last_byte_offset = 0` for the 79+ affected files and re-running sync (idempotent INSERTs are documented in `decompose.rs:1-7`). This is a one-time backfill; whether to do it (and whether to do it incrementally per file or via full DB rebuild) is a decision adjacent to the parser/decomposer fix.
6. **CLI surfacing.** A `claude-history record-type-drift` subcommand mirroring `claude-history schema-drift` would surface the new table to users. Out of scope for the parser fix but a natural follow-on; existing precedent is in `crates/store/src/query.rs:618-629` (`get_schema_drift`) which a parallel `get_record_type_drift` would mirror.
