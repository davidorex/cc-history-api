## Practical atomic next steps — promoting `user.planContent`

Each is independently ship-able. Listed by dependency order; some can ship in any order once the prerequisite column exists.

### 1. Migration 007: add `messages.plan_content` column + backfill

The column promotion itself, mirroring the migration-006 pattern for `is_compact_summary`.

```sql
-- migrations/007_promote_plan_content.sql
ALTER TABLE messages ADD COLUMN plan_content TEXT;

-- Backfill from existing extra_json — source data already in row, no JSONL re-parse needed
UPDATE messages
SET plan_content = json_extract(extra_json, '$.planContent'),
    extra_json   = json_remove(extra_json, '$.planContent')
WHERE type = 'user'
  AND json_extract(extra_json, '$.planContent') IS NOT NULL;

-- Optional NULL-vs-empty cleanup: collapse '{}' extra_json to NULL for tidiness
UPDATE messages SET extra_json = NULL WHERE extra_json = '{}';

CREATE INDEX IF NOT EXISTS idx_messages_plan_content_present
  ON messages(session_id) WHERE plan_content IS NOT NULL;
```

- **Touches**: `crates/store/migrations/007_promote_plan_content.sql`, `crates/store/src/schema.rs` (add to MIGRATIONS array), tests in `schema.rs`
- **Closes**: data is queryable via a real column. SQL filters like `WHERE plan_content IS NOT NULL` become trivial, no JSON extraction. The 81 existing plans backfill in one statement.
- **Risk**: low. ALTER TABLE ADD COLUMN is non-destructive on SQLite. Backfill is idempotent (UPDATE with predicate).
- **LOC**: ~25

### 2. Decomposer extraction in `decompose_user`

Without this, future ingestion still puts `planContent` in `extra_json` instead of the new column. Pattern is identical to the existing extraction at `crates/store/src/decompose.rs:409-432`.

```rust
// In decompose_user, after the existing isCompactSummary/sourceToolUseID extraction:
let plan_content = r.overflow.get("planContent")
    .and_then(|v| v.as_str())
    .map(|s| s.to_string());

let mut remaining = r.overflow.clone();
remaining.remove("isCompactSummary");
remaining.remove("sourceToolUseID");
remaining.remove("planContent");          // NEW
let extra_json = if remaining.is_empty() { None } else {
    Some(serde_json::to_string(&remaining)?)
};

tx.execute(
    "UPDATE messages SET is_compact_summary = ?1, source_tool_use_id = ?2, plan_content = ?3, extra_json = ?4 WHERE uuid = ?5",
    rusqlite::params![is_compact as i32, source_tool_id, plan_content, extra_json, r.base.uuid],
)?;
```

- **Touches**: `crates/store/src/decompose.rs` `decompose_user` and tests
- **Closes**: ingestion path. After this, every new plan-mode session has its planContent populated in the real column on first ingestion, not duplicated into extra_json
- **Risk**: low. Drift logging at the bottom of `decompose_user` continues to receive the original `r.overflow` unchanged, so drift detection is unaffected
- **LOC**: ~15
- **Depends on**: step 1

### 3. FTS5 coverage for plan content

Without this, `claude-history search "context engine"` still cannot find phrases that exist only in plan markdown. Two sub-options; pick one.

**3a. Synthetic `message_content` rows (recommended)** — reuses the existing `fts_message_content` index. After insertion of a user record carrying planContent, additionally insert a content-block row with `block_type = 'plan_content'` and `text_content = plan_content`.

```rust
// In decompose_user, after the UPDATE:
if let Some(ref plan) = plan_content {
    tx.execute(
        "INSERT OR IGNORE INTO message_content
         (message_uuid, block_index, block_type, text_content)
         VALUES (?1, -1, 'plan_content', ?2)",
        rusqlite::params![r.base.uuid, plan],
    )?;
}
```

- Block index `-1` (sentinel) avoids collision with real block indices `0..N`
- The existing FTS5 trigger or rebuild path picks this up automatically
- `claude-history search` works without handler changes
- A backfill in migration 007 inserts these synthetic rows for the 81 existing plans

**3b. Separate `fts_plan_content` table** — dedicated FTS5 virtual table over `messages(plan_content)`. Cleaner separation, but requires a new search handler and an extra rebuild path in the watcher.

- **Touches**: `crates/store/src/decompose.rs`, `crates/store/src/fts.rs` (rebuild path may need plan_content awareness), migration 007 (synthetic-row backfill)
- **Closes**: `search_messages` MCP tool and `claude-history search` CLI return plans matching the query, with snippets
- **Risk**: 3a is low (reuses existing pipeline). 3b is medium (new index, more code).
- **LOC**: 3a ~20; 3b ~80
- **Depends on**: steps 1 + 2

### 4. CLI surface — `claude-history plans` subcommand

Without a consumer surface, the column exists but is invisible to non-SQL users.

Two endpoints in one subcommand:

```
claude-history plans list   [--project P] [--since DATE] [--limit N]
claude-history plans show <session-id>
```

`list` outputs session_id, project, timestamp, plan length, first-line-of-plan-as-title; `show` prints the full plan markdown.

- **Touches**: `crates/server/src/main.rs` (add `Plans { ... }` subcommand), new `crates/server/src/output.rs` formatter for plans
- **Closes**: CLI users can browse and read plans without SQL
- **Risk**: low. Read-only.
- **LOC**: ~120
- **Depends on**: step 1

Optional companion: `--has-plan` filter on the existing `sessions` subcommand.

### 5. MCP surface — extend `query_messages` and add `list_plans` / `get_plan`

The MCP tool surface should know plans exist. Two changes:

- **Extend `query_messages`**: add a `has_plan: bool` param that filters to messages with non-NULL plan_content; include `plan_content` in the response when present
- **Add `list_plans`**: top-level MCP tool listing plans across projects with metadata (session_id, project, timestamp, length, title)
- **Add `get_plan`**: fetch full plan markdown by session_id or message_uuid

- **Touches**: `crates/server/src/mcp/tools.rs`, `crates/server/src/mcp/mod.rs` instructions, `crates/store/src/query.rs` (new query functions)
- **Closes**: MCP clients (Claude Desktop, Claude Code CLI sessions) can find and retrieve plans through the standard tool surface
- **Risk**: low. Additive, no breaking changes.
- **LOC**: ~150
- **Depends on**: step 1

### 6. REST surface — `/v1/plans` endpoints

For HTTP consumers and parity with MCP:

- `GET /v1/plans` — list with filters (project, date range, has_content_match)
- `GET /v1/plans/{session_id}` — full content
- `GET /v1/plans/search?q=...` — FTS5 search restricted to plans (depends on step 3)

- **Touches**: new `crates/server/src/api/plans.rs`, route registration in `crates/server/src/api/mod.rs:92`
- **Closes**: HTTP clients can browse and search plans
- **Risk**: low. Additive.
- **LOC**: ~180
- **Depends on**: step 1; search endpoint depends on step 3

### 7. Analytical view — `v_session_plans`

```sql
CREATE VIEW v_session_plans AS
SELECT
    s.session_id, s.project_path, s.first_seen_at,
    m.uuid AS message_uuid, m.timestamp,
    LENGTH(m.plan_content) AS plan_length,
    SUBSTR(m.plan_content, 1, INSTR(m.plan_content || char(10), char(10)) - 1) AS plan_title,
    m.plan_content
FROM messages m
JOIN sessions s ON s.session_id = m.session_id
WHERE m.plan_content IS NOT NULL;
```

- **Touches**: migration 008 (or include in migration 007)
- **Closes**: ad-hoc analytics via `SELECT * FROM v_session_plans WHERE project_path LIKE '%smak%'`; canned-query authors get a stable surface
- **Risk**: minimal. Views are cheap.
- **LOC**: ~15
- **Depends on**: step 1

### Recommended atomic shipping order

A single shippable batch that closes the visibility gap minimally is **steps 1 + 2 + 3a + 4** — column + extraction + FTS + CLI. After that batch, plans are queryable, indexed, and browsable.

If sequencing one commit at a time:
1. **Step 1** — migration + backfill (data is now in the right place)
2. **Step 2** — decomposer (future ingestion stays consistent)
3. **Step 3a** — FTS5 synthetic rows (search starts working)
4. **Step 6** — combined CLI subcommand `plans list` and `plans show`
5. **Step 5** — MCP tools (most clients live here)
6. **Step 7** — REST endpoints
7. **Step 8** — view (canned-query convenience)

### What is explicitly NOT recommended

- **Adding the column without the decomposer change** (steps 1 without 2) — leaves new ingestion writing to extra_json, so the column slowly becomes inconsistent: backfilled history is in plan_content, new plans are in extra_json. False sense of promotion. Negligent under mandate-004.
- **Adding a CLI/MCP/REST surface that reads from `extra_json` instead of the new column** — keeps the JSON extraction pattern alive in consumer code, doesn't actually promote anything. Same negligence.
- **Skipping FTS coverage** — leaves the most valuable consumer behavior (full-text search across plan markdown) broken even though the data is now in a real column. The ~80 plans are exactly the kind of dense, prose-heavy content that benefits most from FTS.
- **Documentation-only changes** ("you can `json_extract(extra_json, '$.planContent')` to get plans") — moves the burden onto every consumer. Doesn't address the structural gap.

### One-line invariants any of these changes must preserve

- Drift logging at the bottom of `decompose_user` continues to receive the original `r.overflow` (unchanged by promotion)
- `messages.extra_json` is `NULL` rather than `'{}'` when no overflow remains (consistent with current pattern)
- Backfill UPDATE in migration 007 is bounded by `WHERE … IS NOT NULL` so re-running the migration is a no-op