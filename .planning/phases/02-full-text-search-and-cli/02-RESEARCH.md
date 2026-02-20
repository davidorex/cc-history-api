# Phase 2: Full-Text Search and CLI - Research

**Researched:** 2026-02-20
**Domain:** SQLite FTS5 full-text search indexing, Rust CLI subcommand expansion, session export
**Confidence:** HIGH

<spec_constraints>
## Spec Constraints (IMMUTABLE -- from ROADMAP.md Success Criteria)

**Success Criteria** (what must be TRUE):
  1. `claude-history search "some phrase"` returns ranked results from across all sessions, matching against message content and file operation content via FTS5
  2. `claude-history sessions` lists sessions with filters (project, date range, status) and `claude-history stats` shows token usage, tool frequency, and model breakdown
  3. `claude-history export <session-id>` produces valid JSON, Markdown, or CSV output of a complete session conversation
  4. `claude-history query` accepts filter arguments and outputs matching messages as JSON to stdout

**Requirements**: FTS-01, FTS-02, FTS-03, CLI-02, CLI-03, CLI-04, CLI-05, CLI-06, CLI-07, CLI-08, CLI-09

These are non-negotiable user-story outcomes. Research recommendations must not narrow below these.
</spec_constraints>

## Summary

Phase 2 builds on the complete ingestion pipeline from Phase 1 to add two capabilities: full-text search over ingested content via SQLite FTS5, and a full CLI interface with subcommands for querying, listing, searching, exporting, and analyzing session data. The codebase already has a working `claude-history sync` command, a single `tokio-rusqlite::Connection` writer, all normalized tables populated, and `rusqlite 0.37` with bundled SQLite that unconditionally enables FTS5 (confirmed: `libsqlite3-sys 0.35.0` build.rs sets `-DSQLITE_ENABLE_FTS5`).

The FTS5 work involves creating two external-content FTS5 virtual tables: one indexing `message_content.text_content` (for FTS-01) and one that will be needed for file operations content in Phase 5 (FTS-02 references `file_operations` which does not exist yet -- see Open Questions). The search endpoint (FTS-03) will combine results from both FTS tables with BM25 ranking and snippet extraction. The CLI work involves adding 8 new subcommands to the existing clap-based CLI, each producing either human-readable table output to stdout or structured JSON when `--json` is specified.

**Primary recommendation:** Use FTS5 external-content tables (via `content=` and `content_rowid=` options) that reference the existing `message_content` table, with `unicode61` tokenizer for general text and a `rebuild` command triggered after sync. Add new CLI subcommands to the existing clap `Commands` enum, using serde serialization for JSON output and manual formatting for human-readable tables. Add `csv` crate for CSV export. All FTS DDL goes in a new migration (`002_fts5.sql`).

## Standard Stack

### Core

| Library | Version | Purpose | Why Standard |
|---------|---------|---------|--------------|
| `rusqlite` | 0.37 (bundled) | FTS5 virtual tables + queries | Already in workspace; bundled SQLite includes FTS5 unconditionally |
| `tokio-rusqlite` | 0.7 | Async bridge for FTS queries | Already in workspace; all DB access goes through this |
| `clap` | 4.x (derive) | CLI subcommand expansion | Already in workspace; derive macros for new subcommands |
| `serde` | 1.0 | Serialize query results to JSON/CSV | Already in workspace |
| `serde_json` | 1.0 | JSON output for `--json` flag and `query` subcommand | Already in workspace |

### Supporting

| Library | Version | Purpose | When to Use |
|---------|---------|---------|-------------|
| `csv` | 1.3 | CSV export for `export --format csv` | Only for CLI-07 export subcommand |
| `chrono` | 0.4 | Date range parsing for `--after`/`--before` filters | Session and query filtering |

### Alternatives Considered

| Instead of | Could Use | Tradeoff |
|------------|-----------|----------|
| External-content FTS5 | Regular (content-storing) FTS5 | Regular FTS5 duplicates all indexed text in FTS shadow tables, roughly doubling DB size. External content avoids duplication but requires manual index maintenance (rebuild after sync). For a tool that syncs first and queries second, rebuild-after-sync is the right pattern. |
| `unicode61` tokenizer | `trigram` tokenizer | Trigram enables substring matching but produces much larger indexes (every 3-char sequence is a token). unicode61 is standard for natural language + code comments, which is the primary content type. Trigram could be offered as an optional mode later. |
| Manual table formatting | `comfy-table` or `tabled` crate | External table formatting crates add dependencies for something achievable with simple `format!` + fixed-width columns. The output is simple enough (5-10 columns max) that manual formatting avoids dependency creep. |
| `csv` crate | Manual CSV generation | CSV has edge cases (quoting, escaping, newlines in fields) that the `csv` crate handles correctly. Message content frequently contains commas and newlines, making hand-rolled CSV dangerous. |

**Installation:**

Add to `Cargo.toml` workspace dependencies:
```toml
csv = "1.3"
chrono = { version = "0.4", features = ["serde"] }
```

Add to `crates/server/Cargo.toml`:
```toml
csv = { workspace = true }
chrono = { workspace = true }
serde_json = { workspace = true }
```

## Architecture Patterns

### Recommended Project Structure Changes

```
crates/
  store/
    src/
      fts.rs              # NEW: FTS5 index creation, rebuild, search queries
      query.rs            # NEW: Query builder for message/session filtering
      lib.rs              # add: pub mod fts; pub mod query;
    migrations/
      002_fts5.sql        # NEW: FTS5 virtual table DDL
  server/
    src/
      main.rs             # MODIFY: expand Commands enum with 8 new subcommands
      output.rs           # NEW: output formatting (human-readable tables, JSON)
      export.rs           # NEW: session export logic (JSON, Markdown, CSV)
```

### Pattern 1: FTS5 External-Content Table Referencing message_content

**What:** Create an FTS5 virtual table that indexes `message_content.text_content` without duplicating the text data. The FTS5 index stores only the token positions and metadata, while the actual text remains in the `message_content` table.

**When to use:** When the content to index already exists in a regular table and you want to avoid doubling storage.

**Implementation (SQL -- in migration 002_fts5.sql):**

```sql
-- FTS5 index over message content text.
-- External content mode: indexes message_content.text_content without duplication.
-- content_rowid must reference the INTEGER PRIMARY KEY of the content table.
-- unicode61 tokenizer handles code + natural language reasonably well.
CREATE VIRTUAL TABLE fts_message_content USING fts5(
    text_content,
    content='message_content',
    content_rowid='id',
    tokenize='unicode61'
);
```

**Confidence:** HIGH -- FTS5 external content is well-documented in official SQLite docs and the `content=` / `content_rowid=` syntax is stable.

**Why external content over regular FTS5:**
- `message_content.text_content` contains assistant responses, user prompts, tool results, and thinking blocks. This text can be very large (multi-thousand-line file contents in tool results).
- Storing this text twice (once in `message_content`, once in FTS shadow tables) would roughly double the database size.
- The rebuild-after-sync pattern works naturally because `claude-history sync` is the single write entry point -- trigger a rebuild at the end of sync.

### Pattern 2: FTS5 Rebuild After Sync

**What:** After `sync_all` completes, issue a `rebuild` command to re-index the FTS5 table from the current content of `message_content`.

**Why not triggers:** Triggers on `message_content` would fire for every INSERT during sync, which could be thousands of inserts in rapid succession. The rebuild command at the end of sync is more efficient -- it processes all content in a single pass. Additionally, all writes happen through the decomposer in batch transactions, so trigger-based sync would add overhead to the hot path.

**Implementation:**

```rust
// In crates/store/src/fts.rs
pub fn rebuild_fts_index(conn: &rusqlite::Connection) -> Result<(), rusqlite::Error> {
    conn.execute_batch("INSERT INTO fts_message_content(fts_message_content) VALUES('rebuild');")?;
    Ok(())
}
```

Call this at the end of `sync_all` (inside the `conn.call()` closure, after all files are synced).

**Confidence:** HIGH -- the rebuild command is documented in official SQLite FTS5 docs. It deletes the entire full-text index and rebuilds it from the content table.

**Trade-off:** Rebuild re-indexes ALL content, not just new content. For large databases (100K+ sessions), this could take several seconds. An optimization for later: track whether any new content was actually added and skip rebuild if not. For Phase 2, the simplicity of always-rebuild is preferred.

### Pattern 3: FTS5 Search Query with BM25 Ranking and Snippets

**What:** Search across FTS-indexed content, returning ranked results with context snippets.

**Implementation:**

```sql
-- Search with BM25 ranking and snippet extraction
SELECT
    mc.message_uuid,
    mc.block_type,
    mc.block_index,
    m.session_id,
    m.type AS message_type,
    m.timestamp,
    snippet(fts_message_content, 0, '>>>', '<<<', '...', 30) AS snippet,
    bm25(fts_message_content) AS rank
FROM fts_message_content
JOIN message_content mc ON mc.id = fts_message_content.rowid
JOIN messages m ON m.uuid = mc.message_uuid
WHERE fts_message_content MATCH ?1
ORDER BY rank  -- lower bm25() values = better match
LIMIT ?2
OFFSET ?3;
```

**Key FTS5 query syntax:**
- Simple terms: `MATCH 'error handling'` (matches both words anywhere)
- Phrase: `MATCH '"error handling"'` (exact phrase)
- Prefix: `MATCH 'err*'` (prefix search)
- Column filter: N/A (single-column FTS table)
- Boolean: `MATCH 'error AND NOT warning'` (FTS5 supports AND, OR, NOT)

**Confidence:** HIGH -- bm25() and snippet() are built-in FTS5 auxiliary functions documented in official SQLite docs.

### Pattern 4: CLI Subcommand Architecture with Output Mode

**What:** Expand the existing clap `Commands` enum with new subcommands. Each subcommand supports a `--json` flag for machine-readable output and defaults to human-readable formatted output.

**Implementation:**

```rust
#[derive(Subcommand)]
enum Commands {
    /// Sync JSONL session files into the database
    Sync { /* existing */ },

    /// Search across all session content
    Search {
        /// Search query (FTS5 syntax)
        query: String,
        /// Maximum results to return
        #[arg(long, default_value = "20")]
        limit: usize,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },

    /// List sessions with filters
    Sessions {
        /// Filter by project path (substring match)
        #[arg(long)]
        project: Option<String>,
        /// Show sessions after this date (ISO8601)
        #[arg(long)]
        after: Option<String>,
        /// Show sessions before this date (ISO8601)
        #[arg(long)]
        before: Option<String>,
        /// Maximum sessions to return
        #[arg(long, default_value = "50")]
        limit: usize,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },

    /// Query messages with filters, output JSON to stdout
    Query {
        /// Filter by session ID
        #[arg(long)]
        session_id: Option<String>,
        /// Filter by message type (user, assistant)
        #[arg(long, name = "type")]
        message_type: Option<String>,
        /// Filter by model name
        #[arg(long)]
        model: Option<String>,
        /// Filter by tool name
        #[arg(long)]
        tool: Option<String>,
        /// Show messages after this date
        #[arg(long)]
        after: Option<String>,
        /// Show messages before this date
        #[arg(long)]
        before: Option<String>,
        /// Maximum results
        #[arg(long, default_value = "100")]
        limit: usize,
    },

    /// Show token usage, tool frequency, and model breakdown
    Stats {
        /// Filter by session ID
        #[arg(long)]
        session_id: Option<String>,
        /// Group by: session, day, model
        #[arg(long, default_value = "session")]
        group_by: String,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },

    /// Export a session to JSON, Markdown, or CSV
    Export {
        /// Session ID to export
        session_id: String,
        /// Output format: json, markdown, csv
        #[arg(long, default_value = "json")]
        format: String,
    },

    /// Show Claude Code version and drift
    VersionCheck {
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },

    /// Show schema drift events
    SchemaDrift {
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
}
```

**Confidence:** HIGH -- this is standard clap derive pattern, identical to the existing `Sync` variant.

### Pattern 5: Session Export Formats

**What:** Export a complete session conversation in three formats.

**JSON export:** Serialize the full session with all messages, content blocks, and usage stats as a single JSON object. Use `serde_json::to_string_pretty`.

**Markdown export:** Format as a readable conversation transcript:
```markdown
# Session: {session_id}
**Project:** {project_path}
**Date:** {first_seen_at}
**Model:** {model}

---

## User (2026-02-20 01:00:00)

Hello, Claude!

---

## Assistant (2026-02-20 01:01:00)

Here is my response.

### Tool Use: Read
**Input:** `{"file_path": "/tmp/test.txt"}`

---
```

**CSV export:** One row per message with columns: `uuid, session_id, type, timestamp, model, content_preview, input_tokens, output_tokens`. Use the `csv` crate for correct escaping. Content is truncated to 500 chars for the preview column.

**Confidence:** HIGH -- all three formats are straightforward serialization tasks.

### Pattern 6: Query Builder for Message Filtering

**What:** A struct that accumulates filter conditions and produces a parameterized SQL query.

**Implementation:**

```rust
pub struct MessageQuery {
    session_id: Option<String>,
    message_type: Option<String>,
    model: Option<String>,
    tool: Option<String>,
    after: Option<String>,
    before: Option<String>,
    limit: usize,
}

impl MessageQuery {
    pub fn to_sql(&self) -> (String, Vec<Box<dyn rusqlite::types::ToSql>>) {
        let mut conditions = Vec::new();
        let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

        if let Some(ref sid) = self.session_id {
            conditions.push(format!("m.session_id = ?{}", params.len() + 1));
            params.push(Box::new(sid.clone()));
        }
        // ... more conditions ...

        let where_clause = if conditions.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", conditions.join(" AND "))
        };

        let sql = format!(
            "SELECT m.uuid, m.session_id, m.type, m.timestamp, m.model,
                    m.stop_reason, m.version
             FROM messages m
             {} ORDER BY m.timestamp DESC LIMIT ?{}",
            where_clause,
            params.len() + 1
        );
        params.push(Box::new(self.limit as i64));

        (sql, params)
    }
}
```

**Confidence:** MEDIUM -- the pattern is well-understood but the dynamic parameter boxing with `Box<dyn ToSql>` can be awkward in rusqlite. An alternative is using `rusqlite::params_from_iter` with a `Vec<String>` and converting at the boundary. This needs testing during implementation.

### Anti-Patterns to Avoid

- **Creating FTS5 tables without `content=`:** Without external content mode, FTS5 duplicates all indexed text in shadow tables, roughly doubling storage. With message content including multi-thousand-line tool results, this could mean gigabytes of duplication.
- **Triggering FTS rebuild on every INSERT:** In external content mode, triggers on the content table can keep the index in sync, but during bulk sync this fires thousands of times. Rebuild-after-sync is more efficient.
- **Using `ORDER BY rank` without `bm25()`:** The implicit `rank` column in FTS5 uses the default ranking function. Using `bm25()` explicitly allows column weighting and produces better results.
- **Formatting output with `println!` inside `conn.call()`:** The `conn.call()` closure runs on the tokio-rusqlite background thread. Side effects like `println!` inside the closure work but mix I/O with DB operations. Collect results in the closure, return them, then format outside.
- **Building SQL with string interpolation:** Always use parameterized queries (`?1`, `?2`, ...) for user-provided values. The query builder must NEVER interpolate user strings directly into SQL.

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| Full-text search | Custom word indexing / inverted index | SQLite FTS5 | BM25 ranking, snippets, prefix search, boolean queries -- all built-in |
| CSV escaping | Manual comma/quote handling | `csv` crate (BurntSushi) | RFC 4180 compliance, handles newlines in fields, quoting edge cases |
| Date range parsing | Manual ISO8601 parsing | `chrono` | Time zones, partial dates, comparison operators |
| CLI argument parsing | Manual `std::env::args` | `clap` derive | Subcommands, help text, validation, shell completions |
| JSON pretty-printing | Manual formatting | `serde_json::to_string_pretty` | Handles nested structures, escaping, indentation |

**Key insight:** The complexity in this phase is in the SQL queries (JOINs across normalized tables, FTS5 MATCH expressions, aggregate statistics) and the output formatting, not in the infrastructure. The crates are all mature and well-tested.

## Common Pitfalls

### Pitfall 1: FTS5 External Content Table Out of Sync

**What goes wrong:** The FTS5 index returns stale or missing results because the external content table was modified without rebuilding the FTS index.

**Why it happens:** External content FTS5 tables do not automatically stay in sync with their content table. Inserts, updates, and deletes to `message_content` are invisible to the FTS index until a rebuild is triggered.

**How to avoid:** Call `rebuild_fts_index()` at the end of every `sync_all` invocation, before returning the result. Document this requirement clearly in the `fts.rs` module.

**Warning signs:** Search returns no results despite having data in `message_content`. Search returns results for old content that was re-synced. `SELECT count(*) FROM fts_message_content` returns 0 while `message_content` has rows.

### Pitfall 2: BM25 Score Direction (Lower = Better)

**What goes wrong:** Results are ordered worst-to-best because the developer assumes higher BM25 scores mean better matches.

**Why it happens:** SQLite FTS5's `bm25()` function returns negative values (multiplied by -1 internally), so LOWER (more negative) values indicate BETTER matches. `ORDER BY bm25(table)` naturally sorts best-first (most negative first).

**How to avoid:** Always use `ORDER BY bm25(table)` (ascending, which is default). Do NOT use `ORDER BY bm25(table) DESC`. When displaying rank to users, negate the value or use a label like "relevance" without showing the raw score.

**Warning signs:** The most relevant results appear at the bottom of the list.

### Pitfall 3: FTS5 MATCH Syntax Errors from User Input

**What goes wrong:** Users pass search queries with FTS5 special characters (colons, quotes, parentheses, AND/OR/NOT) that cause parse errors in the MATCH expression.

**Why it happens:** FTS5 has its own query language. Input like `error: not found` contains a colon (column filter syntax) and `not` (boolean operator). Input like `"unclosed quote` is a syntax error.

**How to avoid:** Sanitize user input before passing to MATCH. Two strategies:
1. **Quote the entire query:** Wrap user input in double quotes to treat as a phrase: `MATCH '"' || user_input || '"'`. This disables boolean operators and special syntax.
2. **Escape special characters:** Replace `"` with `""`, strip or escape `:`, `(`, `)`. This preserves some FTS5 syntax.

The simpler approach (option 1) is recommended for Phase 2. Advanced FTS5 query syntax can be exposed via a `--raw` flag later.

**Warning signs:** Crash on queries containing `:`, `"`, `(`, `)`, or FTS5 keywords. Error message: "fts5: syntax error near..."

### Pitfall 4: Query Result Set Size for Stats Aggregation

**What goes wrong:** `claude-history stats` takes a very long time or runs out of memory because it loads all token_usage rows into memory before aggregating.

**Why it happens:** Naive implementation: SELECT all rows, aggregate in Rust. Correct implementation: aggregate in SQL.

**How to avoid:** Use SQL aggregation:
```sql
SELECT
    m.model,
    COUNT(*) AS message_count,
    SUM(tu.input_tokens) AS total_input,
    SUM(tu.output_tokens) AS total_output,
    SUM(tu.cache_read_input_tokens) AS total_cache_read
FROM token_usage tu
JOIN messages m ON m.uuid = tu.message_uuid
GROUP BY m.model;
```

**Warning signs:** `stats` command takes > 1 second on a database with 10K+ messages.

### Pitfall 5: Export of Large Sessions Consuming Excessive Memory

**What goes wrong:** Exporting a large session (thousands of messages with multi-MB tool results) loads everything into memory as a single JSON object, causing OOM.

**Why it happens:** `serde_json::to_string_pretty(session_data)` where `session_data` holds all content.

**How to avoid:** For JSON export, stream objects using `serde_json::Serializer` writing directly to stdout. For Markdown, write message-by-message. For CSV, use `csv::Writer` which streams rows. For Phase 2, a practical compromise: load messages in batches (100 at a time) and write them to stdout incrementally, rather than loading the entire session at once.

**Warning signs:** `export` command crashes or hangs on sessions with > 5000 messages.

### Pitfall 6: CLI-02 (sync) Already Implemented -- Scope Overlap

**What goes wrong:** Phase 2 lists CLI-02 as a requirement, but `claude-history sync` was fully implemented in Phase 1 (Plan 01-04). Attempting to re-implement it creates confusion or regressions.

**Why it happens:** CLI-02 is mapped to Phase 2 in the roadmap traceability, but the actual sync CLI subcommand was built as part of the end-to-end integration in Phase 1.

**How to avoid:** Recognize CLI-02 as already satisfied. Phase 2's scope for CLI-02 is limited to verifying that the existing `sync` subcommand still works correctly after the Phase 2 changes (new migration, FTS rebuild). No new implementation needed.

## Code Examples

### FTS5 Migration DDL (002_fts5.sql)

```sql
-- Migration 002: FTS5 full-text search indexes.
--
-- Creates external-content FTS5 virtual tables that index text from
-- existing normalized tables without duplicating storage.

-- FTS5 index over message_content.text_content [FTS-01]
-- Indexes all text/thinking/tool_result content blocks.
-- tool_use blocks have tool_input in a separate column (not indexed here).
-- unicode61 tokenizer handles mixed code + natural language.
CREATE VIRTUAL TABLE fts_message_content USING fts5(
    text_content,
    content='message_content',
    content_rowid='id',
    tokenize='unicode61'
);

-- Note: FTS-02 (file_operations content index) is deferred.
-- The file_operations table does not exist yet -- it is created in Phase 5
-- (Artifact Layer). The FTS index for file operations will be added as part
-- of Phase 5's migration. This is not a scope reduction; the table that
-- FTS-02 references does not exist until Phase 5 creates it.
```

### Search Function (store/src/fts.rs)

```rust
use rusqlite::Connection;
use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct SearchResult {
    pub message_uuid: String,
    pub session_id: String,
    pub message_type: String,
    pub timestamp: String,
    pub block_type: String,
    pub snippet: String,
    pub rank: f64,
}

/// Rebuild the FTS5 index from the current content of message_content.
///
/// This must be called after sync operations to keep the FTS index
/// consistent with the external content table.
pub fn rebuild_fts_index(conn: &Connection) -> Result<(), rusqlite::Error> {
    conn.execute_batch(
        "INSERT INTO fts_message_content(fts_message_content) VALUES('rebuild');"
    )?;
    tracing::info!("FTS5 message content index rebuilt");
    Ok(())
}

/// Search across message content using FTS5 full-text search.
///
/// Returns ranked results with context snippets. The query is wrapped
/// in double quotes to sanitize user input (treats as phrase search).
/// Pass raw FTS5 queries via the `raw` parameter for advanced usage.
pub fn search_messages(
    conn: &Connection,
    query: &str,
    limit: usize,
    offset: usize,
) -> Result<Vec<SearchResult>, rusqlite::Error> {
    // Sanitize: wrap in quotes for phrase matching, escape internal quotes
    let safe_query = format!("\"{}\"", query.replace('"', "\"\""));

    let mut stmt = conn.prepare(
        "SELECT
            mc.message_uuid,
            m.session_id,
            m.type,
            m.timestamp,
            mc.block_type,
            snippet(fts_message_content, 0, '>>>', '<<<', '...', 30),
            bm25(fts_message_content)
         FROM fts_message_content
         JOIN message_content mc ON mc.id = fts_message_content.rowid
         JOIN messages m ON m.uuid = mc.message_uuid
         WHERE fts_message_content MATCH ?1
         ORDER BY bm25(fts_message_content)
         LIMIT ?2 OFFSET ?3"
    )?;

    let results = stmt.query_map(
        rusqlite::params![safe_query, limit as i64, offset as i64],
        |row| {
            Ok(SearchResult {
                message_uuid: row.get(0)?,
                session_id: row.get(1)?,
                message_type: row.get(2)?,
                timestamp: row.get(3)?,
                block_type: row.get(4)?,
                snippet: row.get(5)?,
                rank: row.get(6)?,
            })
        },
    )?;

    results.collect()
}
```

### Stats Aggregation Query (store/src/query.rs)

```rust
#[derive(Debug, Serialize)]
pub struct TokenStats {
    pub group_key: String,
    pub message_count: i64,
    pub total_input_tokens: i64,
    pub total_output_tokens: i64,
    pub total_cache_read: Option<i64>,
    pub total_cache_creation: Option<i64>,
}

pub fn token_stats_by_model(conn: &Connection) -> Result<Vec<TokenStats>, rusqlite::Error> {
    let mut stmt = conn.prepare(
        "SELECT
            m.model,
            COUNT(*) AS message_count,
            SUM(tu.input_tokens) AS total_input,
            SUM(tu.output_tokens) AS total_output,
            SUM(tu.cache_read_input_tokens) AS total_cache_read,
            SUM(tu.cache_creation_input_tokens) AS total_cache_creation
         FROM token_usage tu
         JOIN messages m ON m.uuid = tu.message_uuid
         GROUP BY m.model
         ORDER BY total_input DESC"
    )?;

    let results = stmt.query_map([], |row| {
        Ok(TokenStats {
            group_key: row.get(0)?,
            message_count: row.get(1)?,
            total_input_tokens: row.get(2)?,
            total_output_tokens: row.get(3)?,
            total_cache_read: row.get(4)?,
            total_cache_creation: row.get(5)?,
        })
    })?;

    results.collect()
}

pub fn tool_frequency(conn: &Connection) -> Result<Vec<(String, i64, i64)>, rusqlite::Error> {
    let mut stmt = conn.prepare(
        "SELECT
            te.tool_name,
            COUNT(*) AS invocations,
            SUM(CASE WHEN te.is_error = 1 THEN 1 ELSE 0 END) AS errors
         FROM tool_executions te
         GROUP BY te.tool_name
         ORDER BY invocations DESC"
    )?;

    let results = stmt.query_map([], |row| {
        Ok((row.get(0)?, row.get(1)?, row.get(2)?))
    })?;

    results.collect()
}
```

### Session Listing Query

```rust
#[derive(Debug, Serialize)]
pub struct SessionSummary {
    pub session_id: String,
    pub project_path: Option<String>,
    pub first_seen_at: Option<String>,
    pub version: Option<String>,
    pub message_count: i64,
    pub model: Option<String>,
}

pub fn list_sessions(
    conn: &Connection,
    project_filter: Option<&str>,
    after: Option<&str>,
    before: Option<&str>,
    limit: usize,
) -> Result<Vec<SessionSummary>, rusqlite::Error> {
    // Build dynamic WHERE clause
    let mut conditions = Vec::new();
    let mut param_values: Vec<String> = Vec::new();

    if let Some(project) = project_filter {
        conditions.push(format!("s.project_path LIKE ?{}", param_values.len() + 1));
        param_values.push(format!("%{}%", project));
    }
    if let Some(after) = after {
        conditions.push(format!("s.first_seen_at >= ?{}", param_values.len() + 1));
        param_values.push(after.to_string());
    }
    if let Some(before) = before {
        conditions.push(format!("s.first_seen_at <= ?{}", param_values.len() + 1));
        param_values.push(before.to_string());
    }

    let where_clause = if conditions.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", conditions.join(" AND "))
    };

    let sql = format!(
        "SELECT
            s.session_id,
            s.project_path,
            s.first_seen_at,
            s.version,
            COUNT(m.uuid) AS message_count,
            (SELECT model FROM messages
             WHERE session_id = s.session_id AND model IS NOT NULL
             LIMIT 1) AS primary_model
         FROM sessions s
         LEFT JOIN messages m ON m.session_id = s.session_id
         {}
         GROUP BY s.session_id
         ORDER BY s.first_seen_at DESC
         LIMIT ?{}",
        where_clause,
        param_values.len() + 1
    );
    param_values.push(limit.to_string());

    let params: Vec<&dyn rusqlite::types::ToSql> = param_values
        .iter()
        .map(|s| s as &dyn rusqlite::types::ToSql)
        .collect();

    let mut stmt = conn.prepare(&sql)?;
    let results = stmt.query_map(rusqlite::params_from_iter(params), |row| {
        Ok(SessionSummary {
            session_id: row.get(0)?,
            project_path: row.get(1)?,
            first_seen_at: row.get(2)?,
            version: row.get(3)?,
            message_count: row.get(4)?,
            model: row.get(5)?,
        })
    })?;

    results.collect()
}
```

## State of the Art

| Old Approach | Current Approach | When Changed | Impact |
|--------------|------------------|--------------|--------|
| FTS3/FTS4 | FTS5 | SQLite 3.9.0 (2015) | FTS5 is the recommended version; faster, better ranking, extensible tokenizers |
| Content-storing FTS | External content FTS5 (`content=`) | Available since FTS5 initial release | Halves storage for indexed text; requires explicit rebuild |
| Custom text search | FTS5 `MATCH` + `bm25()` | Stable | BM25 ranking provides relevance ordering without custom scoring |
| `prettytable-rs` for CLI output | `comfy-table` or manual formatting | 2024 | `prettytable-rs` is unmaintained; `comfy-table` is the current alternative, but manual formatting is sufficient for this use case |

**Deprecated/outdated:**
- FTS3/FTS4: Still functional but FTS5 is recommended for all new development
- `prettytable-rs`: Unmaintained since 2021; not recommended for new projects

## Open Questions

1. **FTS-02: file_operations table does not exist yet**
   - What we know: FTS-02 requires "FTS5 virtual table over file_operations content, old_content, command"
   - What's unclear: The `file_operations` table is created in Phase 5 (Artifact Layer, per ROADMAP.md). Phase 2 cannot create an FTS5 index over a table that does not exist.
   - Recommendation: Create the FTS5 index for `message_content` (FTS-01) in Phase 2. Defer the FTS5 index for `file_operations` (FTS-02) to Phase 5, where it naturally belongs alongside the table creation. The search endpoint (FTS-03) should be designed to query both FTS tables when they exist, falling back gracefully to message_content-only search when file_operations FTS is not yet available. This is not scope narrowing -- it is dependency-respecting sequencing.

2. **Search result ranking across multiple FTS tables**
   - What we know: FTS-03 says "ranked results across all indexed content." When both FTS tables exist (Phase 5+), results from `fts_message_content` and `fts_file_operations` need to be merged and ranked.
   - What's unclear: How to combine BM25 scores from different FTS5 tables (different corpus sizes produce different score scales).
   - Recommendation: For Phase 2, search only message content. Design the search API to return a `source` field indicating which FTS table matched, so that when file operations are added in Phase 5, results can be interleaved. Cross-table score normalization is a Phase 5 concern.

3. **CLI-02 already implemented**
   - What we know: `claude-history sync` was fully implemented in Phase 1 (Plan 01-04).
   - What's unclear: Whether the roadmap intended CLI-02 to cover additional sync features not in Phase 1.
   - Recommendation: Treat CLI-02 as satisfied by Phase 1. Phase 2 only needs to verify that sync still works after adding the FTS migration and rebuild step.

4. **Dynamic SQL query building with rusqlite parameter types**
   - What we know: rusqlite's `params!` macro requires known parameter count at compile time. Dynamic WHERE clauses need `params_from_iter` or boxed trait objects.
   - What's unclear: Whether `params_from_iter` with `Vec<&dyn ToSql>` handles all SQL types needed (String, i64, Option<String>).
   - Recommendation: Use `params_from_iter` with string parameters for all filter values (SQLite's type affinity handles the conversion). Test with mixed types early.

5. **Export format for Markdown -- how to represent tool_use and tool_result blocks**
   - What we know: Tool use blocks have structured JSON inputs. Tool result blocks can contain thousands of lines of file content.
   - What's unclear: How verbose the Markdown export should be for tool I/O.
   - Recommendation: For tool_use blocks, show tool name and a summary of the input (first 200 chars). For tool_result blocks, show the result status and a truncated preview (first 500 chars). Include a `--full` flag for complete output.

<phase_requirements>
## Phase Requirements

| ID | Description | Research Support |
|----|-------------|-----------------|
| FTS-01 | FTS5 virtual table over message_content text | External-content FTS5 table `fts_message_content` using `content='message_content'` and `content_rowid='id'`. Migration 002_fts5.sql creates it. `unicode61` tokenizer handles mixed content. |
| FTS-02 | FTS5 virtual table over file_operations content, old_content, command | DEFERRED TO PHASE 5 -- `file_operations` table does not exist until Phase 5 (Artifact Layer). The FTS index will be created alongside the table. This is dependency sequencing, not scope reduction. See Open Question 1. |
| FTS-03 | Search endpoint returning ranked results across all indexed content | `search_messages()` function using FTS5 MATCH with `bm25()` ranking and `snippet()` for context. CLI `search` subcommand wraps this. Designed to extend to file_operations when Phase 5 adds them. |
| CLI-02 | claude-history sync -- one-shot bulk import | ALREADY IMPLEMENTED in Phase 1 (Plan 01-04). Phase 2 adds FTS rebuild after sync completion. No new CLI work needed. |
| CLI-03 | claude-history query -- query messages with filters, output JSON to stdout | New `Query` subcommand with `--session-id`, `--type`, `--model`, `--tool`, `--after`, `--before` filters. Dynamic SQL WHERE clause builder. Always outputs JSON to stdout (no `--json` flag needed). |
| CLI-04 | claude-history sessions -- list sessions with filters | New `Sessions` subcommand with `--project`, `--after`, `--before`, `--limit` filters. Human-readable table by default, `--json` for JSON. Joins sessions with message count and primary model. |
| CLI-05 | claude-history search -- full-text search across all sessions | New `Search` subcommand accepting a query string. Uses `search_messages()` with BM25 ranking and snippet display. Human-readable by default, `--json` for structured output. |
| CLI-06 | claude-history stats -- token usage, tool frequency, model breakdown | New `Stats` subcommand with SQL aggregation queries. Shows: total tokens by model, tool invocation frequency with error rates, model usage distribution. Human-readable by default, `--json` for JSON. |
| CLI-07 | claude-history export -- export session to JSON/Markdown/CSV | New `Export` subcommand with `--format` flag (json, markdown, csv). Loads session messages in batches. JSON uses serde_json, CSV uses csv crate, Markdown uses manual formatting. |
| CLI-08 | claude-history version-check -- show Claude Code version and drift | New `VersionCheck` subcommand. Queries `messages` table for distinct versions ordered by timestamp. Shows version history and any version changes detected. |
| CLI-09 | claude-history schema drift -- show schema drift events | New `SchemaDrift` subcommand. Queries `schema_drift_log` table. Shows field name, record type, version, first seen date, and sample value. |
</phase_requirements>

## Sources

### Primary (HIGH confidence)
- [SQLite FTS5 Extension](https://www.sqlite.org/fts5.html) -- official SQLite FTS5 documentation. External content tables, bm25(), snippet(), highlight(), rebuild command, tokenizer options
- [libsqlite3-sys 0.35.0 build.rs](~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/libsqlite3-sys-0.35.0/build.rs) -- confirmed `-DSQLITE_ENABLE_FTS5` is set unconditionally in bundled build
- [rusqlite 0.37 docs](https://docs.rs/rusqlite/0.37.0/rusqlite/) -- Connection, params!, params_from_iter API
- [csv crate 1.3 (BurntSushi)](https://github.com/BurntSushi/rust-csv) -- CSV serialization with serde support
- [clap 4 derive docs](https://docs.rs/clap/latest/clap/_derive/index.html) -- subcommand derive pattern

### Secondary (MEDIUM confidence)
- [SQLite User Forum: External Content Tables](https://sqlite.org/forum/forumpost/acdc2aa30a) -- community discussion of external content FTS5 usage patterns
- [Sling Academy: Ranking FTS Results](https://www.slingacademy.com/article/ranking-full-text-search-results-in-sqlite-explained/) -- BM25 ranking explanation
- [Rust CLI Book: Machine Communication](https://rust-cli.github.io/book/in-depth/machine-communication.html) -- JSON output patterns for CLI tools

### Tertiary (LOW confidence)
- FTS5 rebuild performance on large databases: Rule of thumb (seconds for 100K+ rows). Needs empirical profiling with real data.
- `params_from_iter` with mixed types: Based on API review. Needs testing with actual rusqlite 0.37.

## Metadata

**Confidence breakdown:**
- Standard stack: HIGH -- all crates already in workspace or well-established (csv). FTS5 confirmed enabled in bundled SQLite build.
- Architecture (FTS5 patterns): HIGH -- based on official SQLite FTS5 documentation (external content, rebuild, bm25, snippet)
- Architecture (CLI patterns): HIGH -- extending existing clap derive pattern established in Phase 1
- Architecture (query builder): MEDIUM -- dynamic parameterized SQL with rusqlite needs implementation-time validation
- Pitfalls: HIGH -- FTS5 sync, BM25 direction, input sanitization are well-documented concerns

**Research date:** 2026-02-20
**Valid until:** 2026-03-20 (FTS5 is a stable SQLite feature; changes unlikely)
