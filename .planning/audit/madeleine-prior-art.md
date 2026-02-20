# Madeleine Prior Art: Patterns for cc-history-api Evolution

Research audit examining `/Users/david/Projects/madeleine` (v1) and
`/Users/david/Projects/madeleine-core` (v2) for architectural patterns
applicable to cc-history-api's next evolution beyond domain-scoped endpoints.

Date: 2026-02-21

---

## 1. Project Overview

### madeleine (v1)

- **Stack**: Python 3.10+, Click CLI, SQLite, `python-to-mcp` for MCP server
- **Purpose**: "Memory extraction and session recovery for Claude conversations"
  -- an application written *for* LLM coding agents. The LLM is the primary user.
- **Database**: `~/.proust/madeleine/conversations.db` with 7 tables: `sessions`,
  `turns`, `turn_usage`, `tool_uses`, `file_operations`, `todos`, `todo_state_changes`
- **Key innovation**: Plugin architecture with `@register_query` decorator for
  Python queries + live-reloaded YAML queries in `~/.claude/commands/queries/*.md`
  that become both CLI commands and Claude Code slash commands with zero code changes
- **Query count**: ~51 queries organized by domain (session, turn, tool, analysis,
  assistant, core, operators)
- **MCP integration**: Exposes JSONL analysis and formatting utilities as MCP tools
  (file: `/Users/david/Projects/madeleine/src/madeleine/mcp_tools.py`)
- **Monadic composition**: `@filter`, `@map`, `@group_by`, `@aggregate` operators
  enable functional pipeline composition in YAML

### madeleine-core (v2)

- **Stack**: Python 3.10+, Click CLI, SQLite, `python-to-mcp`, Starlette web viewer
- **Purpose**: "YAML-driven schema-to-database pipeline for Claude conversation
  histories" -- a rewrite with declarative schema layer
- **Database**: Dual-database architecture:
  - `schema_first.db` -- extracted conversation data (read-only, rebuilt on extract)
  - `user_metadata.db` -- user bookmarks, scores, patterns (persistent)
  Cross-database queries via SQLite `ATTACH DATABASE`.
- **Schema**: YAML-defined at `/Users/david/Projects/madeleine-core/schemas/conversation_memory.yml`.
  7 models: `Project`, `Session`, `Conversation`, `Tool`, `File`, `ConversationSearch`
  (FTS5), `Progress`. Plus user metadata schema with `ResponseScore`, `Bookmark`, `Pattern`.
- **Extraction**: YAML-driven preprocessing pipeline with `group_conversation_turns`,
  `index`, `filter_transform`, `filter_by_type` stages. Incremental ingestion via
  byte offset tracking.
- **Query layer**: 11 built-in operators (`@filter`, `@map`, `@group_by`, `@aggregate`,
  `@exists`, `@safe_any`, `@expand`, `@transform`, `@window`, `@analyze`, `@correlate`).
  28 seed queries as `.md` files with embedded YAML.
- **MCP integration**: 4 tools (`query_run`, `query_list`, `action_run`, `action_list`)
  via `/Users/david/Projects/madeleine-core/src/madeleine_core/mcp_tools.py`
- **Web viewer**: Starlette + HTMX server with SSE live updates
  (`/Users/david/Projects/madeleine-core/src/madeleine_core/viewer/`)

---

## 2. Query Architecture

### How Cross-Domain Queries Work

**madeleine v1** uses two approaches:

1. **Hardcoded SQL in Python query classes** -- Direct multi-table JOINs written in
   Python, registered via `@register_query`. Example from
   `/Users/david/Projects/madeleine/src/madeleine/queries/analysis/get_session_cost_analysis.py`:

   ```sql
   SELECT s.uuid, s.project_path, s.total_turns,
          COALESCE(SUM(tu.input_tokens), 0) as total_input_tokens,
          COALESCE(SUM(tu.output_tokens), 0) as total_output_tokens,
          ...
   FROM sessions s
   LEFT JOIN turn_usage tu ON s.uuid = tu.session_uuid
   WHERE s.uuid = ?
   GROUP BY s.uuid
   ```

   This joins `sessions` + `turn_usage` to answer "what did session X cost?"

2. **Schema-agnostic core queries** -- The `core.*` query family in
   `/Users/david/Projects/madeleine/src/madeleine/queries/core/` discovers entity
   types and FK relationships from a `ModelRegistry` at runtime, building SQL dynamically.
   `BaseCoreQuery` (at `base_core_query.py`) provides:
   - `_build_fk_graph()` -- Builds FK relationship graph from model metadata
   - `_find_path_to_target()` -- BFS pathfinding through FK graph
   - `_build_join_clause()` -- Generates SQL JOINs by walking FK edges
   - `_get_table_with_project()` -- Automatically JOINs any entity to its Session
     for project context

   Six universal queries: `core.entities`, `core.relationships`, `core.search`,
   `core.analytics`, `core.timeline`, `core.compose`

**madeleine-core (v2)** uses YAML-defined SQL queries:

```yaml
# From /Users/david/Projects/madeleine-core/src/madeleine_core/seeds/tool_recent_files.md
query:
  name: tool.recent_files
  category: tool
  description: Recently modified files across sessions
  params: [days, limit]
  query: |
    SELECT f.file_path, f.operation_type, c.timestamp,
           s.session_uuid, p.project_name
    FROM files f
    JOIN tools t ON f.tool_id = t.tool_id
    JOIN conversations c ON t.conversation_id = c.conversation_id
    JOIN sessions s ON c.session_id = s.session_id
    JOIN projects p ON s.project_id = p.project_id
    WHERE c.timestamp > datetime('now', '-{days} days')
    ORDER BY c.timestamp DESC
    LIMIT {limit}
```

This traverses 5 tables (files -> tools -> conversations -> sessions -> projects)
in a single query. The `{days}` and `{limit}` parameters are substituted by the
query loader at runtime.

### Pipeline Composition

Both versions support composing queries into pipelines:

```
session.list | @filter:item.total_tokens>1000 | turn.search
```

The `PipelineExecutor` at `/Users/david/Projects/madeleine-core/src/madeleine_core/query/composition.py`
parses this string, instantiates each stage, and passes `input_value` from
one stage to the next. Operators (`@filter`, `@map`, etc.) receive the previous
result and transform it.

### Cross-Database Queries (v2 only)

madeleine-core uses SQLite `ATTACH DATABASE` for joining user metadata with
conversation data. From `/Users/david/Projects/madeleine-core/src/madeleine_core/integration.py`:

```python
class DatabaseAdapter:
    def _get_connection(self, attach_user_db: bool = False):
        conn = connect_db(self.schema_db_path)
        if attach_user_db and self.user_db_path:
            conn.execute(f'ATTACH DATABASE "{self.user_db_path}" AS user_metadata')
        return conn
```

Queries can then reference `user_metadata.bookmarks` alongside `sessions`:

```yaml
# From /Users/david/Projects/madeleine-core/src/madeleine_core/seeds/bookmark_sessions.md
query:
  requires_attach: true
  query: |
    SELECT s.session_uuid, p.project_name, b.note, b.tags
    FROM sessions s
    JOIN user_metadata.bookmarks b ON s.session_uuid = b.session_uuid
    JOIN projects p ON s.project_id = p.project_id
```

---

## 3. Cross-Domain Intelligence

### What Madeleine Can Answer That cc-history-api Currently Cannot

**"What files cost the most tokens?"** -- Madeleine v1's `analysis.cost` query
joins sessions with turn_usage to get per-session token costs. The v2 seed
`analysis.cost` (`/Users/david/Projects/madeleine-core/src/madeleine_core/seeds/analysis_cost.md`)
joins sessions -> projects -> conversations to aggregate tokens by session. Neither
currently answers the *exact* question "files that cost the most tokens" because
that requires joining file_operations -> conversations -> token columns, but the
schema supports it and the YAML query pattern makes adding it trivial.

**"Show conversation context around file edits"** -- Madeleine v1's
`assistant.messages_with_tools` query at
`/Users/david/Projects/madeleine/src/madeleine/queries/assistant/messages_with_tools.py`
joins turns -> sessions -> tool_uses to get assistant messages paired with their
following tool invocations. This is exactly the pattern for "what was the
conversation that led to this edit."

**"What happened in project X this week?"** -- The v2 `viewer.feed` seed query at
`/Users/david/Projects/madeleine-core/src/madeleine_core/seeds/viewer_feed.md`
provides project-level dashboard data: latest turn per project, turn counts, and
preview text across projects -> sessions -> conversations.

**"Search file operations by path"** -- The v2 `viewer.search_files` seed at
`/Users/david/Projects/madeleine-core/src/madeleine_core/seeds/viewer_search_files.md`
traverses files -> tools -> conversations -> sessions -> projects for file path
search with full project/session/timestamp context.

### How Domain Boundaries Are Bridged

The key insight is that **joins are the bridge mechanism**. Both madeleine versions
use SQL JOIN chains to traverse the normalized schema. The v1 `core.relationships`
query (`/Users/david/Projects/madeleine/src/madeleine/queries/core/relationships.py`)
is the most sophisticated -- it uses recursive CTEs and FK graph traversal to answer
"given entity X, find all related entities of type Y within N hops."

The v1 `core.search` query (`/Users/david/Projects/madeleine/src/madeleine/queries/core/search.py`)
demonstrates **unified cross-entity search**: it builds a UNION ALL query across
all entities with text fields, searching each entity's text columns and returning
results in a common format `(entity_type, entity_id, content, timestamp, project_id)`.

---

## 4. Project-Level Views

### Project as First-Class Entity

**madeleine-core** has `Project` as a first-class model in the YAML schema with its
own table (`projects`) and a `project_id` FK throughout Session -> Conversation chains.
This enables:

```sql
-- From viewer.feed seed query
SELECT p.project_name, COUNT(c.conversation_id) as turn_count,
       MAX(c.timestamp) as latest_timestamp, ...
FROM projects p
JOIN sessions s ON s.project_id = p.project_id
JOIN conversations c ON c.session_id = s.session_id
GROUP BY p.project_id
```

**madeleine v1** has `normalized_project_name` on the sessions table but no separate
project table. The `session.list_normalized_projects` query at
`/Users/david/Projects/madeleine/src/madeleine/queries/session/list_normalized_projects.py`
aggregates project-level views from session data.

**cc-history-api** has `project_path` as a column on `sessions` but no normalized
project entity. This means project-level aggregation requires GROUP BY on raw paths,
which may have inconsistencies across sessions (e.g., different path encodings for
the same project).

### Key Gap for cc-history-api

Without a normalized `projects` table, cc-history-api cannot efficiently:
- List projects with aggregated metrics
- Get per-project token costs
- Show project-level activity feeds
- Filter any query by project cleanly

---

## 5. Conversation Intelligence

### Surfacing Semantic Content

**madeleine v1** provides several patterns:

1. **Direct content retrieval**: The `turn.get_conversation_response` query at
   `/Users/david/Projects/madeleine/src/madeleine/queries/turn/get_conversation_response.py`
   returns full assistant response text for a specific turn.

2. **Content search with context**: `turn.search` at
   `/Users/david/Projects/madeleine/src/madeleine/queries/turn/search_turns.py`
   does LIKE-based search across user_input and assistant_response fields.

3. **FTS5 with snippets**: madeleine-core's `search.fts` seed query at
   `/Users/david/Projects/madeleine-core/src/madeleine_core/seeds/search_fts.md`
   uses FTS5 snippet() for highlighted search with user_input context:

   ```sql
   SELECT snippet(conversation_search, 0, '**', '**', '...', 32) as snippet,
          c.user_input, s.session_uuid, p.project_name
   FROM conversation_search cs
   JOIN conversations c ON cs.conversation_id = c.conversation_id
   JOIN sessions s ON c.session_id = s.session_id
   JOIN projects p ON s.project_id = p.project_id
   WHERE conversation_search MATCH '{query}'
   ```

4. **Messages with tool correlation**: The `assistant.messages_with_tools` query
   (v1 Python implementation, v2 YAML seed) pairs assistant responses with the tool
   invocations they triggered. This answers "what reasoning led to this edit?"

### What's Missing

Neither madeleine version provides embedding-based semantic search or LLM-powered
summarization. All search is keyword/FTS-based. The "substantive responses about
topic X" question can only be approximated via FTS match with LIKE fallback.

---

## 6. File Provenance

### How File History Is Tracked

**madeleine v1**: `file_operations` table with `(session_uuid, turn_number, operation_type,
file_path, file_content_preview)`. The `tool.file_timeline` query at
`/Users/david/Projects/madeleine/src/madeleine/queries/tool/file_timeline.py`
is actually a stub returning empty results. The `tool.recent_file_operations` query
at `/Users/david/Projects/madeleine/src/madeleine/queries/tool/recent_file_operations.py`
provides basic recent file listing.

**madeleine-core (v2)**: The `File` model in the YAML schema tracks `(file_path,
operation_type, tool_use_id)` with FK to `Tool`. The `viewer.search_files` seed query
reconstructs file provenance across sessions by traversing:
files -> tools -> conversations -> sessions -> projects.

**cc-history-api** has the richest file model:
- `files` table: per-session deduplication with `first_seen`, `last_modified`, `operation_count`
- `file_operations` table: full content for writes, old/new for edits, bash commands
- `git_operations` table: extracted from Bash git commands with parsed commit messages
- Content reconstruction via `reconstruct_content_at()` in `artifact_queries.rs`
- Unified diff generation via the `similar` crate
- FTS5 on file_operations content

cc-history-api is *ahead* of madeleine on file provenance features. The gap is that
this file data lives in isolation -- it cannot currently be correlated with
conversation context or token cost data.

---

## 7. API Design Patterns

### madeleine v1 API Surface

- **CLI**: Click-based with dynamic command registration from query metadata.
  `madeleine query <name> --param value` for all registered queries.
- **MCP Tools**: JSONL analysis utilities exposed via `python-to-mcp`
  (14 tools for file analysis and formatting -- NOT query execution)
- **Slash Commands**: YAML queries become Claude Code slash commands via
  `~/.claude/commands/queries/` file placement

### madeleine-core (v2) API Surface

- **CLI**: Click-based. `madeleine extract`, `madeleine query run <name> --params '{}'`,
  `madeleine action run <name>`, `madeleine interactive`, `madeleine serve-mcp`
- **MCP Tools**: 4 tools: `query_run`, `query_list`, `action_run`, `action_list`.
  Any registered query (SQL or composition) is callable via MCP.
- **Web Viewer**: Starlette + HTMX with routes for feed, sessions, turns, tools,
  search. HTML fragment responses for HTMX partial updates.
- **Programmatic API**: `Application` class with `execute_query()` and
  `execute_action()` methods

### cc-history-api API Surface

- **CLI**: clap-based. Subcommands for `sync`, `search`, `sessions`, `query`,
  `stats`, `export`, `files`, `file-history`, `reconstruct`, `git-log`, `artifacts`
- **HTTP API**: axum-based, 28 endpoints across 11 resource groups at `/v1/`.
  REST-style resource endpoints (sessions, messages, files, git, artifacts).
- **SSE Events**: `GET /v1/events` for live ingestion notifications
- **No MCP integration yet**

### Key Comparison

| Capability | cc-history-api | madeleine v1 | madeleine-core |
|------------|---------------|--------------|----------------|
| HTTP API | 28 endpoints | None | HTMX viewer |
| CLI | clap, fixed subcommands | Click, dynamic from queries | Click, dynamic from queries |
| MCP Tools | None | 14 (utilities) | 4 (query/action execution) |
| SSE Events | Yes | No | Yes (viewer) |
| Query composition | No | Yes (monadic) | Yes (pipeline) |
| YAML-defined queries | No | Yes (live-reload) | Yes (live-reload) |
| Cross-entity queries | Per-endpoint hardcoded | Plugin + core.* universal | YAML seed queries |

---

## 8. Data Model Comparison

### Same Source Data, Different Decomposition

All three systems ingest the same Claude Code JSONL files from `~/.claude/projects/`.
They decompose the data differently:

**cc-history-api** (13 tables, migration-based):
- Preserves the full message structure: `messages` -> `message_content` -> `token_usage`
- Content blocks as separate rows (text, thinking, tool_use, tool_result)
- `tool_executions` joins tool_use + tool_result by `tool_use_id`
- Schema drift captured in `schema_drift_log`
- Artifact layer: `files`, `file_operations`, `git_operations`

**madeleine v1** (7 tables, code-defined):
- Groups JSONL entries into conversation `turns` (user + assistant + tools per turn)
- Separate `turn_usage` table for tokens
- `tool_uses` and `file_operations` extracted per turn
- `todos` + `todo_state_changes` for task tracking (unique to v1)

**madeleine-core** (7 models, YAML-defined):
- `Project` -> `Session` -> `Conversation` -> `Tool` -> `File` hierarchy
- `ConversationSearch` as FTS5 virtual table
- `Progress` for hook/agent events
- Tokens stored directly on `Conversation` (not separate table)
- User metadata in separate database: `bookmarks`, `response_scores`, `patterns`

### Key Schema Differences

1. **cc-history-api preserves message-level granularity** while madeleine aggregates
   into turns. cc-history-api has `messages` with individual UUID, parent_uuid, and
   message_type. Madeleine has `turns`/`conversations` which combine user + assistant
   into one row.

2. **cc-history-api has richer content block modeling** with `message_content` separating
   text, thinking, tool_use, and tool_result blocks. Madeleine stores `user_input` and
   `assistant_response` as single text fields with `thinking_text` separated out.

3. **madeleine-core has explicit Project entity** while cc-history-api and madeleine v1
   store project as a string on sessions.

4. **cc-history-api has git_operations** as a first-class table. Neither madeleine
   version extracts git operations.

---

## 9. Patterns Applicable to cc-history-api

### Pattern 1: Cross-Domain SQL Queries as Named Resources

**What madeleine does**: YAML files containing SQL with parameter placeholders become
named, discoverable, executable query resources. Example from
`/Users/david/Projects/madeleine-core/src/madeleine_core/seeds/analysis_cost.md`:

```yaml
query:
  name: analysis.cost
  category: analysis
  params: [days, limit]
  query: |
    SELECT s.session_uuid, p.project_name,
           COALESCE(SUM(c.input_tokens + c.output_tokens), 0) as total_tokens
    FROM sessions s JOIN projects p ON ...
    JOIN conversations c ON ...
```

**How to apply**: cc-history-api could define cross-domain queries as SQL files or
YAML-embedded SQL that join across its existing tables. For example, "files that cost
the most tokens" would be:

```sql
SELECT fo.file_path,
       COUNT(fo.id) as operation_count,
       SUM(tu.input_tokens + tu.output_tokens) as total_tokens
FROM file_operations fo
JOIN messages m ON fo.message_uuid = m.uuid
JOIN token_usage tu ON m.uuid = tu.message_uuid
GROUP BY fo.file_path
ORDER BY total_tokens DESC
```

The schema already supports this join chain. The gap is that no endpoint or query
function exists to execute it.

### Pattern 2: Unified Timeline as Cross-Entity UNION ALL

**What madeleine does**: The `core.timeline` query builds UNION ALL across all entity
types that have timestamp fields, normalizing each into a common event format:
`(timestamp, event_type, entity_identifier, event_description, project_id)`.

**How to apply**: cc-history-api's `session_timeline()` in `artifact_queries.rs`
already does this for file_operations + git_operations + tool_executions within a
session. Extending this to include messages and token_usage events would create a
true cross-domain timeline.

### Pattern 3: Project Normalization Table

**What madeleine-core does**: Separate `projects` table with FK from sessions.

**How to apply**: cc-history-api should consider a migration adding a `projects`
table extracted from `sessions.project_path`. This would enable:
- `GET /v1/projects` listing
- `GET /v1/projects/{name}/stats` with aggregated token/file/git metrics
- Project-scoped filtering on all endpoints

### Pattern 4: MCP Tool Gateway

**What madeleine-core does**: 4 MCP tools that act as a gateway to the entire query
system. `query_run(name, params)` can execute any registered query. This means
adding a new query (YAML file) immediately makes it available to Claude Code agents.

**How to apply**: cc-history-api could expose 2-3 MCP tools:
- `query_run(endpoint, params)` -- execute any API endpoint programmatically
- `search(query)` -- FTS5 search
- `timeline(session_id)` -- cross-domain timeline

This would make cc-history-api usable as a development intelligence source within
Claude Code sessions.

### Pattern 5: Parameterized Composition

**What madeleine does**: Pipeline composition where query results flow from one
stage to the next via `input_value`:

```
session.list | @filter:item.total_tokens>1000 | turn.search
```

**How to apply**: While cc-history-api may not need runtime composition (its typed
Rust queries provide compile-time safety), the *concept* of composable query
building blocks is valuable. The API could support a `POST /v1/query` endpoint
that accepts a structured query specification with joins, filters, and aggregations
across domains.

---

## 10. Anti-Patterns and Lessons

### Anti-Pattern 1: Schema-Agnostic Over-Abstraction

madeleine v1's `core.*` queries use BFS through FK graphs, dynamic SQL generation,
and model introspection to build queries for *any* schema. This is powerful but
produces complex, hard-to-debug code. The `BaseCoreQuery` at
`/Users/david/Projects/madeleine/src/madeleine/queries/core/base_core_query.py`
is 590 lines of infrastructure for generating SQL that could be written directly.

**Lesson**: cc-history-api's schema is stable and purpose-built. The right abstraction
is *named queries with explicit SQL*, not a generic SQL generator. The madeleine-core
v2 YAML seed queries demonstrate this more pragmatic approach.

### Anti-Pattern 2: Dual Schema Systems

madeleine v1 has hardcoded schema in `database_schema_initializer.py` AND
YAML-driven schema in `schema_first/`. This created confusion about which is
canonical.

**Lesson**: cc-history-api's migration-based schema is clean and canonical. Do not
introduce a competing schema definition system. Add new queries as Rust functions
or SQL files, not a YAML schema layer.

### Anti-Pattern 3: Stub Queries

madeleine v1's `tool.file_timeline` is a registered query that returns empty results
(it is a stub). This creates false discoverability.

**Lesson**: Do not expose endpoints or query functions that return no data. If a
cross-domain query is not implemented, do not register it.

### Anti-Pattern 4: Live-Reload Complexity

Both madeleine versions use file watchers for live query reloading. This adds
complexity (watchdog dependencies, debounce logic, race conditions) for a feature
whose primary benefit is development-time convenience.

**Lesson**: cc-history-api is compiled Rust. Hot-reloading queries is not a natural
fit. If YAML-defined queries are ever desired, they should be loaded at startup
and require a restart to change, not watched at runtime.

---

## 11. Synthesis Opportunities

### Opportunity 1: Cross-Domain Query Endpoints

The highest-value pattern from madeleine is the multi-table SQL query that joins
across domain boundaries. cc-history-api already has the schema and indexes. Specific
new queries cc-history-api could implement:

| Query | Tables Joined | What It Answers |
|-------|--------------|-----------------|
| file_token_cost | file_operations -> messages -> token_usage | "Which files cost the most tokens?" |
| conversation_around_edit | file_operations -> messages -> message_content | "What conversation led to this edit?" |
| project_activity_feed | sessions -> messages -> file_operations + git_operations | "What happened in project X?" |
| tool_cost_breakdown | tool_executions -> messages -> token_usage | "Which tools are most expensive?" |
| session_file_summary | files -> file_operations -> messages -> token_usage | "What files were touched and how much did it cost?" |

These are all expressible as SQL queries over the existing cc-history-api schema.
No schema changes needed.

### Opportunity 2: Projects Normalization

Add a `projects` table via migration 004:

```sql
CREATE TABLE projects (
    project_path  TEXT PRIMARY KEY,
    display_name  TEXT,
    first_seen    TEXT,
    last_seen     TEXT,
    session_count INTEGER DEFAULT 0
);
```

Backfill from `SELECT DISTINCT project_path FROM sessions`. Then add
`GET /v1/projects` and `GET /v1/projects/{path}/summary` endpoints.

### Opportunity 3: Conversation Context Retrieval

The madeleine `assistant.messages_with_tools` pattern is directly applicable.
For cc-history-api, this would be a query function:

```sql
SELECT m.uuid, m.timestamp, m.model,
       mc.text_content as assistant_text,
       te.tool_name, te.input_json,
       fo.file_path, fo.operation_type
FROM messages m
JOIN message_content mc ON m.uuid = mc.message_uuid AND mc.block_type = 'text'
LEFT JOIN tool_executions te ON m.uuid = te.message_uuid
LEFT JOIN file_operations fo ON te.tool_use_id = fo.tool_use_id
WHERE m.session_id = ?
ORDER BY m.timestamp
```

This gives conversation flow with tool use and file operations in a single stream.

### Opportunity 4: MCP Tool Integration

Following madeleine-core's pattern of 4 gateway MCP tools, cc-history-api could
expose its capabilities as MCP tools that Claude Code agents can call directly.
This would allow agents to query their own development history during sessions --
the core value proposition described in madeleine's README.

### Opportunity 5: Hybrid Query Approach

Rather than choosing between cc-history-api's type-safe Rust functions and
madeleine's dynamic YAML queries, cc-history-api could:

1. Keep its typed Rust query functions for well-defined endpoints
2. Add a `POST /v1/query/sql` endpoint that accepts parameterized SQL for ad-hoc
   cross-domain queries (read-only, with parameter binding for safety)
3. Optionally load `.sql` files from a queries directory at startup for named
   cross-domain queries

This gives the safety of Rust types for core queries while enabling the exploratory
cross-domain queries that madeleine's YAML system provides.

---

## 12. Summary

The most transferable patterns from madeleine to cc-history-api, in priority order:

1. **Cross-domain SQL queries** -- Multi-table JOINs that answer questions spanning
   files + tokens + conversations + projects. cc-history-api's schema already supports
   these; it needs the query functions and endpoints.

2. **Project normalization** -- First-class project entity with aggregated views.
   One migration + 2 endpoints.

3. **Conversation context retrieval** -- Queries that pair assistant reasoning with
   the file operations it triggered. Essential for "why was this edit made?"

4. **MCP tool gateway** -- Exposing query capabilities as MCP tools so Claude Code
   agents can query their own history.

5. **Unified cross-entity timeline** -- Extending the existing session timeline to
   include conversation turns and token usage events alongside file and git operations.

What cc-history-api should *not* adopt from madeleine: YAML schema definitions,
live-reload file watchers, schema-agnostic query generators, or monadic composition
pipelines. These solve problems that cc-history-api does not have, and would add
complexity without proportionate benefit in a compiled Rust system.
