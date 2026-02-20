# Phase 5: Artifact Layer - Research

**Researched:** 2026-02-20
**Domain:** File operation tracking, git operation extraction, tool result matching, content reconstruction, diff generation, FTS indexing
**Confidence:** HIGH

<spec_constraints>
## Spec Constraints (IMMUTABLE -- from ROADMAP.md Success Criteria)

**Success Criteria** (what must be TRUE):
  1. `claude-history files` lists every file touched by Claude Code across sessions, and `claude-history file-history <path>` shows the chronological Write/Edit/Read operations on that file with content
  2. `claude-history reconstruct <file-path> --at <message-uuid>` replays writes and edits to produce the file's content as it existed at that point in the session
  3. `claude-history git-log` shows git operations extracted from Bash tool calls, with commit messages, branches, and operation types correctly parsed
  4. GET /v1/artifacts/:session_id/timeline returns a chronological feed of all file writes, edits, git commits, and tool outputs for a session
  5. tool_use blocks in assistant messages are correctly linked to their tool_result blocks in subsequent user messages by tool_use_id, and file:written / git:commit SSE events fire during live ingestion

**Requirements**: FTS-02, ART-01, ART-02, ART-03, ART-04, ART-05, ART-06, ART-07, ART-08, ART-09, ART-10, ART-11, API-17, API-18, API-19, API-20, API-21, API-22, API-23, API-24, API-25, API-26, API-27, CLI-10, CLI-11, CLI-12, CLI-13, CLI-14, SSE-06, SSE-07

These are non-negotiable user-story outcomes. Research recommendations must not narrow below these.
</spec_constraints>

## Summary

Phase 5 is the largest and most architecturally complex phase in the project. It adds three new database tables (files, file_operations, git_operations), an artifact decomposition pipeline that extracts structured data from existing tool_use/tool_result records, tool result matching that links assistant and user message blocks by tool_use_id, file content reconstruction via ordered edit replay, unified diff generation, FTS5 indexing over file operation content, 11 new HTTP API endpoints, 5 new CLI subcommands, and 2 new SSE event types.

The core challenge is the artifact decomposition pipeline. Claude Code's tool_use blocks in assistant messages contain structured JSON inputs for Write (file_path, content), Edit (file_path, old_string, new_string), Read (file_path), and Bash (command, description) tools. These inputs are already stored as JSON in the `tool_executions.input_json` and `message_content.tool_input` columns from Phase 1 decomposition. Phase 5's artifact decomposer must parse these JSON inputs, extract file paths and content, match git operations via regex on Bash commands, and insert rows into the three new tables. This decomposition should run both during bulk sync (retroactively processing existing data) and during live ingestion (emitting SSE events). The tool_use-to-tool_result matching needed by ART-04 is already structurally supported: both tables store the `tool_use_id` field, and user records carry a `sourceToolAssistantUUID` field that equals the assistant record's UUID.

File content reconstruction (ART-10) replays Write and Edit operations in timestamp order. A Write operation sets the full file content. An Edit operation applies a string replacement (old_string -> new_string) to the current state. This is deterministic and does not require a diff algorithm -- it is pure string replacement replay. Diff generation (ART-11) uses the `similar` crate's `TextDiff::from_lines().unified_diff()` API to produce standard unified diffs between consecutive file states.

**Primary recommendation:** Implement artifact decomposition as a second-pass function that runs after the existing `decompose_record` pipeline, operating on the same transaction. For bulk retroactive processing, add a `decompose_artifacts_retroactive` function that queries existing `tool_executions` and `message_content` rows. Place the artifact decomposer in a new `artifacts.rs` module in the store crate. Use `similar` 2.7.0 for diff generation and `glob` 0.3.3 for file path pattern matching in query endpoints.

## Standard Stack

### Core
| Library | Version | Purpose | Why Standard |
|---------|---------|---------|--------------|
| similar | 2.7.0 | Unified diff generation from old/new text content | De facto Rust diff library by mitsuhiko. Provides TextDiff::from_lines() with unified_diff() output. Used by insta (snapshot testing). Dependency-free. |
| glob | 0.3.3 | File path glob pattern matching for POST /v1/files/query | Standard Unix glob matching in Rust. Mature, widely used, matches the "glob pattern support" requirement in API-22. |

### Supporting
| Library | Version | Purpose | When to Use |
|---------|---------|---------|-------------|
| regex | 1.x | Git command pattern matching for extracting commit messages, branches, operation types from Bash commands | Needed for ART-08 git operation parsing. Already available as transitive dependency of many workspace crates, but should be declared explicitly. |
| serde_json | 1.0 (workspace) | Parsing tool_use input JSON to extract file_path, content, old_string, new_string, command fields | Already in workspace. Used for all input JSON parsing in the artifact decomposer. |

### Alternatives Considered
| Instead of | Could Use | Tradeoff |
|------------|-----------|----------|
| similar 2.7.0 | diffy 0.4.2 | diffy produces unified diffs directly but has fewer features. similar has broader ecosystem adoption (used by insta), actively maintained, and provides both line-level and character-level diffs. |
| similar 2.7.0 | imara-diff | imara-diff is faster in pathological cases but lacks the high-level unified diff formatting API that similar provides. For our use case (typically small files, not performance-critical), the API convenience of similar wins. |
| glob 0.3.3 | globset 0.4.18 | globset is faster for matching multiple patterns against a single path. glob is simpler for our case (one pattern against many paths in SQL query results). |
| regex | hand-rolled string parsing | Git commands follow complex patterns (HEREDOC commit messages, chained && commands, quoted arguments). Regex is more maintainable and testable than hand-rolled parsing for this domain. |

**Installation:**
```toml
# Workspace Cargo.toml additions
similar = "2.7"
glob = "0.3"
regex = "1"
```

## Architecture Patterns

### Recommended Module Structure
```
crates/
├── store/
│   ├── migrations/
│   │   └── 003_artifacts.sql        # NEW: files, file_operations, git_operations tables + FTS5
│   ├── src/
│   │   ├── artifacts.rs             # NEW: artifact decomposer (parse tool inputs, extract operations)
│   │   ├── artifact_queries.rs      # NEW: query functions for files, git, artifacts, reconstruction
│   │   ├── decompose.rs             # MODIFIED: call artifact decomposer after record decomposition
│   │   ├── schema.rs                # MODIFIED: add ("003", include_str!("../migrations/003_artifacts.sql"))
│   │   ├── fts.rs                   # MODIFIED: add rebuild_fts_file_operations + search_file_operations
│   │   ├── sync.rs                  # MODIFIED: retroactive artifact decomposition on bulk sync
│   │   └── lib.rs                   # MODIFIED: pub mod artifacts; pub mod artifact_queries;
│   └── Cargo.toml                   # MODIFIED: add similar, glob, regex
├── server/src/
│   ├── api/
│   │   ├── files.rs                 # NEW: handlers for API-17 through API-22
│   │   ├── git.rs                   # NEW: handlers for API-23 through API-25
│   │   ├── artifacts.rs             # NEW: handlers for API-26 and API-27
│   │   └── mod.rs                   # MODIFIED: add routes for 11 new endpoints
│   ├── events.rs                    # MODIFIED: add FileWritten, FileEdited, GitCommit SSE variants
│   ├── watcher.rs                   # MODIFIED: call artifact decomposer, emit file/git SSE events
│   ├── main.rs                      # MODIFIED: add 5 new CLI subcommands
│   └── output.rs                    # MODIFIED: add output formatters for files, git-log, artifacts
└── core/src/                        # NO CHANGES (tool input shapes are already JSON blobs)
```

### Pattern 1: Artifact Decomposition as Second-Pass Processing
**What:** After the existing `decompose_record` completes (inserting messages, content, tool_executions), a separate `decompose_artifacts` function examines the same record's tool_use blocks, parses their input JSON, and inserts rows into files/file_operations/git_operations tables. This runs in the same transaction.
**When to use:** For every assistant record during sync/live ingestion.
**Why:** Keeps the existing decomposition pipeline clean and unchanged. Artifact decomposition is additive -- it reads tool_use blocks that were already stored by Phase 1 decomposition. The same transaction guarantees atomicity. The artifact decomposer can also run retroactively against existing data.
**Example:**
```rust
// In decompose.rs, after decompose_assistant:
pub fn decompose_record(
    record: &JSONLRecord,
    session_id_from_file: &str,
    tx: &Transaction,
) -> Result<DecomposeResult, DecomposeError> {
    let mut result = match record {
        JSONLRecord::User(r) => decompose_user(r, tx)?,
        JSONLRecord::Assistant(r) => decompose_assistant(r, tx)?,
        // ... other types
    };

    // Second pass: artifact extraction from tool_use blocks
    result.rows_inserted += artifacts::decompose_artifacts(record, session_id_from_file, tx)?;

    Ok(result)
}
```

### Pattern 2: Tool_Use Input Parsing by Tool Name
**What:** The artifact decomposer dispatches on the tool name from each tool_use block. Each tool has a known input schema that can be parsed from the serde_json::Value.
**When to use:** For every tool_use content block in assistant messages and every tool_result block in the subsequent user message.
**Known input shapes (verified from real JSONL data):**
```rust
// Write tool: { "file_path": "/abs/path", "content": "full file content" }
// Edit tool:  { "file_path": "/abs/path", "old_string": "...", "new_string": "...", "replace_all": bool }
// Read tool:  { "file_path": "/abs/path" }  (optional: "limit", "offset")
// Bash tool:  { "command": "...", "description": "..." }  (description is optional)
```
**Example:**
```rust
fn extract_file_operation(
    tool_name: &str,
    input: &serde_json::Value,
    tool_use_id: &str,
    message_uuid: &str,
    session_id: &str,
    timestamp: &str,
    tx: &Transaction,
) -> Result<usize, DecomposeError> {
    match tool_name {
        "Write" => {
            let file_path = input.get("file_path").and_then(|v| v.as_str());
            let content = input.get("content").and_then(|v| v.as_str());
            if let (Some(fp), Some(c)) = (file_path, content) {
                upsert_file(session_id, fp, timestamp, tx)?;
                insert_file_operation(session_id, fp, "write", Some(c), None, tool_use_id, message_uuid, timestamp, tx)?;
            }
            Ok(1)
        }
        "Edit" => {
            let file_path = input.get("file_path").and_then(|v| v.as_str());
            let old_string = input.get("old_string").and_then(|v| v.as_str());
            let new_string = input.get("new_string").and_then(|v| v.as_str());
            if let (Some(fp), Some(old), Some(new_s)) = (file_path, old_string, new_string) {
                upsert_file(session_id, fp, timestamp, tx)?;
                insert_file_operation(session_id, fp, "edit", Some(new_s), Some(old), tool_use_id, message_uuid, timestamp, tx)?;
            }
            Ok(1)
        }
        // ... Read, Bash patterns
    }
}
```

### Pattern 3: Git Command Regex Extraction
**What:** Bash tool_use commands are matched against regex patterns to identify git operations. Real data shows commit messages use HEREDOC syntax: `git commit -m "$(cat <<'EOF'\nmessage\nEOF\n)"`. Branch extraction uses patterns like `git checkout -b <branch>`, `git push origin <branch>`.
**When to use:** For every Bash tool_use block.
**Key patterns verified from real JSONL data:**
```rust
// Git commit with HEREDOC (most common pattern in real data):
//   git add <files> && git commit -m "$(cat <<'EOF'\n...\nEOF\n)"
// Git commit with inline message:
//   git commit -m "message"
// Git operations:
//   git push, git checkout, git branch, git merge, git rebase, git stash, git pull
// Git status/log (read-only, still worth tracking):
//   git status, git log, git diff
lazy_static! {
    static ref GIT_CMD_RE: Regex = Regex::new(r"(?:^|\s|&&\s*)git\s+(\w+)").unwrap();
    // HEREDOC commit message extraction
    static ref HEREDOC_MSG_RE: Regex = Regex::new(
        r#"git\s+commit\s+.*?-m\s+"\$\(cat\s+<<'?EOF'?\n([\s\S]*?)\nEOF"#
    ).unwrap();
    // Inline commit message
    static ref INLINE_MSG_RE: Regex = Regex::new(
        r#"git\s+commit\s+.*?-m\s+"([^"]+)""#
    ).unwrap();
    // Branch from checkout -b or push
    static ref BRANCH_RE: Regex = Regex::new(
        r"git\s+(?:checkout\s+-b|push\s+\w+)\s+(\S+)"
    ).unwrap();
}
```

### Pattern 4: File Content Reconstruction via Ordered Replay
**What:** Reconstruct a file's content at any point in time by replaying Write and Edit operations in timestamp order up to the target message UUID.
**When to use:** For CLI-12 `reconstruct` and API-19 `GET /v1/files/:file_id/content?at=<uuid>`.
**Algorithm:**
1. Query file_operations for the given file path, ordered by timestamp
2. Filter to operations at or before the target message UUID's timestamp
3. Start with empty content
4. For each "write" operation: replace content entirely
5. For each "edit" operation: apply string replacement (old_string -> new_string)
6. Return final content

**Example:**
```rust
pub fn reconstruct_file_content(
    conn: &Connection,
    file_path: &str,
    session_id: &str,
    at_message_uuid: Option<&str>,
) -> Result<Option<String>, rusqlite::Error> {
    // Get the timestamp cutoff from the message UUID, if provided
    let cutoff_timestamp = if let Some(uuid) = at_message_uuid {
        conn.query_row(
            "SELECT timestamp FROM messages WHERE uuid = ?1",
            [uuid],
            |row| row.get::<_, String>(0),
        ).optional()?
    } else {
        None
    };

    // Query operations in order
    let ops = query_file_operations_ordered(conn, file_path, session_id, cutoff_timestamp.as_deref())?;

    let mut content: Option<String> = None;
    for op in &ops {
        match op.operation_type.as_str() {
            "write" => {
                content = op.content.clone();
            }
            "edit" => {
                if let (Some(ref mut c), Some(ref old), Some(ref new_s)) =
                    (&mut content, &op.old_content, &op.content) {
                    *c = c.replace(old, new_s);
                }
            }
            _ => {} // read, bash_file_op are not mutations
        }
    }

    Ok(content)
}
```

### Pattern 5: Tool Result Matching (ART-04)
**What:** Link tool_use blocks from assistant messages to their corresponding tool_result blocks in subsequent user messages using the shared tool_use_id field.
**Current state:** The existing `tool_executions` table has a `result_content` column that is DEFINED but NEVER POPULATED. Phase 1 decomposition inserts tool_use data (from assistant records) into tool_executions but does not update the row when the corresponding tool_result arrives (in the next user record). The linkage data exists: user records with tool_result blocks carry a `sourceToolAssistantUUID` field that equals the assistant record's UUID, and both tool_use and tool_result share the same `tool_use_id`.
**Implementation:** During user record decomposition, when processing tool_result blocks, UPDATE the existing tool_executions row to populate result_content and is_error:
```rust
// In decompose_content_block, for ToolResult:
ContentBlock::ToolResult { tool_use_id, content, is_error } => {
    // ... existing message_content insert ...

    // ART-04: Update tool_executions with the result
    tx.execute(
        "UPDATE tool_executions SET result_content = ?1, is_error = ?2
         WHERE tool_use_id = ?3",
        rusqlite::params![content_str, is_error.map(|v| v as i32), tool_use_id],
    )?;
}
```

### Pattern 6: Bash File-Touching Command Detection (ART-09)
**What:** Parse Bash tool_use commands for file-touching operations (cp, mv, rm, mkdir, touch).
**Patterns from real data:** Real Bash commands are often chained (`rm -rf .ccode-intel && ./bin/cli.js init`) and may contain paths with spaces or special characters.
**Approach:** Use regex to detect the operation type and extract file paths. These are lower-confidence extractions compared to Write/Edit (which have structured JSON inputs), so the file_operations rows should carry an `extraction_confidence` or the operation_type should distinguish them (e.g., "bash_cp", "bash_rm" vs "write", "edit").

### Anti-Patterns to Avoid
- **Full content storage for Read operations:** ART-07 specifies Read operations insert into file_operations but produce "no mutation." Do NOT store the full file content returned by Read -- it would massively inflate the database. Store only the file_path and the fact that a read occurred.
- **Re-parsing JSONL for artifact decomposition:** Do not re-read JSONL files. The tool_use input JSON is already stored in `tool_executions.input_json` and `message_content.tool_input`. The artifact decomposer should parse from those columns for retroactive processing, and from the in-memory record during live decomposition.
- **Blocking diff generation on every edit:** Diff generation (ART-11) should be computed on-demand when the API endpoint is called, not pre-computed and stored. File edits can be numerous, and unified diffs are cheap to compute from stored old_string/new_string pairs.
- **Regex for all Bash parsing:** Not every Bash command needs parsing. Only match known patterns (git *, cp, mv, rm, mkdir, touch). Unknown commands should be silently ignored -- there are thousands of possible Bash commands and parsing them all would be an infinite rabbit hole.

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| Unified diff generation | Custom diff algorithm | `similar` 2.7.0 `TextDiff::from_lines().unified_diff()` | Diff algorithms are subtle (O(n*d) vs O(n^2), edge cases with empty lines, trailing newlines). similar handles all this correctly. |
| File path glob matching | Custom wildcard matching | `glob::Pattern::matches_with()` | Glob matching has many edge cases (**, character classes, escaped characters). The standard `glob` crate handles them all. |
| Git command parsing | Simple string contains/starts_with | `regex` crate with compiled patterns | Git commands have complex syntax (HEREDOC messages, chained commands, quoted arguments). Regex is the right tool for structured text extraction from semi-structured strings. |
| FTS5 virtual table | Custom text search | SQLite FTS5 external-content (same pattern as Phase 2) | FTS5 provides BM25 ranking, snippet generation, and phrase matching out of the box. |

**Key insight:** The artifact layer's complexity is in orchestration (linking records, ordering operations, replaying edits), not in the individual algorithms. Each algorithm component has a well-tested library solution. The engineering challenge is wiring them together correctly within the existing decomposition pipeline and ensuring retroactive processing handles all existing data.

## Common Pitfalls

### Pitfall 1: Retroactive Decomposition Ordering
**What goes wrong:** Running artifact decomposition retroactively on existing data requires processing records in the correct order (assistant before user) to ensure tool_use records exist before tool_result matching attempts to UPDATE them.
**Why it happens:** The existing decomposition pipeline processes records in file order (which IS chronological for JSONL), but a retroactive bulk pass over existing data might not maintain this ordering if querying from multiple tables.
**How to avoid:** Query existing tool_executions joined with messages, ordered by timestamp. Process in chronological order. Or, use INSERT OR IGNORE for artifact rows (like existing decomposition) and run the retroactive pass in a single ordered query.
**Warning signs:** tool_executions.result_content is NULL for records where the user response is known to exist.

### Pitfall 2: Edit Replay Ambiguity
**What goes wrong:** Edit operations use `old_string.replace(old, new)` but old_string might appear multiple times in the file content. The `replace_all` field on Edit tool inputs controls whether to replace all occurrences or just the first.
**Why it happens:** The Edit tool in Claude Code uses exact string matching, not line-number-based edits. If old_string is not unique in the file, the wrong occurrence could be replaced during reconstruction.
**How to avoid:** Store the `replace_all` flag from Edit tool inputs. During reconstruction, use `replacen(old, new, 1)` for replace_all=false and `replace(old, new)` for replace_all=true. Note: Claude Code's Edit tool will fail if old_string is not unique and replace_all is false, so in practice this should not cause reconstruction errors for successful edits.
**Warning signs:** Reconstructed file content differs from what the user actually had. Test with files that have repeated patterns.

### Pitfall 3: HEREDOC Commit Message Extraction
**What goes wrong:** The project's commit convention (visible in real data and CLAUDE.md) uses HEREDOC syntax for git commit messages: `git commit -m "$(cat <<'EOF'\n...\nEOF\n)"`. A naive regex looking for `-m "..."` will fail to capture these multi-line messages.
**Why it happens:** HEREDOC is a shell feature that allows multi-line strings. The commit message is not on the same line as the git command.
**How to avoid:** Use a multiline regex that captures content between `<<'EOF'` and `EOF`. Also handle the simpler `-m "inline message"` pattern as a fallback. Process both patterns in order of specificity (HEREDOC first, inline second).
**Warning signs:** commit_message is empty or truncated for commits that should have messages. Test with the actual HEREDOC format from real data.

### Pitfall 4: Chained Bash Commands
**What goes wrong:** Real Bash tool_use commands are often chained with `&&`: `git add file.rs && git commit -m "msg"`. A parser that looks for "the git command" will only find the first one.
**Why it happens:** Claude Code chains git add + git commit in a single Bash tool invocation.
**How to avoid:** Split on `&&` and `;` before applying git pattern matching. Process each sub-command independently. Be aware that `||` is also a command separator but semantically different (only runs if previous fails).
**Warning signs:** git add operations are extracted but not the accompanying commit.

### Pitfall 5: Database Size Growth from Content Storage
**What goes wrong:** Storing full file content for every Write operation and old_string/new_string for every Edit operation can significantly increase database size.
**Why it happens:** Large files (multi-hundred-KB source files, entire package-lock.json, etc.) may be written via the Write tool.
**How to avoid:** This is a requirement (ART-02 specifies "content" column on file_operations, and ART-10 reconstruction depends on it). Accept the storage cost. Consider adding a CLI flag or config to optionally skip content storage for files matching certain patterns (e.g., lock files) in a future version, but for v1, store everything.
**Warning signs:** Database grows much faster after artifact layer is enabled. Monitor db_size in health endpoint.

### Pitfall 6: FTS5 Rebuild Timing for File Operations
**What goes wrong:** The existing FTS5 index rebuild (for message_content) runs on a 30-second timer in the watcher loop. Adding a second FTS5 index for file_operations content means the rebuild must cover both indices.
**Why it happens:** External-content FTS5 tables require explicit rebuild commands. The existing timer handles message content; file operations content needs the same treatment.
**How to avoid:** Add `rebuild_fts_file_operations` alongside the existing `rebuild_fts_index` call in the watcher loop's periodic FTS rebuild timer. Both rebuilds should run in the same batch.
**Warning signs:** File content search returns stale or missing results after live ingestion.

## Code Examples

### Migration 003: Artifact Tables
```sql
-- files: one row per unique file path per session [ART-01]
CREATE TABLE files (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id    TEXT NOT NULL REFERENCES sessions(session_id),
    file_path     TEXT NOT NULL,
    first_seen    TEXT NOT NULL,
    last_modified TEXT NOT NULL,
    operation_count INTEGER NOT NULL DEFAULT 0,
    UNIQUE(session_id, file_path)
);

-- file_operations: every file touch operation [ART-02]
CREATE TABLE file_operations (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id      TEXT NOT NULL REFERENCES sessions(session_id),
    file_path       TEXT NOT NULL,
    operation_type  TEXT NOT NULL,  -- write, edit, read, bash_cp, bash_mv, bash_rm, bash_mkdir, bash_touch
    content         TEXT,           -- full content for write, new_string for edit, NULL for read
    old_content     TEXT,           -- old_string for edit, NULL for write/read
    command         TEXT,           -- Bash command for bash_* operations
    result_summary  TEXT,           -- tool_result content (truncated)
    is_error        INTEGER,
    tool_use_id     TEXT,
    message_uuid    TEXT REFERENCES messages(uuid),
    timestamp       TEXT NOT NULL,
    UNIQUE(tool_use_id)  -- Each tool_use produces at most one file operation
);

-- git_operations: extracted from Bash commands [ART-03]
CREATE TABLE git_operations (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id      TEXT NOT NULL REFERENCES sessions(session_id),
    operation_type  TEXT NOT NULL,  -- commit, push, checkout, branch, merge, rebase, stash, pull, status, log, diff, add
    command         TEXT NOT NULL,  -- full Bash command
    commit_message  TEXT,           -- extracted for commit operations
    branch          TEXT,           -- extracted where detectable
    tool_use_id     TEXT,
    message_uuid    TEXT REFERENCES messages(uuid),
    timestamp       TEXT NOT NULL,
    UNIQUE(tool_use_id, operation_type)  -- One tool_use can produce multiple git ops (add && commit)
);

-- Indexes for query performance
CREATE INDEX idx_files_session_id ON files(session_id);
CREATE INDEX idx_files_file_path ON files(file_path);
CREATE INDEX idx_file_operations_session_id ON file_operations(session_id);
CREATE INDEX idx_file_operations_file_path ON file_operations(file_path);
CREATE INDEX idx_file_operations_timestamp ON file_operations(timestamp);
CREATE INDEX idx_file_operations_tool_use_id ON file_operations(tool_use_id);
CREATE INDEX idx_git_operations_session_id ON git_operations(session_id);
CREATE INDEX idx_git_operations_operation_type ON git_operations(operation_type);
CREATE INDEX idx_git_operations_timestamp ON git_operations(timestamp);

-- FTS5 index for file operations content search [FTS-02]
CREATE VIRTUAL TABLE fts_file_operations USING fts5(
    content_col,
    old_content_col,
    command_col,
    content='file_operations',
    content_rowid='id',
    tokenize='unicode61'
);
```

### Artifact Decomposer Core Logic
```rust
// Source: store/src/artifacts.rs

use claude_history_core::message::ContentBlock;
use claude_history_core::record::{AssistantRecord, JSONLRecord, UserRecord};
use regex::Regex;
use rusqlite::Transaction;

/// Extract artifacts from a decomposed record.
///
/// Called after the standard decompose_record pipeline, in the same transaction.
/// For assistant records: parse tool_use blocks for Write/Edit/Read/Bash operations.
/// For user records: match tool_result blocks to update tool_executions.result_content.
pub fn decompose_artifacts(
    record: &JSONLRecord,
    session_id: &str,
    tx: &Transaction,
) -> Result<usize, crate::decompose::DecomposeError> {
    match record {
        JSONLRecord::Assistant(r) => decompose_assistant_artifacts(r, tx),
        JSONLRecord::User(r) => decompose_user_artifacts(r, tx),
        _ => Ok(0), // Other record types have no artifacts
    }
}

fn decompose_assistant_artifacts(
    r: &AssistantRecord,
    tx: &Transaction,
) -> Result<usize, crate::decompose::DecomposeError> {
    let mut rows = 0;
    for block in &r.message.content {
        if let ContentBlock::ToolUse { id, name, input, .. } = block {
            match name.as_str() {
                "Write" => rows += extract_write_operation(
                    &r.base.session_id, id, &r.base.uuid, &r.base.timestamp, input, tx
                )?,
                "Edit" => rows += extract_edit_operation(
                    &r.base.session_id, id, &r.base.uuid, &r.base.timestamp, input, tx
                )?,
                "Read" => rows += extract_read_operation(
                    &r.base.session_id, id, &r.base.uuid, &r.base.timestamp, input, tx
                )?,
                "Bash" => rows += extract_bash_operations(
                    &r.base.session_id, id, &r.base.uuid, &r.base.timestamp, input, tx
                )?,
                _ => {} // Other tools have no file/git artifacts
            }
        }
    }
    Ok(rows)
}
```

### Unified Diff Generation with `similar`
```rust
// Source: store/src/artifact_queries.rs

use similar::TextDiff;

/// Generate a unified diff of all edits to a file within a session.
///
/// Reconstructs the file state before and after each edit operation,
/// producing a combined unified diff showing all changes. [ART-11]
pub fn generate_file_diff(
    conn: &Connection,
    file_path: &str,
    session_id: &str,
) -> Result<String, rusqlite::Error> {
    let ops = query_file_operations_ordered(conn, file_path, session_id, None)?;

    let mut diffs = Vec::new();
    let mut current_content = String::new();

    for op in &ops {
        match op.operation_type.as_str() {
            "write" => {
                if let Some(ref content) = op.content {
                    let old = current_content.clone();
                    current_content = content.clone();
                    if !old.is_empty() {
                        let diff = TextDiff::from_lines(&old, &current_content);
                        let udiff = diff.unified_diff()
                            .context_radius(3)
                            .header(&format!("a/{}", file_path), &format!("b/{}", file_path));
                        diffs.push(format!("{}", udiff));
                    }
                }
            }
            "edit" => {
                if let (Some(ref old_str), Some(ref new_str)) = (&op.old_content, &op.content) {
                    let old = current_content.clone();
                    current_content = current_content.replace(old_str, new_str);
                    let diff = TextDiff::from_lines(&old, &current_content);
                    let udiff = diff.unified_diff()
                        .context_radius(3)
                        .header(&format!("a/{}", file_path), &format!("b/{}", file_path));
                    diffs.push(format!("{}", udiff));
                }
            }
            _ => {} // read operations do not produce diffs
        }
    }

    Ok(diffs.join("\n"))
}
```

### SSE Events for File and Git Operations
```rust
// Source: server/src/events.rs (additions to existing enum)

// Following the existing SseEvent pattern (manual event_type/to_json_data methods):
pub enum SseEvent {
    // ... existing variants ...

    /// A file was written or created via Write tool [SSE-06]
    FileWritten {
        session_id: String,
        file_path: String,
        message_uuid: String,
    },
    /// A file was edited via Edit tool [SSE-06]
    FileEdited {
        session_id: String,
        file_path: String,
        message_uuid: String,
    },
    /// A git commit was extracted from a Bash tool call [SSE-07]
    GitCommit {
        session_id: String,
        commit_message: Option<String>,
        branch: Option<String>,
        message_uuid: String,
    },
}

impl SseEvent {
    pub fn event_type(&self) -> &'static str {
        match self {
            // ... existing ...
            SseEvent::FileWritten { .. } => "file:written",
            SseEvent::FileEdited { .. } => "file:edited",
            SseEvent::GitCommit { .. } => "git:commit",
        }
    }
}
```

### Retroactive Artifact Decomposition
```rust
// Source: store/src/artifacts.rs

/// Process all existing tool_executions that have not yet been decomposed
/// into artifact tables. This is called during bulk sync to handle data
/// that was ingested before the artifact layer existed.
///
/// Uses the tool_use_id UNIQUE constraint on file_operations/git_operations
/// tables for idempotency -- re-running this function on already-decomposed
/// data produces no duplicates.
pub fn decompose_artifacts_retroactive(
    conn: &Connection,
) -> Result<usize, crate::decompose::DecomposeError> {
    // Query all tool_executions with their message context
    let mut stmt = conn.prepare(
        "SELECT te.tool_use_id, te.tool_name, te.input_json, te.result_content, te.is_error,
                m.uuid, m.session_id, m.timestamp
         FROM tool_executions te
         JOIN messages m ON m.uuid = te.message_uuid
         WHERE te.tool_name IN ('Write', 'Edit', 'Read', 'Bash')
         ORDER BY m.timestamp ASC"
    )?;

    let tx = conn.unchecked_transaction()?;
    let mut total_rows = 0;

    // Process each tool execution...
    // (uses INSERT OR IGNORE for idempotency)

    tx.commit()?;
    Ok(total_rows)
}
```

## State of the Art

| Old Approach | Current Approach | When Changed | Impact |
|--------------|------------------|--------------|--------|
| tool_executions.result_content always NULL | Populate via UPDATE during user record decomposition | Phase 5 (ART-04) | Enables tool_use -> tool_result matching without post-hoc joins |
| file_history_snapshot records silently skipped | Artifact decomposer extracts structured file operations from tool_use inputs | Phase 5 (ART-01 through ART-09) | File operations are now first-class queryable data |
| FTS5 only on message_content | FTS5 also on file_operations content | Phase 5 (FTS-02) | File contents and commands are searchable |

**Deprecated/outdated:**
- The `decompose_file_history_snapshot` function that currently logs at debug level and skips decomposition (line 580-601 in decompose.rs) can optionally be updated to decompose snapshot data into file operations, but this is secondary to the primary artifact decomposition from tool_use blocks. The file-history-snapshot records contain backup metadata, not the operational history that the artifact layer primarily tracks.

## Open Questions

1. **Should retroactive artifact decomposition run automatically on first `serve` after migration, or require explicit `sync`?**
   - What we know: The migration creates the tables. Existing data in tool_executions has the raw JSON inputs needed for decomposition.
   - What's unclear: Whether users expect `serve` to automatically backfill artifacts, or whether they should run `sync` again.
   - Recommendation: Run retroactive decomposition as part of the `sync` command and also on the first `serve` startup after migration 003 is applied. The migration can set a flag in sync_metadata or schema_versions that the sync/serve logic checks.

2. **How large will the file_operations table grow for heavy Claude Code users?**
   - What we know: A single session can have hundreds of Write/Edit operations. Content can be large (full source files for Write).
   - What's unclear: Real-world database size impact.
   - Recommendation: Accept the storage cost for v1. The health endpoint already reports db_size. If size becomes problematic, content storage can be made opt-out in v2.

3. **Should the artifacts timeline (API-27) include tool_result content in the response?**
   - What we know: The timeline is "all file writes, edits, git commits, and tool outputs for a session."
   - What's unclear: Whether "tool outputs" means the full tool_result content or just a summary.
   - Recommendation: Include a truncated result_summary (first 500 chars) in the timeline. Full content available via the file_operations or tool_executions endpoints.

<phase_requirements>
## Phase Requirements

| ID | Description | Research Support |
|----|-------------|-----------------|
| FTS-02 | FTS5 virtual table over file_operations content, old_content, command | Migration 003 creates fts_file_operations external-content FTS5 table. rebuild_fts_file_operations function follows existing fts.rs pattern. |
| ART-01 | files table -- one row per unique file path per session | Migration 003 creates files table with UNIQUE(session_id, file_path). upsert_file function in artifacts.rs. |
| ART-02 | file_operations table -- every Write/Edit/Read/Bash file touch | Migration 003 creates file_operations table. extract_*_operation functions in artifacts.rs parse tool_use input JSON. |
| ART-03 | git_operations table -- extracted from Bash git commands | Migration 003 creates git_operations table. extract_bash_operations in artifacts.rs uses regex to parse git commands. |
| ART-04 | Tool result matching -- link tool_use to tool_result by tool_use_id | UPDATE tool_executions.result_content during user record decomposition in decompose.rs. sourceToolAssistantUUID and tool_use_id verified as matching keys in real data. |
| ART-05 | Parse Write tool_use for file_path + content | extract_write_operation parses input JSON with keys "file_path" and "content" (verified from real JSONL data). |
| ART-06 | Parse Edit tool_use for file_path + old_string + new_string | extract_edit_operation parses input JSON with keys "file_path", "old_string", "new_string", "replace_all" (verified from real JSONL data). |
| ART-07 | Parse Read tool_use for file_path | extract_read_operation parses input JSON with key "file_path". No content stored (read is non-mutating). |
| ART-08 | Parse Bash tool_use for git commands | extract_bash_operations uses regex matching on "command" field. Handles HEREDOC commit messages, chained && commands, branch extraction. |
| ART-09 | Parse Bash tool_use for file-touching commands (cp, mv, rm, mkdir, touch) | extract_bash_operations also checks for file-touching shell commands and inserts file_operations with operation_type "bash_cp", "bash_rm", etc. |
| ART-10 | File content reconstruction -- replay writes + edits in order | reconstruct_file_content queries file_operations ordered by timestamp, replays write (set) and edit (replace) operations. Handles optional --at cutoff via message UUID timestamp lookup. |
| ART-11 | Diff generation -- unified diff of all edits | generate_file_diff uses `similar` TextDiff::from_lines().unified_diff() on consecutive file states. Computed on-demand, not pre-stored. |
| API-17 | GET /v1/files -- list files with filters | api/files.rs handler queries files table with optional session_id, path substring, date range, limit filters. |
| API-18 | GET /v1/files/:file_id -- file entry with all operations | api/files.rs handler joins files + file_operations for a specific file ID. |
| API-19 | GET /v1/files/:file_id/content -- reconstructed file at latest state | api/files.rs handler calls reconstruct_file_content. Optional ?at= query param for point-in-time reconstruction. |
| API-20 | GET /v1/files/:file_id/diff -- unified diff of all edits | api/files.rs handler calls generate_file_diff. |
| API-21 | GET /v1/files/search?q= -- FTS across file contents | api/files.rs handler uses search_file_operations (new fts.rs function) with FTS5 phrase matching. |
| API-22 | POST /v1/files/query -- flexible file query with glob support | api/files.rs handler accepts JSON body with glob pattern, session_id, operation_type filters. Uses glob::Pattern for path matching. |
| API-23 | GET /v1/git -- git operations with filters | api/git.rs handler queries git_operations with optional session_id, operation_type, date range filters. |
| API-24 | GET /v1/git/commits -- commit operations across all sessions | api/git.rs handler queries git_operations WHERE operation_type = 'commit'. |
| API-25 | GET /v1/git/commits/:session_id -- commits in specific session | api/git.rs handler queries git_operations WHERE session_id = :id AND operation_type = 'commit'. |
| API-26 | GET /v1/artifacts/:session_id -- combined files + git + tool outputs | api/artifacts.rs handler joins files, git_operations, tool_executions for a session. |
| API-27 | GET /v1/artifacts/:session_id/timeline -- chronological artifact events | api/artifacts.rs handler union-queries file_operations + git_operations ordered by timestamp. |
| CLI-10 | claude-history files -- list files touched | New Files subcommand in main.rs. Connects via daemon or direct DB. Calls list_files query function. |
| CLI-11 | claude-history file-history -- chronological operations on a file | New FileHistory subcommand. Calls query_file_operations for a specific path. |
| CLI-12 | claude-history reconstruct -- reconstruct file content at a point | New Reconstruct subcommand with --at flag. Calls reconstruct_file_content. |
| CLI-13 | claude-history git-log -- show git operations | New GitLog subcommand. Calls list_git_operations query function. |
| CLI-14 | claude-history artifacts -- combined view for a session | New Artifacts subcommand. Calls combined artifact query function. |
| SSE-06 | file:written and file:edited events | FileWritten and FileEdited variants added to SseEvent enum. Emitted during artifact decomposition in watcher loop. |
| SSE-07 | git:commit event when git commit extracted | GitCommit variant added to SseEvent enum. Emitted when extract_bash_operations finds a git commit. |
</phase_requirements>

## Sources

### Primary (HIGH confidence)
- Codebase analysis: All 17 existing source files in crates/server/src/, crates/store/src/, crates/core/src/ read and analyzed for integration points
- Real JSONL data analysis: Tool_use input shapes (Write, Edit, Read, Bash) verified from actual Claude Code session files
- Tool_use / tool_result matching verified: sourceToolAssistantUUID == assistant UUID, tool_use_id == tool_result.tool_use_id (confirmed in real data)
- [similar crate docs](https://docs.rs/similar) -- TextDiff::from_lines(), unified_diff() API, version 2.7.0
- [similar crate unified diff](https://docs.rs/similar/latest/similar/udiff/index.html) -- UnifiedDiff struct, context_radius, header, Display/to_writer
- [glob crate](https://crates.io/crates/glob) -- version 0.3.3, Pattern::matches_with()
- Existing migration pattern: 001_initial.sql (13 tables), 002_fts5.sql (FTS5 external-content). Migration 003 follows same include_str! pattern in schema.rs.
- Existing decomposition pattern: INSERT OR IGNORE for idempotency, per-type decompose functions, overflow logging. Artifact decomposition extends this.
- [similar GitHub](https://github.com/mitsuhiko/similar) -- De facto Rust diff library by mitsuhiko (also author of insta)

### Secondary (MEDIUM confidence)
- Git commit HEREDOC pattern: verified from 8+ real git commit commands in actual JSONL data. All use `$(cat <<'EOF'\n...\nEOF\n)` format.
- Chained Bash command pattern: verified from real data (git add && git commit is the dominant pattern).
- File-touching Bash commands: verified from real data (rm -rf, mkdir patterns observed).

### Tertiary (LOW confidence)
- Database size impact of content storage: no empirical measurement yet. Depends on user's coding patterns. Monitoring via health endpoint is the mitigation.
- `regex` crate for git parsing: HIGH confidence on the crate itself, MEDIUM on the specific regex patterns for all possible git command variations. Edge cases will likely surface during testing.

## Metadata

**Confidence breakdown:**
- Standard stack: HIGH -- similar, glob, regex are mature, well-documented crates with clear APIs
- Architecture: HIGH -- artifact decomposition pattern follows existing decompose.rs conventions verified in codebase. All integration points identified.
- Data shapes: HIGH -- tool_use input formats verified from real JSONL data (Write, Edit, Read, Bash keys confirmed)
- Tool result matching: HIGH -- sourceToolAssistantUUID linkage verified in real data
- Git parsing: MEDIUM -- HEREDOC and chained command patterns verified, but edge cases in the broader git command space may exist
- Pitfalls: HIGH -- grounded in codebase analysis and real data patterns

**Research date:** 2026-02-20
**Valid until:** 2026-03-20 (stable domain -- SQL schema, Rust crate APIs, Claude Code tool formats unlikely to change rapidly)
