# Phase 4 Audit: Real-Time Ingestion & Events — Refactoring Plan

**Audited:** 2026-02-21
**Spec sections:** 3.1 (File Watcher), 3.2 (Version Monitor), 4.6 (SSE Event Types)
**Implementation files:** `crates/server/src/events.rs`, `crates/server/src/watcher.rs`, `crates/server/src/serve.rs`

---

## 1. SSE Event Type Comparison (Spec Section 4.6 vs Implementation)

### 1.1 `record:added`

| Aspect | Spec | Implementation | Match? |
|--------|------|----------------|--------|
| Event name | `record:added` | `"record:added"` | YES |
| Spec payload | `{"session_id":"abc","uuid":"def","type":"assistant","timestamp":"..."}` | `{"session_id":"...","records_synced":N,"file_path":"..."}` | **NO** |

**Deviation:** The spec describes a per-record event with the record's `uuid`, `type`, and `timestamp`. The implementation emits a batch-level event with `records_synced` count and `file_path` instead. There is no `uuid`, no `type`, and no `timestamp` in the emitted payload.

**Severity: MODERATE** — The spec implies per-record granularity (each ingested record gets its own event with its identity). The implementation coalesces into a single event per sync_file call. A consumer expecting to track individual record arrivals cannot do so with the current shape. However, this may have been an intentional design trade-off for performance.

**Change needed:**
- Option A (match spec exactly): Emit one `record:added` per ingested record, each containing `session_id`, `uuid`, `type` (record type), and `timestamp`. This requires `sync_file` to return the parsed records (or at least their identity fields), not just a count.
- Option B (document deviation): Keep batch semantics but add `uuid` and `type` arrays to the payload, or acknowledge the deviation as intentional.
- **Files to modify:** `crates/server/src/events.rs` (RecordAdded variant fields), `crates/server/src/watcher.rs` (event emission logic), potentially `crates/store/src/sync.rs` (SyncResult must return record identities)

---

### 1.2 `session:started`

| Aspect | Spec | Implementation | Match? |
|--------|------|----------------|--------|
| Event name | `session:started` | `"session:started"` | YES |
| Spec payload | `{"session_id":"abc","project_path":"/my/project","version":"2.1.45"}` | `{"session_id":"..."}` | **NO** |

**Deviation:** Missing `project_path` and `version` fields in the payload.

**Severity: MODERATE** — Consumers wanting to filter by project or know what version started the session cannot do so from this event alone.

**Change needed:**
- Add `project_path: String` and `version: Option<String>` to the `SessionStarted` variant.
- After the first sync of a new file, query the sessions table for `project_path` (derivable from the JSONL file path) and `version`.
- **Files to modify:** `crates/server/src/events.rs` (SessionStarted variant), `crates/server/src/watcher.rs` (populate new fields when emitting)

---

### 1.3 `schema:drift`

| Aspect | Spec | Implementation | Match? |
|--------|------|----------------|--------|
| Event name | `schema:drift` | `"schema:drift"` | YES |
| Spec payload | `{"version":"2.2.0","new_fields":["newField"],"type":"additive"}` | `{"new_fields":N,"session_id":"..."}` | **NO** |

**Deviation:** Three mismatches:
1. Spec `new_fields` is an **array of field names** (strings). Implementation `new_fields` is a **count** (integer).
2. Spec includes `version` (the Claude Code version where drift was detected). Implementation includes `session_id` instead.
3. Spec includes `type` (e.g., `"additive"`). Implementation has no `type` field.

**Severity: MODERATE** — A consumer wanting to know *which* fields drifted gets only a count, not names. The version information is absent. The drift classification (`additive` vs other) is absent.

**Change needed:**
- Change `new_fields: usize` to `new_fields: Vec<String>` (list of field names).
- Add `version: String` (the Claude Code version string from the session).
- Add `drift_type: String` (e.g., `"additive"` — only type currently possible since overflow captures additions).
- The watcher_loop must query or extract the overflow field names from the sync result. Currently `sync_file` returns `overflow_fields_logged: usize`. It would need to return the actual field names.
- **Files to modify:** `crates/server/src/events.rs` (SchemaDrift variant), `crates/server/src/watcher.rs` (populate new fields), `crates/store/src/sync.rs` (SyncResult must return field names, not just count)

---

### 1.4 `version:changed`

| Aspect | Spec | Implementation | Match? |
|--------|------|----------------|--------|
| Event name | `version:changed` | `"version:changed"` | YES |
| Spec payload | `{"from":"2.1.45","to":"2.2.0"}` | `{"old_version":"...","new_version":"...","session_id":"..."}` | **PARTIAL** |

**Deviation:**
1. Field names differ: spec uses `from`/`to`, implementation uses `old_version`/`new_version`.
2. Implementation adds `session_id` which is not in the spec.

**Severity: MINOR** — The semantics are equivalent. The field name difference is a cosmetic mismatch against the spec's example. The extra `session_id` is additive and arguably useful.

**Change needed:**
- Rename `old_version` to `from` and `new_version` to `to` in the JSON payload to match spec.
- Keep `session_id` as an additive field (does not conflict with spec).
- **Files to modify:** `crates/server/src/events.rs` (VersionChanged variant field names in to_json_data, or rename the struct fields and use serde rename)

---

### 1.5 `file:written`

| Aspect | Spec | Implementation | Match? |
|--------|------|----------------|--------|
| Event name | `file:written` | `"file:written"` | YES |
| Spec payload | `{"session_id":"abc","file_path":"/src/main.rs","operation":"write","timestamp":"..."}` | `{"session_id":"...","file_path":"...","message_uuid":"..."}` | **PARTIAL** |

**Deviation:**
1. Missing `operation` field (spec says `"write"`).
2. Missing `timestamp` field.
3. Extra `message_uuid` field not in spec.

**Severity: MINOR** — The `operation` field is redundant given the event name is `file:written` (always implies "write"). The `timestamp` absence means consumers cannot know when the operation occurred without a follow-up query. `message_uuid` is additive and useful.

**Change needed:**
- Add `operation: String` (always `"write"`) and `timestamp: String` to the payload.
- Keep `message_uuid` as additive.
- The timestamp is available from the file_operations table row; the `emit_artifact_events` query should include it.
- **Files to modify:** `crates/server/src/events.rs` (FileWritten variant), `crates/server/src/watcher.rs` (emit_artifact_events query and event construction)

---

### 1.6 `file:edited`

| Aspect | Spec | Implementation | Match? |
|--------|------|----------------|--------|
| Event name | `file:edited` | `"file:edited"` | YES |
| Spec payload | `{"session_id":"abc","file_path":"/src/lib.rs","operation":"edit","old_content":"fn old()","new_content":"fn new()"}` | `{"session_id":"...","file_path":"...","message_uuid":"..."}` | **PARTIAL** |

**Deviation:**
1. Missing `operation` field (spec says `"edit"`).
2. Missing `old_content` and `new_content` fields — the spec includes the actual diff content in the event.
3. Extra `message_uuid` not in spec.

**Severity: MODERATE** — The spec explicitly includes `old_content` and `new_content` in the event payload, meaning consumers can see what changed in real-time without querying the API. The current implementation requires a follow-up query to get diff content.

**Change needed:**
- Add `operation: String` (always `"edit"`), `old_content: Option<String>`, `new_content: Option<String>` to the payload.
- The `emit_artifact_events` query for edits should also select `content` (new_string) and `old_content` (old_string) from file_operations.
- Keep `message_uuid` as additive.
- **Files to modify:** `crates/server/src/events.rs` (FileEdited variant), `crates/server/src/watcher.rs` (emit_artifact_events query and event construction)

---

### 1.7 `git:commit`

| Aspect | Spec | Implementation | Match? |
|--------|------|----------------|--------|
| Event name | `git:commit` | `"git:commit"` | YES |
| Spec payload | `{"session_id":"abc","commit_message":"Add auth middleware","branch":"feature/auth"}` | `{"session_id":"...","commit_message":"...","branch":"...","message_uuid":"..."}` | **CLOSE** |

**Deviation:**
1. Extra `message_uuid` not in spec (additive, not harmful).
2. `commit_message` and `branch` are `Option<String>` in implementation vs plain `String` in spec example.

**Severity: MINOR** — The nullable types are arguably more correct than the spec example (git operations may not always have a parseable commit message or branch). The extra `message_uuid` is useful.

**Change needed:**
- Acceptable as-is. The Optional types are a reasonable defensive choice. No changes strictly required.

---

## 2. Watcher Behavior Comparison (Spec Section 3.1)

### 2.1 Notify Crate Usage

| Aspect | Spec | Implementation | Match? |
|--------|------|----------------|--------|
| Uses `notify` crate | Yes (`notify::recommended_watcher`) | Yes (`notify::recommended_watcher`) | YES |
| Watcher on dedicated thread | Yes (`std::thread::spawn`) | Yes (`std::thread::spawn`) | YES |
| Thread kept alive | Yes (`std::thread::park()`) | Yes (`std::thread::park()`) | YES |
| Callback uses `blocking_send` | Yes (implicit from spec code) | Yes (`tx.blocking_send(result)`) | YES |

**No deviations.** The watcher infrastructure matches the spec pattern exactly.

---

### 2.2 Debounce Pattern

| Aspect | Spec | Implementation | Match? |
|--------|------|----------------|--------|
| Per-file debounce | Yes (HashMap of paths to Instants) | Yes (FileDebouncer struct) | YES |
| Debounce duration | 2 seconds (`>= 2`) | 2 seconds (Duration::from_secs(2)) | YES |
| Event coalescing | Yes (skip sync if within window) | Yes (should_sync returns false) | YES |

**Enhancement beyond spec:** Implementation adds `prune_stale()` for memory management of the debounce HashMap. This is a positive addition not in the spec.

**No deviations.** Debounce behavior matches.

---

### 2.3 Watched Paths

| Aspect | Spec | Implementation | Match? |
|--------|------|----------------|--------|
| Watch `~/.claude/projects/` | Yes (RecursiveMode::Recursive) | Yes (projects_dir, Recursive) | YES |
| Watch `~/.claude/.claude.json` | Yes (NonRecursive) | **NO — not watched** | **NO** |

**Deviation:** The spec code (section 3.1) shows the watcher monitoring two paths:
```rust
watcher.watch(&claude_dir.join("projects"), RecursiveMode::Recursive).unwrap();
watcher.watch(&claude_dir.join(".claude.json"), RecursiveMode::NonRecursive).unwrap();
```
The implementation's `spawn_watcher` only receives and watches `projects_dir`. The `.claude.json` config file is never monitored.

**Severity: MINOR** — The `.claude.json` watch was intended for the version monitor (section 3.2) to detect config changes. Since the version monitor itself was not built (see section 3 below), the config watch is effectively irrelevant in the current architecture. If/when a version monitor is added, this watch should be added.

**Change needed (deferred):**
- Add a second watch path for `.claude.json` when the version monitor is implemented.
- `spawn_watcher` would need to accept a second path or a list of watch targets.
- **Files to modify:** `crates/server/src/watcher.rs` (spawn_watcher), `crates/server/src/serve.rs` (pass additional path)

---

## 3. Version Monitor Comparison (Spec Section 3.2)

| Aspect | Spec | Implementation | Match? |
|--------|------|----------------|--------|
| Dedicated `VersionMonitor` struct | Yes (`crates/core/src/version.rs`) | **NO — does not exist** | **NO** |
| Periodic check loop | Yes (`run_loop` with `check_interval`) | **NO** | **NO** |
| Multiple version detection methods | Yes (DB query, `claude --version`, npm check) | Partial — only DB query in `check_version_change` | **PARTIAL** |
| Schema drift detection on version change | Yes (`detect_drift` compares field sets across versions) | No — drift is detected per-sync via overflow count, not per-version | **NO** |
| Standalone version check | Yes (compares installed vs last-known) | No standalone check — version detection is inline in watcher_loop | **NO** |

**Deviation:** The spec describes a full `VersionMonitor` struct in `crates/core/src/version.rs` with:
1. A periodic `run_loop` that checks for version changes on an interval (independent of file events)
2. Multiple detection methods (DB query, CLI `claude --version`, npm)
3. Schema drift detection that compares field sets between versions
4. A `get_installed_version` method that tries multiple sources

The implementation collapses version monitoring into the watcher_loop as a lightweight `check_version_change` function that only fires after a successful sync (reactive, not proactive). There is no periodic background check, no CLI/npm version detection, and no cross-version field comparison.

**Severity: MODERATE** — The spec's version monitor was designed to proactively detect Claude Code upgrades even when no new sessions are active. The current reactive approach only detects version changes when new data flows in. For users who update Claude Code and don't immediately start a session, the version change goes undetected until the next session creates data.

**Change needed (deferred — separate phase):**
- Create `crates/core/src/version.rs` with `VersionMonitor` struct
- Implement `get_installed_version` with fallback chain: DB query -> `claude --version` -> npm
- Implement `detect_drift` that compares `schema_drift_log` field sets between versions
- Add `run_loop` as a periodic background task (e.g., every 5 minutes)
- Spawn the version monitor loop alongside the watcher_loop in `serve.rs`
- **Files to create:** `crates/core/src/version.rs`
- **Files to modify:** `crates/core/src/lib.rs`, `crates/server/src/serve.rs`

---

## 4. Summary of All Deviations

| # | Component | Spec Says | Implementation Does | Severity | Category |
|---|-----------|-----------|---------------------|----------|----------|
| 1 | `record:added` payload | Per-record: `{uuid, type, timestamp, session_id}` | Per-batch: `{session_id, records_synced, file_path}` | MODERATE | Payload shape |
| 2 | `session:started` payload | `{session_id, project_path, version}` | `{session_id}` only | MODERATE | Missing fields |
| 3 | `schema:drift` payload | `{version, new_fields: [names], type}` | `{new_fields: count, session_id}` | MODERATE | Wrong types, missing fields |
| 4 | `version:changed` payload | `{from, to}` | `{old_version, new_version, session_id}` | MINOR | Field naming |
| 5 | `file:written` payload | `{session_id, file_path, operation, timestamp}` | `{session_id, file_path, message_uuid}` | MINOR | Missing fields |
| 6 | `file:edited` payload | `{session_id, file_path, operation, old_content, new_content}` | `{session_id, file_path, message_uuid}` | MODERATE | Missing diff content |
| 7 | `git:commit` payload | `{session_id, commit_message, branch}` | `{session_id, commit_message, branch, message_uuid}` | MINOR | Additive only |
| 8 | `.claude.json` watch | Watched with NonRecursive | Not watched | MINOR | Missing watch path |
| 9 | Version Monitor | Full `VersionMonitor` struct with periodic loop | Inline reactive check in watcher_loop | MODERATE | Missing subsystem |

---

## 5. Recommended Refactoring Order

### Priority 1: SSE Payload Alignment (5 changes)

These are the most impactful changes because they affect the API contract that consumers depend on.

**5.1 Fix `version:changed` field names** (MINOR, ~15 min)
- Rename `old_version` -> `from`, `new_version` -> `to` in `to_json_data()`
- File: `crates/server/src/events.rs`

**5.2 Add missing fields to `file:written`** (MINOR, ~20 min)
- Add `operation: String` and `timestamp: String` to `FileWritten`
- Update emit_artifact_events query to SELECT timestamp
- Files: `crates/server/src/events.rs`, `crates/server/src/watcher.rs`

**5.3 Add diff content to `file:edited`** (MODERATE, ~30 min)
- Add `operation: String`, `old_content: Option<String>`, `new_content: Option<String>` to `FileEdited`
- Update emit_artifact_events query to SELECT content, old_content
- Files: `crates/server/src/events.rs`, `crates/server/src/watcher.rs`

**5.4 Fix `schema:drift` payload** (MODERATE, ~45 min)
- Change `new_fields` from count to Vec<String> of field names
- Add `version: String` and `drift_type: String`
- Requires `SyncResult` to return field names instead of count
- Files: `crates/server/src/events.rs`, `crates/server/src/watcher.rs`, `crates/store/src/sync.rs`

**5.5 Add missing fields to `session:started`** (MODERATE, ~30 min)
- Add `project_path: String` and `version: Option<String>`
- Derive project_path from JSONL file path, query version from sessions table
- Files: `crates/server/src/events.rs`, `crates/server/src/watcher.rs`

### Priority 2: `record:added` Granularity Decision (1 change)

**5.6 Decide on `record:added` granularity** (MODERATE, ~1-2 hours)
- This is the most significant design decision. The spec implies per-record events.
- Requires `sync_file` to return record identities (uuid, type, timestamp), not just a count.
- This is a larger change touching `crates/store/src/sync.rs` return types.
- Alternatively: document that batch semantics are intentional and add the per-record fields as arrays.

### Priority 3: Version Monitor (separate effort)

**5.7 Implement spec section 3.2 VersionMonitor** (MODERATE, separate phase)
- New file: `crates/core/src/version.rs`
- Periodic check loop, multiple detection methods, cross-version drift analysis
- Add `.claude.json` watch path to watcher
- This is effectively a new subsystem and may warrant its own planning phase.

---

## 6. Files Requiring Modification

| File | Changes |
|------|---------|
| `crates/server/src/events.rs` | All payload shape fixes (items 5.1-5.5) |
| `crates/server/src/watcher.rs` | Event emission logic, queries for artifact events (items 5.2-5.6) |
| `crates/store/src/sync.rs` | Return field names in SyncResult for drift events; return record identities for record:added (items 5.4, 5.6) |
| `crates/core/src/version.rs` | New file for VersionMonitor (item 5.7) |
| `crates/core/src/lib.rs` | Module declaration for version.rs (item 5.7) |
| `crates/server/src/serve.rs` | Spawn version monitor loop (item 5.7) |

---

## Demo Requirements

After refactoring, `/gsd:demo-phase` must capture evidence for each SSE payload shape and watcher behavior. SSE demos require starting the daemon and triggering events via sync.

### Demo 1: version:changed event uses spec field names

**Validates:** Deviation 4 (spec section 4.6 — `{from, to}` not `{old_version, new_version}`)
**Category:** API curl (SSE capture)

```
$ ./target/debug/claude-history serve --projects-dir ~/.claude/projects &
$ sleep 2
$ timeout 5 curl -sN http://localhost:7424/v1/events > /tmp/sse-capture.txt &
$ # Trigger a sync that encounters a version change (may need test fixture)
$ sleep 6
$ grep "version:changed" /tmp/sse-capture.txt | head -1
→ data payload must contain "from" and "to" keys (not "old_version", "new_version")
```

**Observation target:** Field names in JSON payload match spec example exactly.

### Demo 2: file:written event includes operation and timestamp

**Validates:** Deviation 5 (spec section 4.6 — file:written has `{session_id, file_path, operation, timestamp}`)
**Category:** API curl (SSE capture)

```
$ timeout 15 curl -sN http://localhost:7424/v1/events > /tmp/sse-capture.txt &
$ # Trigger activity that produces file:written events (start a Claude Code session, or sync test data)
$ sleep 16
$ grep "file:written" /tmp/sse-capture.txt | head -1 | sed 's/data: //' | jq 'keys'
→ must contain "session_id", "file_path", "operation", "timestamp", "message_uuid"
```

**Observation target:** All spec-required fields present. `operation` field value is `"write"`. `timestamp` is an ISO 8601 string.

### Demo 3: file:edited event includes old_content and new_content

**Validates:** Deviation 6 (spec section 4.6 — file:edited has `{old_content, new_content}`)
**Category:** API curl (SSE capture)

```
$ grep "file:edited" /tmp/sse-capture.txt | head -1 | sed 's/data: //' | jq 'keys'
→ must contain "old_content", "new_content", "operation", "session_id", "file_path"
```

**Observation target:** Edit diff content is included in the SSE event payload, not just file identity. Consumers can see what changed without a follow-up API call.

### Demo 4: schema:drift event has field names array and version

**Validates:** Deviation 3 (spec section 4.6 — `new_fields` is array of strings, includes `version` and `type`)
**Category:** API curl (SSE capture)

```
$ grep "schema:drift" /tmp/sse-capture.txt | head -1 | sed 's/data: //' | jq '{new_fields: (.new_fields | type), version, drift_type}'
→ new_fields must be "array" (not "number")
→ version must be a string
→ drift_type must be present (e.g., "additive")
```

**Observation target:** `new_fields` is an array of field name strings (not an integer count). `version` identifies which Claude Code version introduced the drift.

### Demo 5: session:started event has project_path and version

**Validates:** Deviation 2 (spec section 4.6 — `{session_id, project_path, version}`)
**Category:** API curl (SSE capture)

```
$ grep "session:started" /tmp/sse-capture.txt | head -1 | sed 's/data: //' | jq '{session_id, project_path, version}'
→ all three fields must be present and non-null
→ project_path must be an absolute path
```

**Observation target:** session:started carries enough context for consumers to filter by project without a follow-up query.

### Demo 6: record:added granularity decision [HUMAN]

**Validates:** Deviation 1 (spec section 4.6 — per-record vs batch semantics)
**Requires:** Review the captured record:added events and decide:
- If per-record: each event has `{uuid, type, timestamp, session_id}` for a single record
- If batch: document the deviation as intentional with rationale
**Expected:** Either per-record events matching spec example, or documented rationale for batch semantics

### Demo 7: VersionMonitor periodic check [HUMAN]

**Validates:** Deviation 9 (spec section 3.2 — periodic version check loop)
**Requires:** Start daemon, wait 5+ minutes, check logs for periodic version check output even without new JSONL data flowing in
**Expected:** Log entries showing version checks at regular intervals, independent of file watcher events
