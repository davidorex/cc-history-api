# Phase 6: Version Monitoring - Research

**Researched:** 2026-02-21
**Domain:** SQLite schema evolution, ingestion pipeline extension, version tracking, compact summary message classification, analytical view correction
**Confidence:** HIGH

<spec_constraints>
## Spec Constraints (IMMUTABLE — from ROADMAP.md Success Criteria)

1. `claude-history version-check` (and GET /v1/schema/versions) shows the detected Claude Code version and a history of version changes with timestamps
2. GET /v1/schema/drift shows overflow fields grouped by version, highlighting new fields that appeared when Claude Code updated
3. In daemon mode, a periodic check loop detects version changes and logs them to schema_versions without requiring a restart

These are non-negotiable user-story outcomes. Research recommendations must not narrow below these.
</spec_constraints>

<user_constraints>
## User Constraints (from CONTEXT.md)

### Locked Decisions

**Version detection:**
- Source is the sessions table `version` field, already parsed from JSONL during ingestion (existing Phase 4 code)
- Detection is ingestion-only — no separate periodic polling loop; next session reveals any version change
- On version change: write to `schema_versions` table immediately (real-time persistence) AND fire existing `SseEvent::VersionChanged`
- Backfill on startup: scan sessions table for distinct versions and populate `schema_versions` on first run

**schema_versions table:**
- Columns: version, first_seen_at, last_seen_at, session_id (first session with this version), session_count, new_fields_count (overflow fields introduced with this version)
- Session count per version tracked (how many sessions ran on each Claude Code version)

**Drift presentation:**
- Primary grouping by Claude Code version, sub-grouped by record type within each version
- Each drift field includes truncated sample value (first ~200 chars from actual JSONL data)
- Each drift field shows promotion status: 'promoted' (has a real column), 'extra_json' (captured but not indexed), 'unhandled' (overflow only)
- Drift logging enhanced to record the Claude Code version that produced the overflow field (currently captures record context but not session version)

**/v1/schema/versions endpoint:**
- Default: timeline list — version string, first/last seen timestamps, session count, new_fields_count
- `?diff=true` query param adds per-version field diffs: new overflow fields introduced, fields that disappeared

**/v1/schema/drift endpoint:**
- Grouped by version (primary) then record type (sub-group)
- Each field: name, sample value (truncated), occurrence count, promotion status
- Version correlation shows which upgrade introduced each drift field

**Compact summary absorption (from milestone-2-spec):**
- Add `is_compact_summary INTEGER DEFAULT 0` and `source_tool_use_id TEXT` as real columns on messages table
- Add `extra_json TEXT` on messages table for residual overflow fields (container, context_management, future unknowns)
- Wire `decompose_user` and `decompose_assistant` to populate both real columns (from known overflow keys) and extra_json (remaining overflow)
- Backfill existing rows from schema_drift_log data

**Analytical view updates:**
- All views that count messages or sum tokens (v_project_summary, v_session_cost, etc.) updated with `WHERE m.is_compact_summary = 0`
- `is_compact_summary` column exposed in view output so consumers can filter explicitly if needed
- Views are accurate by default; consumers query raw tables for full picture including synthetic messages

**CLI enhancement:**
- `version-check` and `schema-drift` CLI subcommands enhanced to match API richness
- Version history timeline with session counts and drift impact
- Grouped drift output with promotion status and sample values

### Claude's Discretion
None — all decisions locked

### Deferred Ideas (OUT OF SCOPE)
None — discussion stayed within phase scope

NOTE: If any Deferred Idea conflicts with a Spec Constraint above, the conflict is flagged here:
None — all deferrals are compatible with spec constraints
</user_constraints>

<phase_requirements>
## Phase Requirements

| ID | Description | Research Support |
|----|-------------|-----------------|
| VER-01 | Detect Claude Code version from most recent JSONL record, claude --version, or npm ls | Version detection from sessions.version field already exists in watcher.rs check_version_change(). CONTEXT decision locks to sessions-table-only detection (no CLI/npm probing). Integration point at watcher.rs:375. |
| VER-02 | Record version changes in schema_versions table | Requires migration 006 creating version_history table (name avoids collision with existing schema_versions migration-tracking table). Backfill query against sessions table for startup population. Write on version change in watcher loop. |
| VER-03 | Compare overflow field sets between versions to detect new fields | schema_drift_log already has version column. Grouping drift entries by version and computing set differences between consecutive versions is a query-layer operation. The `?diff=true` param on /v1/schema/versions implements this. |
| VER-04 | Periodic version check loop in daemon mode | CONTEXT decision redefines this: detection is ingestion-triggered (not periodic polling). The existing check_version_change() in watcher_loop already fires on every sync. Phase 6 adds persistence to version_history table at that same call site. This satisfies the spec intent — versions are detected during live ingestion without restart. |
</phase_requirements>

## Summary

Phase 6 combines three related capabilities into the final milestone: (1) persistent version tracking with a dedicated table, (2) enriched schema drift presentation grouped by version with promotion status, and (3) compact summary message classification to eliminate noise from analytical views.

The codebase is well-positioned for this phase. The version detection infrastructure exists in `crates/server/src/watcher.rs` (the `check_version_change` function at line 375), the schema drift logging pipeline exists in `crates/store/src/drift.rs`, and the decomposition pipeline in `crates/store/src/decompose.rs` already routes overflow fields to `schema_drift_log`. The primary work is: a new migration (006), Rust-level changes to the decomposer for compact summary fields and extra_json on messages, new/enhanced query functions, updated API handlers and CLI output, and view recreation.

**Primary recommendation:** Structure as 3 plans following the established bottom-up pattern: (1) migration + backfill, (2) decomposer + query layer changes, (3) API + CLI enhancements.

## Standard Stack

### Core
| Library | Version | Purpose | Why Standard |
|---------|---------|---------|--------------|
| rusqlite | 0.37 | SQLite operations, migrations, queries | Already in workspace, pinned per decision [01-01] |
| tokio-rusqlite | 0.7 | Async SQLite bridge | Already in workspace, conn.call pattern established |
| serde/serde_json | 1.0 | JSON serialization for API responses | Already in workspace |
| axum | 0.8 | HTTP handler for enhanced endpoints | Already in workspace |
| clap | 4 | CLI argument parsing | Already in workspace |

### Supporting
| Library | Version | Purpose | When to Use |
|---------|---------|---------|-------------|
| tracing | 0.1 | Structured logging | Version change events, migration progress |

### Alternatives Considered
None — all dependencies are already in the workspace. No new crates needed for Phase 6.

## Architecture Patterns

### Recommended Project Structure
```
crates/store/
├── migrations/
│   └── 006_version_monitoring.sql    # New migration
├── src/
│   ├── schema.rs                     # Add migration 006 to MIGRATIONS array
│   ├── decompose.rs                  # Wire compact summary + extra_json on messages
│   ├── drift.rs                      # No changes needed (already captures version)
│   ├── query.rs                      # Enhanced version_history() + new drift grouped query
│   └── lib.rs                        # No changes needed
crates/server/
├── src/
│   ├── watcher.rs                    # Persist to version_history on version change
│   ├── api/schema.rs                 # Enhanced versions + drift handlers
│   ├── main.rs                       # Enhanced run_version_check + run_schema_drift
│   └── output.rs                     # Enhanced CLI formatting
```

### Pattern 1: Migration with Backfill
**What:** Migration 006 creates new table and columns, then backfills data from existing tables in the same DDL script.
**When to use:** When adding infrastructure that depends on already-ingested data.
**Example:**
```sql
-- Create version_history table (NOT schema_versions — that name is taken by migration tracker)
CREATE TABLE IF NOT EXISTS version_history (
    version         TEXT PRIMARY KEY,
    first_seen_at   TEXT NOT NULL,
    last_seen_at    TEXT NOT NULL,
    session_id      TEXT,          -- first session with this version
    session_count   INTEGER NOT NULL DEFAULT 0,
    new_fields_count INTEGER NOT NULL DEFAULT 0
);

-- Backfill from sessions table
INSERT OR IGNORE INTO version_history (version, first_seen_at, last_seen_at, session_id, session_count)
SELECT
    version,
    MIN(first_seen_at),
    MAX(COALESCE(last_seen_at, first_seen_at)),
    (SELECT session_id FROM sessions s2 WHERE s2.version = sessions.version
     ORDER BY first_seen_at ASC LIMIT 1),
    COUNT(*)
FROM sessions
WHERE version IS NOT NULL AND version != ''
GROUP BY version;
```

### Pattern 2: Column Promotion from Overflow
**What:** Add real columns for semantically important overflow fields, plus an extra_json column for residual overflow.
**When to use:** When overflow fields have proven analytical value (filtering, grouping, WHERE clauses).
**Example:**
```sql
-- Promote compact summary fields to real columns on messages
ALTER TABLE messages ADD COLUMN is_compact_summary INTEGER DEFAULT 0;
ALTER TABLE messages ADD COLUMN source_tool_use_id TEXT;
ALTER TABLE messages ADD COLUMN extra_json TEXT;

-- Backfill from schema_drift_log evidence (approximate: marks messages from
-- sessions where isCompactSummary was detected)
-- NOTE: Precise backfill may require re-parsing JSONL files since overflow
-- data is only sampled in schema_drift_log, not stored per-row.
```

### Pattern 3: View Recreation with Compact Summary Filtering
**What:** DROP and CREATE views to add `WHERE m.is_compact_summary = 0` clauses.
**When to use:** When adding a filter that changes the semantics of existing views.
**Example:**
```sql
-- Must DROP then CREATE because SQLite does not support CREATE OR REPLACE VIEW
DROP VIEW IF EXISTS v_project_summary;
CREATE VIEW v_project_summary AS
SELECT ...
FROM sessions s
LEFT JOIN messages m ON m.session_id = s.session_id
    AND m.is_compact_summary = 0   -- Exclude synthetic compact summary messages
...
```

### Pattern 4: Version Persistence in Watcher Loop
**What:** Extend `check_version_change` to INSERT/UPDATE `version_history` table alongside SSE event emission.
**When to use:** When the watcher detects a new version string.
**Example (Rust pseudocode):**
```rust
// Inside check_version_change, after detecting a version change:
conn.call(move |conn| {
    conn.execute(
        "INSERT INTO version_history (version, first_seen_at, last_seen_at, session_id, session_count)
         VALUES (?1, ?2, ?2, ?3, 1)
         ON CONFLICT(version) DO UPDATE SET
           last_seen_at = MAX(version_history.last_seen_at, excluded.last_seen_at),
           session_count = version_history.session_count + 1",
        params![new_version, timestamp, session_id],
    )
}).await;
```

### Anti-Patterns to Avoid
- **Naming collision with schema_versions:** The existing `schema_versions` table (created in `schema.rs` bootstrap, not in any migration file) tracks applied migrations. The new version-tracking table MUST use a different name. `version_history` is the recommended name.
- **Backfill from schema_drift_log sample_value for per-row compact summary marking:** `schema_drift_log` records one sample per (field, record_type, version) — not per message. True per-row backfill of `is_compact_summary` on existing messages would require re-reading the JSONL files. The migration should mark what it can from available data and rely on the decomposer for future rows.
- **Modifying views in-place:** SQLite does not support `CREATE OR REPLACE VIEW`. Always `DROP VIEW IF EXISTS` then `CREATE VIEW`.
- **Periodic polling for version changes:** The CONTEXT decision explicitly rejects a separate polling loop. Version detection is ingestion-triggered via the existing watcher flow.

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| Migration tracking | Custom version tracking | Existing `schema.rs` MIGRATIONS array + schema_versions table | Proven idempotent pattern through 5 prior migrations |
| Overflow capture | Per-record manual JSON extraction | Existing `serde(flatten)` + `drift::log_overflow()` | Already handles all record types with deduplication |
| View recreation | Manual ALTER VIEW (not supported) | DROP + CREATE pattern | SQLite limitation — established in codebase |
| Session-version correlation | Manual join queries | `version_history` table with session_count column | Precomputed aggregation avoids expensive runtime JOINs |

**Key insight:** The codebase already has all the moving parts — Phase 6 is primarily wiring existing infrastructure together (watcher version detection -> new persistence table, overflow fields -> enriched presentation) rather than building new subsystems.

## Common Pitfalls

### Pitfall 1: schema_versions Name Collision
**What goes wrong:** Creating a `schema_versions` table for version tracking when that name is already used for migration tracking.
**Why it happens:** The migration tracker table `schema_versions` is created in `schema.rs` `run_migrations()` function, not in any migration SQL file, making it easy to miss.
**How to avoid:** Name the new table `version_history`. Verified: `schema_versions` is created at `crates/store/src/schema.rs:35` in the `run_migrations()` bootstrap code.
**Warning signs:** Migration SQL fails with "table already exists" error.

### Pitfall 2: Backfill Precision for is_compact_summary
**What goes wrong:** Attempting to precisely mark every existing compact summary message during migration backfill, but `schema_drift_log` only stores sample values per (field, record_type, version), not per message row.
**Why it happens:** The overflow capture is designed for drift detection (one sample per field), not for per-row data recovery.
**How to avoid:** The migration backfill should be pragmatic: mark `is_compact_summary = 0` as default for all existing rows (safe default). For rows where compact summary data can be inferred (e.g., if a future re-sync occurs, the decomposer will set it correctly on new/updated rows), rely on the enhanced decomposer. Alternatively, a one-time re-parse pass could be added as a CLI command.
**Warning signs:** Analytical views still include some undetected compact summary messages in counts.

### Pitfall 3: View Dependencies During Migration
**What goes wrong:** DROP/CREATE VIEW fails because another view depends on the dropped view, or the new view references a column that hasn't been added yet.
**Why it happens:** SQLite evaluates views lazily (on query), so DROP succeeds even with dependents, but the dependent views then fail at query time.
**How to avoid:** Order operations in migration: (1) ALTER TABLE to add columns, (2) backfill data, (3) DROP all views, (4) CREATE all views with updated definitions. All seven views should be recreated in a single migration to keep them consistent.
**Warning signs:** Queries against views fail with "no such column" after migration.

### Pitfall 4: Watcher State and version_history Table Sync
**What goes wrong:** The watcher detects a version change and fires the SSE event but fails to persist to `version_history`, or persists but the session_count is incorrect.
**Why it happens:** The `check_version_change` function currently only updates in-memory `WatcherState.last_known_version` and fires an SSE event. Adding DB persistence introduces a new failure mode.
**How to avoid:** Persist to `version_history` inside the existing `check_version_change` function, using the same `conn` reference. Use INSERT ... ON CONFLICT for idempotency. Log persistence failures at warn level but do not skip the SSE event.
**Warning signs:** `version_history` table has fewer versions than observed via SSE events.

### Pitfall 5: extra_json Overflow Extraction Order in Decomposer
**What goes wrong:** The decomposer extracts known fields (isCompactSummary, sourceToolUseID) from the overflow HashMap, but the remaining overflow also needs to go to `extra_json`. If extraction and serialization happen in wrong order, fields are duplicated or lost.
**Why it happens:** The overflow HashMap is consumed during decomposition. Fields need to be extracted before the remaining map is serialized to `extra_json`.
**How to avoid:** In `decompose_user`: (1) extract `is_compact_summary` from `r.overflow`, (2) extract `source_tool_use_id` from `r.overflow`, (3) serialize remaining `r.overflow` as extra_json. Use `HashMap::remove()` to extract known keys, then serialize the remainder.
**Warning signs:** Known fields appear both as real columns AND inside extra_json.

### Pitfall 6: version_history new_fields_count Computation
**What goes wrong:** The `new_fields_count` column on `version_history` should count drift fields introduced with that version, but `schema_drift_log` doesn't track "introduction" vs. "re-observation" across versions.
**Why it happens:** `schema_drift_log` has UNIQUE(field_name, record_type, version), so the same field can appear in multiple versions. A field is "new" if it first appeared in that version (no earlier version has it).
**How to avoid:** Compute `new_fields_count` as a correlated subquery or post-hoc update: count fields in `schema_drift_log` for this version that don't exist in any earlier version's entries. This should be a backfill query in the migration, and an incremental update in the watcher.
**Warning signs:** `new_fields_count` is always 0 or matches total drift count for the version.

## Code Examples

Verified patterns from the existing codebase:

### Migration 006: version_history Table + Messages Columns + View Recreation
```sql
-- Migration 006: Version monitoring + compact summary absorption

-- 1. version_history table
CREATE TABLE IF NOT EXISTS version_history (
    version          TEXT PRIMARY KEY,
    first_seen_at    TEXT NOT NULL,
    last_seen_at     TEXT NOT NULL,
    session_id       TEXT,
    session_count    INTEGER NOT NULL DEFAULT 0,
    new_fields_count INTEGER NOT NULL DEFAULT 0
);

-- Backfill version_history from sessions
INSERT OR IGNORE INTO version_history (version, first_seen_at, last_seen_at, session_id, session_count)
SELECT
    version,
    MIN(first_seen_at),
    MAX(COALESCE(last_seen_at, first_seen_at)),
    (SELECT s2.session_id FROM sessions s2 WHERE s2.version = sessions.version
     ORDER BY s2.first_seen_at ASC LIMIT 1),
    COUNT(*)
FROM sessions
WHERE version IS NOT NULL AND version != ''
GROUP BY version;

-- Backfill new_fields_count from schema_drift_log
UPDATE version_history SET new_fields_count = (
    SELECT COUNT(DISTINCT d.field_name || ':' || d.record_type)
    FROM schema_drift_log d
    WHERE d.version = version_history.version
      AND NOT EXISTS (
          SELECT 1 FROM schema_drift_log d2
          WHERE d2.field_name = d.field_name
            AND d2.record_type = d.record_type
            AND d2.version != d.version
            AND d2.first_seen_at < d.first_seen_at
      )
);

-- 2. New columns on messages table
ALTER TABLE messages ADD COLUMN is_compact_summary INTEGER DEFAULT 0;
ALTER TABLE messages ADD COLUMN source_tool_use_id TEXT;
ALTER TABLE messages ADD COLUMN extra_json TEXT;

-- 3. Recreate views with compact summary filter
-- (DROP then CREATE for all 7 views)
```

### Decomposer: Compact Summary Field Extraction
```rust
// In decompose_user(), after inserting the message row:
// Extract known overflow keys -> real columns
let is_compact = r.overflow.get("isCompactSummary")
    .and_then(|v| v.as_bool())
    .unwrap_or(false);
let source_tool_id = r.overflow.get("sourceToolUseID")
    .and_then(|v| v.as_str())
    .map(|s| s.to_string());

// Build extra_json from remaining overflow (excluding promoted keys)
let mut remaining = r.overflow.clone();
remaining.remove("isCompactSummary");
remaining.remove("sourceToolUseID");
// Also remove other known-but-unpromoted fields if desired
let extra_json = if remaining.is_empty() {
    None
} else {
    Some(serde_json::to_string(&remaining)?)
};

// UPDATE the message row with the extracted values
tx.execute(
    "UPDATE messages SET is_compact_summary = ?1, source_tool_use_id = ?2, extra_json = ?3
     WHERE uuid = ?4",
    params![is_compact as i32, source_tool_id, extra_json, r.base.uuid],
)?;
```

### Watcher: Version Persistence
```rust
// In check_version_change(), after SSE event emission:
let ver = new_version.clone();
let sid = session_id.to_string();
let ts = /* timestamp from session or current time */;
let persist_result = conn.call(move |conn| {
    conn.execute(
        "INSERT INTO version_history (version, first_seen_at, last_seen_at, session_id, session_count)
         VALUES (?1, ?2, ?2, ?3, 1)
         ON CONFLICT(version) DO UPDATE SET
           last_seen_at = MAX(version_history.last_seen_at, excluded.last_seen_at),
           session_count = version_history.session_count + 1",
        rusqlite::params![ver, ts, sid],
    )
}).await;

if let Err(e) = persist_result {
    tracing::warn!(error = %e, "Failed to persist version to version_history table");
}
```

### Enhanced Query: Version History with Session Count
```rust
pub fn version_history_enhanced(conn: &Connection) -> Result<Vec<VersionHistoryEntry>, rusqlite::Error> {
    let mut stmt = conn.prepare(
        "SELECT version, first_seen_at, last_seen_at, session_id, session_count, new_fields_count
         FROM version_history
         ORDER BY first_seen_at ASC"
    )?;
    // ... row mapping
}
```

### Enhanced Query: Drift Grouped by Version
```rust
pub fn drift_by_version(conn: &Connection) -> Result<Vec<VersionDriftGroup>, rusqlite::Error> {
    let mut stmt = conn.prepare(
        "SELECT d.version, d.record_type, d.field_name, d.sample_value, d.first_seen_at,
                COUNT(*) OVER (PARTITION BY d.version, d.field_name, d.record_type) as occurrence_count
         FROM schema_drift_log d
         WHERE d.version IS NOT NULL
         ORDER BY d.version, d.record_type, d.field_name"
    )?;
    // Group results in Rust by version -> record_type -> fields
}
```

## State of the Art

| Old Approach | Current Approach | When Changed | Impact |
|--------------|------------------|--------------|--------|
| Version from messages table GROUP BY | Dedicated version_history table with precomputed counts | Phase 6 | Eliminates expensive GROUP BY on every version-check query |
| Flat drift list (no version grouping) | Drift grouped by version with promotion status | Phase 6 | Actionable version-correlated drift analysis |
| Compact summaries treated as real messages | is_compact_summary column + view filtering | Phase 6 | Accurate message counts and token attribution |
| Version detection via CLI/npm probe | Ingestion-only detection from sessions.version | Phase 6 (CONTEXT decision) | Simpler, no external process dependency |

**Deprecated/outdated:**
- The existing `query::version_history()` function queries `messages` table with `GROUP BY version`. Phase 6 replaces this with a query against the dedicated `version_history` table. The old function should be updated, not removed, since the messages-based approach is the fallback if version_history hasn't been populated yet.
- The current CLI `run_version_check` output format (3-column table) will be enhanced with session_count and new_fields_count columns.

## Open Questions

1. **JSONL Re-parse for Backfill Accuracy**
   - What we know: The migration can add `is_compact_summary = 0` as default for all existing rows. Future ingestion will set it correctly via the enhanced decomposer.
   - What's unclear: Whether a one-time re-parse of all JSONL files is needed for accurate retroactive classification of existing compact summary messages. The user may want a `claude-history sync --force` or `--rebuild` flag.
   - Recommendation: Implement the migration with safe defaults (all existing rows get `is_compact_summary = 0`). Add a `--rebuild` flag to the `sync` command that forces re-read from byte offset 0. This is a low-risk addition that enables accurate backfill without complicating the migration.

2. **Promotion Status Determination**
   - What we know: A drift field's promotion status should indicate whether it has been promoted to a real column ('promoted'), captured in extra_json ('extra_json'), or remains unhandled overflow ('unhandled').
   - What's unclear: Whether promotion status should be computed dynamically from schema introspection or stored as a column in schema_drift_log.
   - Recommendation: Compute dynamically at query time. The query can check if the field_name matches a column in the relevant table (via PRAGMA table_info). This avoids maintaining a status column that could go stale. The query function builds a set of known columns from the schema and classifies each drift field accordingly.

3. **Occurrence Count for Drift Fields**
   - What we know: The CONTEXT specifies each drift field should include an "occurrence count." The current schema_drift_log uses UNIQUE(field_name, record_type, version) with INSERT OR IGNORE, so there's no per-row occurrence count.
   - What's unclear: Whether adding an `occurrence_count` column to schema_drift_log (incremented on duplicate) is needed, or if the count of 1 per unique combination is sufficient.
   - Recommendation: Add an `occurrence_count INTEGER DEFAULT 1` column to schema_drift_log in migration 006, and change the INSERT from `INSERT OR IGNORE` to `INSERT ... ON CONFLICT DO UPDATE SET occurrence_count = occurrence_count + 1, last_seen_at = datetime('now')`. This requires a small change to `drift.rs::log_overflow()`. This gives actual frequency data without the deduplication overhead concern (the UNIQUE constraint already limits rows).

## Sources

### Primary (HIGH confidence)
- `/Users/david/Projects/cc-history-api/crates/server/src/watcher.rs` — Existing check_version_change function (line 375), watcher loop architecture, SSE event emission pattern
- `/Users/david/Projects/cc-history-api/crates/store/src/drift.rs` — Schema drift logging with UNIQUE constraint dedup, overflow capture pattern
- `/Users/david/Projects/cc-history-api/crates/store/src/decompose.rs` — Record decomposition pipeline, INSERT OR IGNORE pattern, upsert patterns
- `/Users/david/Projects/cc-history-api/crates/store/src/schema.rs` — Migration runner, MIGRATIONS array, schema_versions bootstrap
- `/Users/david/Projects/cc-history-api/crates/store/src/query.rs` — Existing version_history() and schema_drift_list() query functions
- `/Users/david/Projects/cc-history-api/crates/server/src/api/schema.rs` — Existing /v1/schema/versions and /v1/schema/drift handlers
- `/Users/david/Projects/cc-history-api/crates/server/src/main.rs` — CLI subcommand structure, run_version_check and run_schema_drift implementations
- `/Users/david/Projects/cc-history-api/crates/store/migrations/001_initial.sql` — schema_drift_log DDL, messages table DDL
- `/Users/david/Projects/cc-history-api/crates/store/migrations/004_modeling.sql` — View creation pattern, backfill pattern, ALTER TABLE pattern
- `/Users/david/Projects/cc-history-api/.planning/audit/milestone-2-spec.md` — Compact summary discovery documentation, proposed fix specification

### Secondary (MEDIUM confidence)
- `/Users/david/Projects/cc-history-api/crates/core/src/record.rs` — UserRecord overflow HashMap captures isCompactSummary, isVisibleInTranscriptOnly, sourceToolUseID
- `/Users/david/Projects/cc-history-api/crates/server/src/events.rs` — SseEvent::VersionChanged variant structure

## Metadata

**Confidence breakdown:**
- Standard stack: HIGH — all dependencies already in workspace, no new crates needed
- Architecture: HIGH — all patterns are extensions of existing code (migration, decomposer, query, handler patterns all established in Phases 1-5)
- Pitfalls: HIGH — identified from direct code inspection of the codebase, not external sources; naming collision with schema_versions is the highest-risk pitfall

**Research date:** 2026-02-21
**Valid until:** indefinite — this is internal codebase research, not external dependency research
