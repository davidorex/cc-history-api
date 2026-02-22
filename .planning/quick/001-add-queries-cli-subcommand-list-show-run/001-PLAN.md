---
phase: quick
plan: 01
type: execute
wave: 1
depends_on: []
files_modified:
  - Cargo.toml
  - crates/store/Cargo.toml
  - crates/store/src/lib.rs
  - crates/store/src/query_registry.rs
  - crates/server/src/main.rs
  - crates/server/src/output.rs
  - queries/recent-sessions.sql
  - queries/recent-sessions.toml
  - queries/tool-usage-by-session.sql
  - queries/tool-usage-by-session.toml
  - queries/message-search-context.sql
  - queries/message-search-context.toml
autonomous: true
requirements: []

must_haves:
  truths:
    - "User can run `claude-history queries list` and see all available canned queries with name, description, and parameter signatures"
    - "User can run `claude-history queries show <name>` and see the SQL template plus metadata for a specific query"
    - "User can run `claude-history queries run <name> --param key=value` and get query results back (JSON or human-readable)"
    - "Named :param placeholders in SQL are converted to positional ?N parameters and bound through existing execute_sql()"
    - "Query files are loaded from $CLAUDE_HISTORY_QUERIES or ~/.claude/claude-history/queries/ with .sql + optional .toml sidecar"
  artifacts:
    - path: "crates/store/src/query_registry.rs"
      provides: "CannedQuery struct, load_queries(), param parsing, named-to-positional conversion"
      min_lines: 80
    - path: "crates/server/src/main.rs"
      provides: "Queries subcommand group with List, Show, Run variants"
      contains: "Queries"
    - path: "queries/recent-sessions.sql"
      provides: "Seed example query for distribution"
  key_links:
    - from: "crates/server/src/main.rs"
      to: "crates/store/src/query_registry.rs"
      via: "load_queries() call"
      pattern: "query_registry::load_queries"
    - from: "crates/server/src/main.rs"
      to: "crates/store/src/sql_passthrough.rs"
      via: "execute_sql() for run subcommand"
      pattern: "sql_passthrough::execute_sql"
---

<objective>
Add a `queries` CLI subcommand group to claude-history that loads canned SQL queries from a configurable directory (~/.claude/claude-history/queries/ by default), supports listing/showing/running them, and feeds execution through the existing sql_passthrough module with named-to-positional parameter conversion.

Purpose: Enable reusable, parameterized SQL queries that users can curate as .sql files with .toml metadata sidecars, making common analytical queries accessible without remembering SQL.

Output: Working `queries list`, `queries show <name>`, `queries run <name>` subcommands + query_registry module + seed example queries.
</objective>

<execution_context>
@~/.claude/get-shit-done/workflows/execute-plan.md
@~/.claude/get-shit-done/templates/summary.md
</execution_context>

<context>
@crates/server/src/main.rs
@crates/store/src/sql_passthrough.rs
@crates/store/src/lib.rs
@crates/store/Cargo.toml
@crates/server/Cargo.toml
@Cargo.toml
@crates/server/src/output.rs
</context>

<tasks>

<task type="auto">
  <name>Task 1: Add toml dependency and create query_registry module in store crate</name>
  <files>
    Cargo.toml
    crates/store/Cargo.toml
    crates/store/src/lib.rs
    crates/store/src/query_registry.rs
  </files>
  <action>
1. Add `toml = "0.8"` to `[workspace.dependencies]` in root Cargo.toml.
2. Add `toml = { workspace = true }` to `[dependencies]` in crates/store/Cargo.toml.
3. Add `pub mod query_registry;` to crates/store/src/lib.rs.
4. Create crates/store/src/query_registry.rs with:

**CannedQuery struct** (derive Debug, Clone, Serialize):
```rust
pub struct CannedQuery {
    pub name: String,           // stem of .sql file
    pub sql: String,            // raw SQL template with :param placeholders
    pub description: String,    // from .toml or "No description"
    pub params: Vec<ParamDef>,  // ordered param definitions
}

pub struct ParamDef {
    pub name: String,           // param name without colon
    pub description: String,    // from .toml or empty
    pub default: Option<String>,// optional default value from .toml
}
```

**TOML sidecar format** (deserialize struct):
```toml
description = "..."
[[params]]
name = "limit"
description = "Maximum rows"
default = "20"
```

**load_queries(dir: &Path) -> Result<HashMap<String, CannedQuery>>:**
- Read all *.sql files in the directory
- For each .sql file, look for a matching .toml sidecar
- If .toml exists, parse it for description and params list
- If no .toml, scan the SQL for `:word` patterns (not inside single-quoted strings) to auto-discover params, set description to "No description"
- Return HashMap keyed by query name (file stem)
- If directory doesn't exist, return Ok(empty HashMap) with a tracing::warn

**resolve_queries_dir() -> PathBuf:**
- Check $CLAUDE_HISTORY_QUERIES env var first
- Fall back to $HOME/.claude/claude-history/queries/

**prepare_sql(query: &CannedQuery, params: &HashMap<String, String>) -> Result<(String, Vec<serde_json::Value>)>:**
- Find all `:param_name` occurrences in the SQL (not inside single-quoted strings)
- Assign each unique param a positional index (1-based)
- Replace `:param_name` with `?N` where N is the positional index
- Build the params Vec in positional order, using the provided param values or defaults
- Return error if a required param (no default) is missing
- Return the rewritten SQL and the positional params vec (as serde_json::Value::String for all values)

**extract_named_params(sql: &str) -> Vec<String>:**
- Parse `:word_chars` patterns from SQL, skipping content inside single-quoted strings
- Return unique param names in order of first appearance
- Use a simple state machine (inside_quote bool, track escaped quotes)

Include unit tests:
- extract_named_params finds params and skips quoted strings
- prepare_sql converts named params to positional correctly
- prepare_sql uses defaults when param not provided
- prepare_sql errors on missing required param
- load_queries returns empty HashMap for nonexistent dir
  </action>
  <verify>
    `cargo test -p claude-history-store -- query_registry` passes all tests.
    `cargo check -p claude-history-store` compiles without errors.
  </verify>
  <done>
    query_registry module compiles, exports CannedQuery/ParamDef/load_queries/prepare_sql/resolve_queries_dir, and all unit tests pass.
  </done>
</task>

<task type="auto">
  <name>Task 2: Add Queries subcommand group to CLI with list/show/run handlers and seed queries</name>
  <files>
    crates/server/src/main.rs
    crates/server/src/output.rs
    queries/recent-sessions.sql
    queries/recent-sessions.toml
    queries/tool-usage-by-session.sql
    queries/tool-usage-by-session.toml
    queries/message-search-context.sql
    queries/message-search-context.toml
  </files>
  <action>
1. **Add Queries variant to Commands enum** in main.rs using clap `#[command(subcommand)]`:
```rust
/// Manage and run canned SQL queries
Queries {
    #[command(subcommand)]
    action: QueriesAction,
},
```

2. **Define QueriesAction enum:**
```rust
#[derive(Subcommand)]
enum QueriesAction {
    /// List all available canned queries
    List {
        /// Output as JSON
        #[arg(long)]
        json: bool,
        /// Path to queries directory
        #[arg(long)]
        queries_dir: Option<PathBuf>,
    },
    /// Show SQL and metadata for a specific query
    Show {
        /// Query name (filename without .sql extension)
        name: String,
        /// Path to queries directory
        #[arg(long)]
        queries_dir: Option<PathBuf>,
    },
    /// Execute a canned query with parameter binding
    Run {
        /// Query name (filename without .sql extension)
        name: String,
        /// Parameters as key=value pairs (repeatable)
        #[arg(long = "param", value_parser = parse_key_val)]
        params: Vec<(String, String)>,
        /// Output as JSON (default: true for run)
        #[arg(long)]
        json: bool,
        /// Path to queries directory
        #[arg(long)]
        queries_dir: Option<PathBuf>,
    },
}
```

3. **Add parse_key_val helper** for clap value_parser:
```rust
fn parse_key_val(s: &str) -> Result<(String, String), String> {
    let pos = s.find('=').ok_or_else(|| format!("invalid KEY=value: no `=` found in `{s}`"))?;
    Ok((s[..pos].to_string(), s[pos + 1..].to_string()))
}
```

4. **Route the Queries subcommand** in the main match. The Queries subcommand does NOT need ConnectionMode for list/show (they are filesystem-only). For `run`, it needs a DB connection. Handle this by:
- Moving `Commands::Queries { action }` to the top-level match alongside Serve/Sync (before the read_cmd catch-all)
- For List/Show: call `load_queries()` and format output, no DB needed
- For Run: resolve connection mode, load queries, prepare_sql, then call `execute_sql` through `conn.call()`

5. **Implement run_queries_list():**
- Resolve queries dir from --queries-dir arg or `resolve_queries_dir()`
- Load queries via `load_queries()`
- If empty, print "No queries found in {dir}" to stderr
- If --json: serialize the HashMap values as JSON array (name, description, params)
- If human-readable: print a table with columns NAME (20), DESCRIPTION (40), PARAMS (remaining)

6. **Implement run_queries_show():**
- Load queries, find by name, error if not found
- Print the SQL template with a header, then the metadata (description, params with defaults)
- Always human-readable (this is a display command)

7. **Implement run_queries_run():**
- Load queries, find by name, error if not found
- Call `prepare_sql()` to convert named params to positional
- Use `conn.call(move |conn| sql_passthrough::execute_sql(conn, &sql, &params))` to execute
- Output JSON rows (always JSON by default for run, consistent with sql passthrough behavior)
- Print row count to stderr

8. **Add output helpers** in output.rs:
- `print_queries_list(queries: &[&CannedQuery])` -- table format with NAME, DESCRIPTION, PARAMS columns
- No need for a separate show formatter -- show can print directly in the handler

9. **Create seed query files** in `queries/` at repo root (these ship with the repo as examples; users copy or symlink to ~/.claude/claude-history/queries/):

**queries/recent-sessions.sql:**
```sql
SELECT
    s.session_id,
    s.project_path,
    s.first_message_at,
    s.last_message_at,
    s.message_count,
    s.model
FROM sessions s
ORDER BY s.last_message_at DESC
LIMIT :limit
```

**queries/recent-sessions.toml:**
```toml
description = "List recent sessions ordered by last activity"
[[params]]
name = "limit"
description = "Maximum number of sessions to return"
default = "20"
```

**queries/tool-usage-by-session.sql:**
```sql
SELECT
    m.session_id,
    tb.tool_name,
    COUNT(*) as invocations,
    SUM(CASE WHEN tb.is_error = 1 THEN 1 ELSE 0 END) as errors
FROM messages m
JOIN tool_blocks tb ON m.uuid = tb.message_uuid
WHERE m.session_id = :session_id
GROUP BY m.session_id, tb.tool_name
ORDER BY invocations DESC
```

**queries/tool-usage-by-session.toml:**
```toml
description = "Show tool usage breakdown for a specific session"
[[params]]
name = "session_id"
description = "Session ID to analyze"
```

**queries/message-search-context.sql:**
```sql
SELECT
    m.uuid,
    m.session_id,
    m.message_type,
    m.timestamp,
    tb.block_type,
    SUBSTR(tb.content, 1, 200) as content_preview
FROM messages m
JOIN text_blocks tb ON m.uuid = tb.message_uuid
WHERE tb.content LIKE '%' || :search_term || '%'
ORDER BY m.timestamp DESC
LIMIT :limit
```

**queries/message-search-context.toml:**
```toml
description = "Search message content with context (uses LIKE, not FTS)"
[[params]]
name = "search_term"
description = "Text to search for in message content"
[[params]]
name = "limit"
description = "Maximum results"
default = "50"
```

10. **Update the doc comment** at the top of main.rs to include the queries subcommand in the usage list.
  </action>
  <verify>
    `cargo build` compiles the full workspace without errors.
    `cargo run -- queries list --queries-dir ./queries` shows the 3 seed queries with names, descriptions, and params.
    `cargo run -- queries show recent-sessions --queries-dir ./queries` displays the SQL and metadata.
    `cargo run -- queries run recent-sessions --queries-dir ./queries --json` executes against the DB and returns JSON rows (may be empty if DB is empty, but should not error on SQL execution if DB has the sessions table).
    `cargo test -p claude-history-store -- query_registry` continues to pass.
  </verify>
  <done>
    The `queries list`, `queries show`, and `queries run` subcommands are functional. Users can list available queries, inspect their SQL/params, and execute them with named parameter binding that flows through the existing sql_passthrough module. Three seed queries ship in the repo's queries/ directory.
  </done>
</task>

</tasks>

<verification>
- `cargo build` succeeds with no warnings on new code
- `cargo test` passes all existing and new tests
- `cargo run -- queries list --queries-dir ./queries` lists 3 seed queries
- `cargo run -- queries show recent-sessions --queries-dir ./queries` shows SQL and metadata
- `cargo run -- queries run recent-sessions --queries-dir ./queries --json` returns JSON output
- `cargo run -- queries run tool-usage-by-session --queries-dir ./queries --param session_id=test` does not panic (may return empty results or SQL error if session doesn't exist, but param binding works)
- Named :param to ?N conversion is tested via unit tests
- Missing required param produces a clear error message
</verification>

<success_criteria>
- query_registry module in store crate compiles, is tested, and exports CannedQuery/load_queries/prepare_sql
- Queries subcommand group in CLI with list/show/run variants fully functional
- Named parameter binding (:name -> ?N) works correctly through existing execute_sql()
- 3 seed queries exist in queries/ directory with .sql + .toml sidecars
- All existing tests continue to pass
</success_criteria>

<output>
After completion, create `.planning/quick/001-add-queries-cli-subcommand-list-show-run/001-SUMMARY.md`
</output>
