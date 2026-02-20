# Feature Research

**Domain:** CLI session history ingestion, developer analytics daemon, AI coding assistant observability
**Researched:** 2026-02-20
**Confidence:** HIGH

## Feature Landscape

### Table Stakes (Users Expect These)

Features users assume exist. Missing these = product feels incomplete.

| Feature | Why Expected | Complexity | Notes |
|---------|--------------|------------|-------|
| JSONL parsing with graceful error handling | Every tool in this space parses JSONL; claude-session-viewer, ccusage, claude-code-chat-explorer, claude-history-explorer all do it. Failing on malformed lines is unacceptable. | LOW | Rust serde handles this well. Streaming line-by-line with per-line error recovery is the established pattern. Every competing tool does this. |
| Session listing with metadata | Gemini CLI, claude-session-viewer, claude-code-chat-explorer, Claude Code History MCP all list sessions with timestamps, message counts, project paths. Users expect to enumerate their history. | LOW | Basic SELECT with ordering. Must include: session ID, project path, timestamp range, message count, model(s) used. |
| Full-text search across message content | FTS is standard. claude-code-analytics (Kapadia) uses FTS5, claude-code-chat-explorer uses FTS5, Claudex indexes for full-text search. Atuin's search is its primary feature. Users will not tolerate grep-only workflows. | MEDIUM | SQLite FTS5 is the right choice. Needs tokenizer tuning for code content (trigram or unicode61). Must search across assistant text, user prompts, tool inputs, and tool outputs. |
| Token usage tracking and cost analytics | ccusage (the most popular tool in this space) is entirely built around this. claude-code-analytics, Claude Code Usage Monitor, claude-dev-insights all track tokens. Anthropic's own analytics dashboard shows token metrics. This is the single most common feature across all competing tools. | LOW | Sum input_tokens, output_tokens, cache_read, cache_creation per session/day/model. Cost calculation requires maintaining a pricing table (which changes). |
| Session export (JSON, Markdown) | claude-session-viewer exports to Markdown. claude-history-explorer exports JSON, Markdown, plain text. claude-code-chat-explorer exports JSON. Users want to share, archive, or pipe session data. | LOW | Markdown export needs readable formatting of tool calls. JSON export should preserve full fidelity. |
| CLI interface for common queries | Every tool in this space has a CLI. ccusage, claude-history-explorer, cchistory, claude-code-analytics all operate from the terminal. Developers live in terminals. | MEDIUM | clap-based. Must cover: sessions, search, stats, export, sync. Each subcommand should produce structured JSON (pipeable) or human-readable table output. |
| Incremental sync (don't re-process old data) | claude-code-chat-explorer watches for incremental changes. The OpenClaw JSONL pipeline tracks file offsets. With session files growing to 2GB (per Claude Session Restore docs), re-processing everything on each run is not viable. | MEDIUM | Byte-offset tracking per file in sync_metadata table. Append-only nature of JSONL makes this safe. Must handle file truncation/rotation edge cases. |
| Bulk import of existing history | claude-code-import exists specifically for this. Users have months of history in ~/.claude/projects/. First-run experience must import everything. | LOW | walkdir over ~/.claude/projects/**/*.jsonl, batch insert in transactions. Progress reporting matters for large imports. |
| Tool usage tracking and statistics | claude-code-analytics tracks tool usage with error rates. claude-dev-insights logs tool invocations. claude-session-viewer shows tool usage analytics. Users want to know which tools Claude uses most and which fail. | LOW | Decompose tool_use content blocks. Track name, frequency, error rate, average result size. |
| Project-scoped queries | Gemini CLI sessions are project-specific. claude-code-chat-explorer groups by project directory. Claude Code's own JSONL files are organized by project. Users think in projects, not global flat lists. | LOW | Project path is derivable from JSONL file path and from the cwd field in records. Index and filter on it. |

### Differentiators (Competitive Advantage)

Features that set the product apart. Not required, but valuable.

| Feature | Value Proposition | Complexity | Notes |
|---------|-------------------|------------|-------|
| Normalized SQLite decomposition (not raw JSONL storage) | No competing tool fully normalizes JSONL into relational tables. claude-code-analytics stores both raw JSONL and formatted text. claude-code-chat-explorer indexes into SQLite but as document blobs. A proper normalized schema (sessions, messages, content_blocks, token_usage, tool_executions) enables SQL queries that raw JSONL storage cannot support: joins across sessions, aggregation by model, tool correlation analysis. | HIGH | 11+ tables with foreign keys. The decomposer is the core intellectual property. Must handle the tool_use/tool_result cross-message linking (tool_use in assistant message, tool_result in subsequent user message, matched by tool_use_id). |
| File operation tracking and artifact layer | No competing tool extracts structured file operations from tool_use blocks. Nobody tracks which files Claude wrote, edited, or read as first-class queryable entities. This enables "what did Claude change in my codebase?" queries that are impossible with message-level tools. | HIGH | Requires parsing tool_use inputs for Write/Edit/Read/Bash/NotebookEdit. Upsert into files + file_operations tables. Bash command parsing for file-touching operations is heuristic and imperfect. |
| File content reconstruction (session-derived version control) | No competing tool can reconstruct a file's state at any point in a session by replaying Write and Edit operations. This is unique. It answers "what did this file look like after message #47?" without requiring git or filesystem snapshots. | HIGH | Replay writes + edits in timestamp order. String replacement for edits is fragile if edits overlap or conflict (rare in practice since Claude applies edits sequentially). Edge cases: multiple writes to same file, edits to content that was already edited. |
| Git operation extraction | No competing tool extracts structured git operations (commits, pushes, branches) from Bash tool calls. This enables "show me all commits Claude made this week" or "which sessions pushed to main?" | MEDIUM | Regex-based extraction from Bash command strings. Parse -m flags, heredoc commit messages, branch names. Heuristic but high-value. git commit is well-structured enough for reliable extraction. |
| Schema drift detection via serde overflow | No competing tool actively monitors for Claude Code schema changes. When Anthropic adds a field to the JSONL format, every other tool silently ignores it or breaks. claude-history captures unknown fields in overflow maps and logs them as drift events. This is a genuine resilience advantage. | MEDIUM | serde(flatten) on every struct. Log overflow keys to schema_drift_log with version, field name, sample value. Report drift via API and CLI. Enables proactive schema adaptation rather than reactive breakage. |
| Real-time file watching with SSE events | claude-code-chat-explorer has real-time WebSocket updates. No other tool combines file watching + SSE event streaming for a pub/sub pattern. This enables live dashboards, automation triggers, and hook-like behavior without Claude Code hook configuration. | MEDIUM | notify crate watches ~/.claude/projects/. Debounce file change events. Sync new bytes. Broadcast SSE events (record:added, file:written, git:commit, schema:drift). |
| HTTP API + Unix domain socket (language-agnostic access) | No competing tool exposes a proper REST API. All existing tools are monolithic: Python scripts, Node apps, or CLI-only. A stable HTTP API means any language, any tool, any automation can consume the data. The Unix socket variant provides lower-latency local access. | MEDIUM | axum serving JSON at /v1/. OpenAPI spec as contract. UDS is same routes, different listener. This is the architectural moat: claude-history becomes infrastructure, not just a tool. |
| Flexible query endpoints (POST body, not just URL params) | No competing tool has parameterized query compilation. POST /v1/messages/query and POST /v1/files/query accept structured query bodies that compile to parameterized SQL. This enables complex queries (multiple filters, content matching, date ranges, token thresholds) without URL parameter gymnastics. | MEDIUM | Query body struct -> WHERE clause builder. Must prevent SQL injection via parameterized queries. Glob pattern support for file paths. |
| Conversation tree reconstruction (sidechain awareness) | Claude Code uses sidechains (branching conversations) and parent_uuid linking. No competing tool renders conversation trees. Most flatten everything into a linear list. Tree reconstruction preserves the actual conversational structure. | MEDIUM | parent_uuid creates a tree. is_sidechain flags branches. GET /v1/sessions/:id/tree returns nested structure. Useful for understanding agent delegation and retry patterns. |
| MCP server integration | claude-code-history-mcp exists but only reads raw JSONL. A claude-history MCP server backed by the normalized SQLite database would provide richer, faster queries directly within Claude Code sessions. | LOW | Thin wrapper over the existing HTTP API. Could be a separate binary or mode (claude-history mcp-serve). Low complexity because it delegates to the already-built query engine. |
| Cross-session file provenance | "Show me every session that touched src/main.rs" across all sessions. No tool does this. Requires the file operations table indexed by file_path across sessions. | LOW | Simple query against files table: SELECT session_id, first_seen_at, operation_count FROM files WHERE file_path = ?. Low complexity because the artifact layer already enables it. |

### Anti-Features (Commonly Requested, Often Problematic)

Features that seem good but create problems.

| Feature | Why Requested | Why Problematic | Alternative |
|---------|---------------|-----------------|-------------|
| Web UI / dashboard frontend | Visual appeal, easier browsing. claude-code-chat-explorer, Claudex, claude-code-analytics (Kapadia) all have web UIs. | Massively increases scope. Frontend framework choice, build tooling, bundling, serving static assets. Every existing web UI in this space is mediocre (Streamlit dashboards, vanilla JS). The API is the product; UIs are consumer concerns. | Expose a clean HTTP API. Let consumers build their own UIs. Document curl examples. The claude-code-chat-explorer pattern (separate frontend, separate backend) is the right separation. |
| Cloud sync / remote database | Team collaboration, cross-machine access. Atuin offers encrypted cloud sync as a major feature. | Encryption, authentication, server infrastructure, privacy concerns, GDPR, API key management. Claude Code sessions contain proprietary code, credentials, API keys. Syncing this data creates enormous liability. | Stay local-only. If users want remote access, they can replicate the SQLite file or expose the API behind their own tunnel/VPN. Atuin's sync is appropriate for shell commands; session history with full code context is a different risk profile. |
| AI-powered analysis / summary generation | claude-code-analytics (Kapadia) offers AI analysis via OpenRouter. Sounds impressive. | Requires API keys, costs money per query, introduces external dependency, results are non-deterministic, privacy leak (sending session data to external LLMs). Violates the "zero runtime dependencies" principle. | Provide the data; let users pipe it to their own LLMs. Export to JSON/Markdown and feed to whatever model they want. The tool is a data layer, not an analysis layer. |
| Real-time token cost predictions / burn rate | Claude Code Usage Monitor does burn rate. Looks cool. | Pricing changes frequently. Requires maintaining a pricing table that must be updated manually. Predictions are often wrong because session patterns vary wildly. Creates false confidence in cost estimates. | Track raw token counts accurately. Provide optional cost calculation with user-configurable pricing. Never predict burn rate; just report actuals. |
| Write-back to JSONL files | "Fix" or "annotate" session history. | JSONL files are Claude Code's data. Writing to them risks corruption, conflicts with Claude Code's own writes, and breaks the append-only invariant that makes incremental sync safe. | Read-only ingestion only. If users want annotations, add a separate annotations table in SQLite that references message UUIDs. Never touch the source files. |
| Multi-user / authentication | Team analytics, shared dashboards. | Authentication adds complexity without value for a localhost tool. Session data is per-user. Multi-user requires authorization (who can see whose sessions?), which requires user management, which requires a database, which requires... scope explosion. | localhost-only, single-user. If a team wants shared analytics, they aggregate at a higher layer. Anthropic's own analytics dashboard handles team-level metrics. |
| Plugin / extension system | Customizability. | Plugin APIs are hard to design well, create maintenance burden, introduce security surface area, and are premature before the core is stable. Every plugin system in developer tools starts ambitious and ends neglected. | Stable HTTP API is the extension mechanism. Any language can consume it. Webhooks or SSE for event-driven extensions. No plugin runtime needed. |
| Windows support (initial release) | Broader user base. | Claude Code itself has limited Windows support. JSONL paths, Unix domain sockets, daemon management, file watching all differ on Windows. Supporting it doubles testing surface for a small user segment. | macOS and Linux first. Windows support as a future phase if demand materializes. Document this explicitly. |

## Feature Dependencies

```
[JSONL Parser]
    |
    +--requires--> [Serde Type Modeling]
    |
    +--enables--> [Incremental Sync Engine]
    |                  |
    |                  +--enables--> [Bulk Import]
    |                  |
    |                  +--enables--> [File Watcher (real-time)]
    |                                    |
    |                                    +--enables--> [SSE Event Stream]
    |
    +--enables--> [Record Decomposer]
                       |
                       +--requires--> [SQLite Schema + Migrations]
                       |
                       +--enables--> [Message Query Engine]
                       |                  |
                       |                  +--enables--> [HTTP API Routes]
                       |                  |                  |
                       |                  |                  +--enables--> [Unix Domain Socket]
                       |                  |                  |
                       |                  |                  +--enables--> [MCP Server Mode]
                       |                  |
                       |                  +--enables--> [CLI Query Commands]
                       |                  |
                       |                  +--enables--> [Flexible POST Query Endpoints]
                       |
                       +--enables--> [FTS5 Full-Text Search]
                       |
                       +--enables--> [Token Analytics]
                       |
                       +--enables--> [Tool Usage Statistics]
                       |
                       +--enables--> [Session Export]
                       |
                       +--enables--> [Artifact Decomposer]
                                         |
                                         +--requires--> [Tool Result Matching (cross-message)]
                                         |
                                         +--enables--> [File Operation Tracking]
                                         |                  |
                                         |                  +--enables--> [File Content Reconstruction]
                                         |                  |
                                         |                  +--enables--> [Cross-Session File Provenance]
                                         |                  |
                                         |                  +--enables--> [File Content FTS]
                                         |
                                         +--enables--> [Git Operation Extraction]
                                         |
                                         +--enables--> [Artifact Timeline]
                       |
                       +--enables--> [Schema Drift Detection]
                                         |
                                         +--requires--> [serde(flatten) overflow on all structs]
                                         |
                                         +--enables--> [Version Monitor]

[Conversation Tree Reconstruction]
    +--requires--> [Record Decomposer] (parent_uuid, is_sidechain fields)
```

### Dependency Notes

- **Artifact Decomposer requires Tool Result Matching:** File operations need both the tool_use (from assistant message) and tool_result (from subsequent user message) to record success/failure. The decomposer must buffer the previous assistant message and match results when the next user message arrives. This is the hardest dependency in the system.
- **File Content Reconstruction requires File Operation Tracking:** You cannot replay edits if you have not extracted and stored them as structured operations. Reconstruction is a read-path feature that depends on the write-path artifact decomposer.
- **SSE Event Stream requires File Watcher:** Events are emitted when new data arrives. No watcher = no events. The watcher triggers sync, sync triggers decomposition, decomposition emits events.
- **Schema Drift Detection requires serde overflow on all structs:** This is a compile-time architectural decision, not a runtime dependency. If overflow fields are not present on every struct, unknown fields are silently dropped and drift goes undetected. This must be enforced from the very first type definition.
- **HTTP API enables MCP Server Mode:** An MCP server for claude-history is a thin translation layer over the existing HTTP API. It should not be built before the API is stable.
- **Flexible POST Query Endpoints enhance HTTP API:** These are enrichments on top of basic GET routes. Build basic GET routes first, add POST query bodies as a refinement.

## MVP Definition

### Launch With (v1)

Minimum viable product -- what's needed to validate the concept.

- [ ] Serde type modeling of all JSONL record types with overflow capture -- without this, nothing else works
- [ ] Streaming JSONL parser with byte-offset tracking -- the ingestion primitive
- [ ] SQLite schema with migrations (core 11 tables) -- the storage layer
- [ ] Record decomposer (all record types to normalized rows) -- the transformation layer
- [ ] Incremental sync engine -- the efficiency layer (prevents re-processing)
- [ ] Bulk import (walk ~/.claude/projects/) -- the first-run experience
- [ ] FTS5 full-text search across message content -- the primary discovery mechanism
- [ ] CLI: sync, sessions, search, query, stats, export -- the user interface
- [ ] Token usage analytics (per session, per day, per model) -- the most-requested feature in the ecosystem
- [ ] Tool usage statistics -- high value, low complexity

### Add After Validation (v1.x)

Features to add once core is working.

- [ ] Artifact decomposer (file ops, git ops, tool result matching) -- adds after core ingestion is proven stable
- [ ] File content reconstruction -- adds after artifact layer is proven correct
- [ ] HTTP API (axum) at /v1/ -- adds when CLI is stable and API contract is designed
- [ ] File watcher for real-time ingestion -- adds when sync engine is battle-tested
- [ ] SSE event stream -- adds after file watcher and HTTP API exist
- [ ] Schema drift detection and version monitoring -- adds when overflow capture is proven to work
- [ ] Unix domain socket -- adds as a listener variant once HTTP API is stable
- [ ] Conversation tree reconstruction -- adds when parent_uuid/sidechain data is reliably decomposed

### Future Consideration (v2+)

Features to defer until product-market fit is established.

- [ ] MCP server mode -- defer until API contract is stable and MCP ecosystem conventions settle
- [ ] Cross-session file provenance queries -- defer until artifact layer has usage data showing demand
- [ ] Flexible POST query endpoints -- defer until GET routes reveal their limitations through real usage
- [ ] Git operation extraction -- defer; heuristic parsing of Bash commands is fragile and needs real-world tuning
- [ ] OpenAPI spec generation -- defer until API routes are stable

## Feature Prioritization Matrix

| Feature | User Value | Implementation Cost | Priority |
|---------|------------|---------------------|----------|
| JSONL parsing + serde types | HIGH | MEDIUM | P1 |
| SQLite schema + decomposer | HIGH | HIGH | P1 |
| Incremental sync | HIGH | MEDIUM | P1 |
| Bulk import | HIGH | LOW | P1 |
| FTS5 search | HIGH | MEDIUM | P1 |
| CLI interface (core commands) | HIGH | MEDIUM | P1 |
| Token analytics | HIGH | LOW | P1 |
| Tool usage stats | MEDIUM | LOW | P1 |
| Session export (JSON/Markdown) | MEDIUM | LOW | P1 |
| Project-scoped queries | MEDIUM | LOW | P1 |
| Artifact decomposer (files + git) | HIGH | HIGH | P2 |
| File content reconstruction | HIGH | HIGH | P2 |
| HTTP API (axum /v1/) | HIGH | MEDIUM | P2 |
| File watcher (real-time) | MEDIUM | MEDIUM | P2 |
| SSE event stream | MEDIUM | MEDIUM | P2 |
| Schema drift detection | MEDIUM | MEDIUM | P2 |
| Unix domain socket | LOW | LOW | P2 |
| Conversation tree | MEDIUM | MEDIUM | P2 |
| MCP server mode | MEDIUM | LOW | P3 |
| Cross-session file provenance | MEDIUM | LOW | P3 |
| POST query endpoints | MEDIUM | MEDIUM | P3 |
| OpenAPI spec | LOW | LOW | P3 |

**Priority key:**
- P1: Must have for launch
- P2: Should have, add when possible
- P3: Nice to have, future consideration

## Competitor Feature Analysis

| Feature | ccusage | claude-code-analytics (spences10) | claude-code-analytics (Kapadia) | claude-code-chat-explorer | claude-history-explorer | claude-session-viewer | AI Observer | claude-history (this project) |
|---------|---------|-----------------------------------|-------------------------------|---------------------------|------------------------|----------------------|-------------|-------------------------------|
| JSONL parsing | Yes | Yes (hooks) | Yes (hooks) | Yes | Yes | Yes | No (OTLP) | Yes |
| SQLite storage | No | Yes | Yes (FTS5) | Yes (FTS5) | No | No | No (DuckDB) | Yes (normalized, FTS5) |
| Token analytics | Yes (primary) | Yes (87+ metrics) | Yes | Yes | Basic | Basic | Yes | Yes |
| Full-text search | No | No | Yes (FTS5) | Yes (FTS5) | Regex only | No | Yes | Yes (FTS5) |
| File operation tracking | No | No | No | No | No | No | No | **Yes (unique)** |
| Git operation extraction | No | No | No | No | No | No | No | **Yes (unique)** |
| File content reconstruction | No | No | No | No | No | No | No | **Yes (unique)** |
| Schema drift detection | No | No | No | No | No | No | No | **Yes (unique)** |
| HTTP API | No | No | Streamlit | Express | No | No | REST+WS | **Yes (axum)** |
| Real-time watching | No | No (hooks) | No (hooks) | WebSocket | No | No | WebSocket | **Yes (SSE)** |
| CLI interface | Yes | Yes | Yes | Docker only | Yes | Yes (TUI) | Docker only | **Yes** |
| Session export | JSON | No | No | JSON | JSON/MD/TXT | Markdown | Parquet | **JSON/MD/CSV** |
| Language | TypeScript | TypeScript | Python | JavaScript | Python | Python | Go | **Rust** |
| Architecture | Script | Hook-driven | Hook-driven | Web app | Script | Script/TUI | Web app | **Daemon + CLI + API** |
| Multi-tool support | Codex, OpenCode | No | No | No | No | No | Claude, Gemini, Codex | No (Claude Code only) |

### Key Competitive Observations

1. **Token analytics is solved.** ccusage does it well. Competing purely on token tracking is a race to the bottom. claude-history's token analytics should be table stakes, not the pitch.

2. **Nobody does artifact tracking.** File operations, git extraction, and content reconstruction are completely unoccupied territory. This is the primary differentiation axis.

3. **Nobody exposes a proper API.** Everything is either a CLI script, a web app, or a hook-driven capture tool. A stable HTTP API that any language can consume is genuine infrastructure differentiation.

4. **Schema resilience is unaddressed.** Every tool will break when Claude Code's JSONL schema changes. The overflow/drift detection pattern is a durability advantage that compounds over time.

5. **The "daemon" architecture is unique.** No competing tool runs as a persistent background process that watches, syncs, serves, and streams. Everything else is either run-once or hook-triggered.

## Sources

- [awesome-claude-code](https://github.com/hesreallyhim/awesome-claude-code) -- curated list of Claude Code tools, primary ecosystem survey source [HIGH confidence]
- [ccusage](https://github.com/ryoppippi/ccusage) -- most popular token tracking tool for Claude Code JSONL [HIGH confidence]
- [claude-code-analytics (spences10)](https://github.com/spences10/claude-code-analytics) -- hook-driven SQLite analytics with 87+ metrics [HIGH confidence]
- [claude-code-analytics (Kapadia)](https://github.com/sujankapadia/claude-code-analytics) -- FTS5 search, Streamlit dashboard, AI analysis [HIGH confidence]
- [claude-code-chat-explorer](https://github.com/drewburchfield/claude-code-chat-explorer) -- Express/SQLite web app with FTS5 and WebSocket [HIGH confidence]
- [claude-history-explorer](https://github.com/adewale/claude-history-explorer) -- Python CLI with regex search, analytics, export [HIGH confidence]
- [claude-session-viewer](https://github.com/jtklinger/claude-session-viewer) -- TUI + Markdown export for session browsing [HIGH confidence]
- [claude-dev-insights (Kanopi)](https://github.com/kanopi/claude-dev-insights) -- Shell/Python hook-driven 29-field session analytics [HIGH confidence]
- [AI Observer](https://github.com/tobilg/ai-observer) -- Go/DuckDB multi-assistant observability with OTLP [HIGH confidence]
- [claude-code-history-mcp](https://mcpservers.org/servers/yudppp/claude-code-history-mcp) -- MCP server for raw JSONL history access [HIGH confidence]
- [Atuin](https://github.com/atuinsh/atuin) -- Rust/SQLite shell history tool; architectural precedent for daemon + search + sync [HIGH confidence]
- [Gemini CLI Session Management](https://geminicli.com/docs/cli/session-management/) -- search, resume, retention policies; establishes user expectations for session tools [HIGH confidence]
- [Claude Code analytics docs](https://code.claude.com/docs/en/analytics) -- Anthropic's official analytics: usage metrics, contribution metrics, CSV export [HIGH confidence]
- [kentgigger: Claude Code hidden history](https://kentgigger.com/posts/claude-code-conversation-history) -- community documentation of JSONL structure [MEDIUM confidence]
- [OpenClaw JSONL pipeline](https://github.com/openclaw/openclaw/issues/7783) -- JSONL-to-SQLite ingestion pattern with offset tracking [MEDIUM confidence]

---
*Feature research for: CLI session history ingestion, developer analytics daemon, AI coding assistant observability*
*Researched: 2026-02-20*
