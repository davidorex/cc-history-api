# Milestone 2: Modeling Layer + Cross-Domain Intelligence

## Premise

Extraction and storage are done. 11 normalized SQLite tables (originally 13 — progress_events and queue_operations dropped in m2-p10 as zero-value bloat) hold every message, content block, token usage record, tool execution, file operation, and git operation from Claude Code's JSONL history. The schema is sound, the indexes exist, the foreign keys link everything.

What's missing is a layer that lets consumers ask questions across those tables without writing Rust code.

## Architecture

```
~/.claude/projects/**/*.jsonl
    ↓ extraction (milestone 1, done, don't touch)
Normalized SQLite (11 tables, WAL, FTS5)
    ↓
SQLite views (cross-domain relationships, materialized at migration time)
    ↓
POST /v1/sql (read-only parameterized passthrough)
    ↓
Consumers: HTTP clients, CLI, MCP tools, dashboards, LLM agents
```

## What gets built

### 1. Projects table (migration)

```sql
CREATE TABLE projects (
    project_path  TEXT PRIMARY KEY,
    display_name  TEXT,
    first_seen    TEXT NOT NULL,
    last_seen     TEXT NOT NULL,
    session_count INTEGER NOT NULL DEFAULT 0
);
```

Backfilled from `SELECT DISTINCT project_path FROM sessions`. Updated during sync. Every session links to a project.

### 2. result_summary and is_error on artifact tables (migration)

```sql
ALTER TABLE file_operations ADD COLUMN result_summary TEXT;
ALTER TABLE file_operations ADD COLUMN is_error INTEGER DEFAULT 0;
ALTER TABLE git_operations ADD COLUMN result_summary TEXT;
ALTER TABLE git_operations ADD COLUMN is_error INTEGER DEFAULT 0;
```

Backfilled from tool_executions via tool_use_id JOIN.

### 3. SQLite views (migration)

Views are the modeling layer. They express cross-domain relationships as queryable entities. Any consumer — HTTP, CLI, MCP, raw sqlite3 — can SELECT from them.

```sql
-- Per-file token cost attribution across all sessions
CREATE VIEW v_file_token_cost AS
SELECT
    s.project_path,
    fo.file_path,
    fo.operation_type,
    COUNT(*) as operation_count,
    COALESCE(SUM(tu.input_tokens), 0) as input_tokens,
    COALESCE(SUM(tu.output_tokens), 0) as output_tokens,
    COALESCE(SUM(tu.input_tokens + tu.output_tokens), 0) as total_tokens
FROM file_operations fo
JOIN messages m ON fo.message_uuid = m.uuid
JOIN sessions s ON m.session_id = s.session_id
LEFT JOIN token_usage tu ON m.uuid = tu.message_uuid
GROUP BY s.project_path, fo.file_path, fo.operation_type;

-- Conversation context around file mutations
-- Each row is an assistant text block adjacent to a file operation
CREATE VIEW v_file_conversation_context AS
SELECT
    fo.file_path,
    fo.operation_type,
    fo.timestamp as operation_timestamp,
    fo.session_id,
    s.project_path,
    m_asst.uuid as assistant_message_uuid,
    mc.text_content as assistant_reasoning,
    m_asst.timestamp as reasoning_timestamp,
    m_asst.model
FROM file_operations fo
JOIN messages m_op ON fo.message_uuid = m_op.uuid
JOIN messages m_asst ON m_asst.session_id = m_op.session_id
    AND m_asst.type = 'assistant'
    AND m_asst.timestamp <= m_op.timestamp
    AND m_asst.timestamp >= datetime(m_op.timestamp, '-60 seconds')
JOIN message_content mc ON mc.message_uuid = m_asst.uuid
    AND mc.block_type = 'text'
JOIN sessions s ON fo.session_id = s.session_id
WHERE fo.operation_type IN ('write', 'edit');

-- Project activity summary
CREATE VIEW v_project_summary AS
SELECT
    s.project_path,
    COUNT(DISTINCT s.session_id) as session_count,
    COUNT(DISTINCT m.uuid) as message_count,
    COALESCE(SUM(tu.input_tokens + tu.output_tokens), 0) as total_tokens,
    COUNT(DISTINCT fo.id) as file_operations,
    COUNT(DISTINCT go2.id) as git_operations,
    MIN(s.first_seen_at) as first_activity,
    MAX(m.timestamp) as last_activity
FROM sessions s
LEFT JOIN messages m ON m.session_id = s.session_id
LEFT JOIN token_usage tu ON tu.message_uuid = m.uuid
LEFT JOIN file_operations fo ON fo.session_id = s.session_id
LEFT JOIN git_operations go2 ON go2.session_id = s.session_id
GROUP BY s.project_path;

-- File provenance: complete operation history across sessions
CREATE VIEW v_file_provenance AS
SELECT
    fo.file_path,
    fo.operation_type,
    fo.timestamp,
    fo.session_id,
    s.project_path,
    fo.content,
    fo.old_content,
    fo.command,
    fo.result_summary,
    fo.is_error,
    fo.message_uuid,
    m.type as message_type
FROM file_operations fo
JOIN messages m ON fo.message_uuid = m.uuid
JOIN sessions s ON fo.session_id = s.session_id
ORDER BY fo.file_path, fo.timestamp;

-- Git commit context: commit message + surrounding conversation
CREATE VIEW v_git_commit_context AS
SELECT
    go2.commit_message,
    go2.branch,
    go2.timestamp as commit_timestamp,
    go2.session_id,
    s.project_path,
    mc.text_content as assistant_reasoning,
    m_asst.timestamp as reasoning_timestamp
FROM git_operations go2
JOIN messages m_commit ON go2.message_uuid = m_commit.uuid
JOIN messages m_asst ON m_asst.session_id = m_commit.session_id
    AND m_asst.type = 'assistant'
    AND m_asst.timestamp <= m_commit.timestamp
    AND m_asst.timestamp >= datetime(m_commit.timestamp, '-120 seconds')
JOIN message_content mc ON mc.message_uuid = m_asst.uuid
    AND mc.block_type = 'text'
JOIN sessions s ON go2.session_id = s.session_id
WHERE go2.operation_type = 'commit';

-- Tool error patterns with conversation context
CREATE VIEW v_tool_errors AS
SELECT
    te.tool_name,
    te.is_error,
    SUBSTR(te.result_content, 1, 500) as error_content,
    te.message_uuid,
    m.session_id,
    s.project_path,
    m.timestamp,
    te.input_json
FROM tool_executions te
JOIN messages m ON te.message_uuid = m.uuid
JOIN sessions s ON m.session_id = s.session_id
WHERE te.is_error = 1;

-- Session cost breakdown
CREATE VIEW v_session_cost AS
SELECT
    s.session_id,
    s.project_path,
    s.first_seen_at,
    COUNT(DISTINCT m.uuid) as message_count,
    COALESCE(SUM(tu.input_tokens), 0) as input_tokens,
    COALESCE(SUM(tu.output_tokens), 0) as output_tokens,
    COALESCE(SUM(tu.cache_read_input_tokens), 0) as cache_read_tokens,
    COUNT(DISTINCT fo.id) as file_ops,
    COUNT(DISTINCT go2.id) as git_ops
FROM sessions s
LEFT JOIN messages m ON m.session_id = s.session_id
LEFT JOIN token_usage tu ON tu.message_uuid = m.uuid
LEFT JOIN file_operations fo ON fo.session_id = s.session_id
LEFT JOIN git_operations go2 ON go2.session_id = s.session_id
GROUP BY s.session_id;
```

### 4. POST /v1/sql endpoint

Read-only parameterized SQL passthrough.

```
POST /v1/sql
Content-Type: application/json

{
  "query": "SELECT * FROM v_file_token_cost WHERE project_path LIKE ?1 ORDER BY total_tokens DESC LIMIT ?2",
  "params": ["%vocab%", 10]
}
```

Response: JSON array of row objects.

Constraints:
- SELECT only. Reject INSERT/UPDATE/DELETE/DROP/ALTER/CREATE/ATTACH.
- Statement is prepared and executed via rusqlite with parameter binding.
- Query timeout (e.g. 5 seconds) to prevent runaway joins.
- Consumers can query base tables, views, and FTS virtual tables.

### 5. GET /v1/schema endpoint (enhanced)

Returns the full schema: tables, columns, types, foreign keys, views, indexes. Consumers (including LLM agents) read this to understand what's queryable.

```json
{
  "tables": [
    {"name": "sessions", "columns": [...], "foreign_keys": [...]},
    ...
  ],
  "views": [
    {"name": "v_file_token_cost", "columns": [...], "description": "Per-file token cost attribution"},
    ...
  ]
}
```

### 6. GET /v1/projects and GET /v1/projects/{path}

```
GET /v1/projects
→ [{ project_path, display_name, session_count, first_seen, last_seen }]

GET /v1/projects/{path}
→ { ...project, summary from v_project_summary }
```

## What the examples from earlier now look like

**"For project vocab, show me the file that's cost me the most in tokens"**
```sql
SELECT file_path, SUM(total_tokens) as cost
FROM v_file_token_cost
WHERE project_path LIKE '%vocab%'
GROUP BY file_path
ORDER BY cost DESC
LIMIT 1
```

**"What are the 3 most recent substantive agent responses in project X having to do with file z"**
```sql
SELECT assistant_reasoning, reasoning_timestamp, operation_type, file_path
FROM v_file_conversation_context
WHERE project_path LIKE '%project_x%'
  AND file_path LIKE '%file_z%'
ORDER BY reasoning_timestamp DESC
LIMIT 3
```

**"Show the provenance of file x from first write to current state"**
```sql
SELECT operation_type, timestamp, session_id, project_path,
       SUBSTR(content, 1, 200) as content_preview,
       SUBSTR(old_content, 1, 200) as old_content_preview,
       command, result_summary, is_error
FROM v_file_provenance
WHERE file_path LIKE '%file_x%'
ORDER BY timestamp ASC
```

**"What commits were made in project X and what was Claude thinking?"**
```sql
SELECT commit_message, branch, commit_timestamp,
       SUBSTR(assistant_reasoning, 1, 500) as reasoning
FROM v_git_commit_context
WHERE project_path LIKE '%project_x%'
ORDER BY commit_timestamp DESC
LIMIT 10
```

## What does NOT change

- Extraction layer (crates/core, crates/store ingestion pipeline)
- Existing 28 HTTP endpoints (they stay as convenience shortcuts)
- Existing 14 CLI subcommands
- SSE events
- File watcher
- Sync engine
- FTS5 indexes

## Post-implementation findings

### Noise tables dropped (m2-p10, implemented)

Semantic value assessment of ingested record types revealed that
`progress_events` (agent_progress, bash_progress, hook_progress,
mcp_progress) and `queue_operations` (enqueue, dequeue, remove, popAll)
carried zero to low semantic value while accounting for ~70% of database
size (~4.25GB of 6.0GB). Migration 005 drops both tables. The decomposer
still parses these records for session upsert, project upsert, and drift
logging — only the blob storage is eliminated.

Result: 6.0GB → 1.6GB (73% reduction).

### Compact summary messages — unmodeled message class (gap, not yet addressed)

Schema drift detection surfaced fields on user and system records that
indicate Claude Code's context compression pipeline injects synthetic
messages into the JSONL stream:

| Field | Record type | Meaning |
|-------|-------------|---------|
| `isCompactSummary` | user | Marks message as auto-generated summary, not a real user prompt |
| `isVisibleInTranscriptOnly` | user | Message exists only for transcript display, not sent to API |
| `sourceToolUseID` | user | Links compact summary back to the original tool_use it replaced |
| `compactMetadata` | system | Context window state at compression time (`preTokens`, `trigger`) |
| `logicalParentUuid` | system | Links system record to the original parent message |

**Impact on current schema:** These synthetic messages are ingested as
regular user messages. Every analytical view that counts messages, sums
tokens, or joins on message type includes them without distinction. This
inflates message counts and may skew token attribution in `v_project_summary`,
`v_session_cost`, and FTS search results.

**Proposed fix (next phase):**

1. Add `is_compact_summary INTEGER DEFAULT 0` and `source_tool_use_id TEXT`
   as real columns on `messages` — these affect query semantics (filtering,
   grouping) and warrant first-class indexed columns, not JSON blobs.

2. Add `extra_json TEXT` on `messages` for residual overflow fields that
   are informational-only (`container`, `context_management`, future
   unknowns). This follows the pattern already proven on `system_events`
   and `token_usage`.

3. Wire `decompose_user` and `decompose_assistant` to populate both the
   real columns (from known overflow keys) and extra_json (remaining
   overflow). This is a one-time Rust change — after which adapting to
   future drift fields is SQL-only (promote from extra_json to real
   column via ALTER TABLE migration when a field proves semantically
   important).

4. Update analytical views to add `WHERE m.is_compact_summary = 0` (or
   expose the column so consumers can filter explicitly).

### Additional drift fields detected (informational)

API response metadata (`type`, `container`, `context_management` on
assistant messages; `server_tool_use`, `inference_geo` on usage stats),
error/retry fields (`retryAttempt`, `maxRetries`, `retryInMs`, `error`
on system records), and hook/agent linkage (`toolUseID`,
`parentToolUseID` on progress records) are captured in
`schema_drift_log` but not yet stored in queryable columns. These are
candidates for the extra_json residual catch-all.

## Audit fixes absorbed

The Phase 2/3/5 audit findings about composable query bodies become less critical — the SQL passthrough handles everything the composable query compilers were supposed to. The audit fixes for SSE payload shapes (Phase 4) and missing schema columns (Phase 1/5) are still needed and are included in the migrations above.

## Deliverables

1. Migration 004: projects table + artifact columns + views
2. Migration 005: drop progress_events and queue_operations (noise tables)
3. POST /v1/sql endpoint with safety validation
4. GET /v1/schema (enhanced with views and descriptions)
5. GET /v1/projects, GET /v1/projects/{path}
6. Schema documentation (table/view descriptions for consumers)
