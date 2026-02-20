# Project Research Summary

**Project:** claude-history
**Domain:** Rust CLI daemon -- JSONL ingestion, normalized SQLite store, HTTP/UDS/CLI API for Claude Code session history
**Researched:** 2026-02-20
**Confidence:** HIGH

## Executive Summary

claude-history is a single-binary Rust daemon that ingests Claude Code's undocumented JSONL session history, decomposes it into a normalized SQLite database, and exposes a stable query API over HTTP, Unix domain socket, and CLI. The competitive landscape (ccusage, claude-code-analytics, claude-code-chat-explorer, and roughly a dozen others) is crowded at the surface level -- everyone parses JSONL and counts tokens -- but no tool normalizes the data into relational tables, no tool tracks file operations or git operations as first-class entities, no tool exposes a proper language-agnostic HTTP API, and no tool runs as a persistent daemon. These four gaps define the differentiation axis.

The recommended approach builds bottom-up through a Cargo workspace of three crates: `core` (pure serde types and JSONL parser, no async runtime), `store` (SQLite schema, record decomposer, incremental sync, FTS5, file watcher), and `server` (axum HTTP, clap CLI, SSE events, daemon lifecycle). The stack is mature and well-documented: axum 0.8, tokio 1.49, rusqlite 0.38 (bundled), serde, clap 4, notify 8. Every major dependency is the ecosystem standard for its role. The architecture follows a split writer/reader SQLite pattern (single writer connection + pooled read-only connections in WAL mode), which is well-benchmarked at roughly 20x the write throughput of naive connection pooling.

The primary risks are (1) blocking the tokio runtime with synchronous rusqlite calls -- mitigated by establishing the tokio-rusqlite bridge pattern from the first database access, (2) byte-offset tracking correctness for incremental sync -- mitigated by line-by-line parsing with offset advancement only after complete line confirmation, and (3) the complexity of the artifact decomposer -- tool_use/tool_result cross-message matching is the hardest logic in the system and should be deferred to a later phase after core ingestion is proven stable.

## Key Findings

### Recommended Stack

The stack is entirely Rust ecosystem standard: tokio for async, axum for HTTP, rusqlite (bundled) for SQLite, serde for serialization, clap for CLI, notify for file watching, tracing for structured logging. All dependencies are actively maintained, version-compatible with each other, and well-documented. The `bundled` feature for rusqlite is mandatory -- it compiles SQLite 3.51.1 from source with FTS5 unconditionally enabled, producing a zero-runtime-dependency binary. No alternative stack choices warrant serious consideration; every recommendation has HIGH confidence and reflects overwhelming ecosystem consensus.

**Core technologies:**
- **Rust (edition 2024, 1.85+):** Single static binary, zero runtime deps, serde enum system maps directly to JSONL discriminated unions
- **tokio 1.49:** De facto async runtime; required by axum, notify bridge, and all async I/O
- **axum 0.8:** Tokio team's web framework with first-class SSE and UDS support; shares tower middleware
- **rusqlite 0.38 (bundled):** Synchronous SQLite with compiled-in FTS5; paired with tokio-rusqlite for async bridge
- **serde + serde_json:** Internally tagged enums (`#[serde(tag = "type")]`) + `#[serde(flatten)]` overflow capture for schema resilience
- **clap 4 (derive):** CLI parsing with subcommand generation from struct definitions
- **notify 8:** Cross-platform file watching (FSEvents/inotify); paired with notify-debouncer-full

**Critical version constraints:** axum 0.8 requires hyper 1.x and tower-http 0.6.x. notify 8 requires notify-debouncer-full 0.7.x. tracing 0.1 pairs with tracing-subscriber 0.3.

### Expected Features

The feature landscape separates cleanly into table stakes that every competing tool already provides, differentiators that no competitor offers, and anti-features that should be explicitly excluded.

**Must have (table stakes):**
- JSONL parsing with graceful per-line error handling
- Session listing with metadata (ID, project, timestamps, message count, model)
- Full-text search via FTS5 across message content
- Token usage tracking and cost analytics (the single most common feature across all competitors)
- Incremental sync with byte-offset tracking (files grow to 2GB; re-processing is not viable)
- Bulk import of existing history from `~/.claude/projects/`
- CLI interface for common queries (sessions, search, stats, export)
- Tool usage statistics and project-scoped queries

**Should have (differentiators -- unique to claude-history):**
- Normalized SQLite decomposition into 11+ relational tables (no competitor fully normalizes)
- File operation tracking as first-class queryable entities (completely unoccupied territory)
- Schema drift detection via serde overflow fields (no competitor monitors for schema changes)
- HTTP API at /v1/ (no competitor exposes a proper REST API)
- Real-time file watching with SSE event streaming (no competitor runs as a persistent daemon)
- File content reconstruction by replaying Write/Edit operations

**Defer (v2+):**
- MCP server mode (wait for API stability and MCP ecosystem conventions)
- Git operation extraction from Bash commands (heuristic parsing needs real-world tuning)
- Flexible POST query endpoints (let GET routes reveal their limitations first)
- OpenAPI spec generation (wait for route stability)

### Architecture Approach

The system is a three-crate Cargo workspace with strict dependency flow: `core` (no async, no DB) -> `store` (depends on core) -> `server` (depends on both). The core crate is a pure library of serde types and a synchronous JSONL parser. The store crate owns all SQLite access through a split writer/reader pool (single tokio-rusqlite writer connection + N read-only connections), runs the record decomposer and artifact extractor inside writer transactions, and bridges the notify file watcher from OS threads to tokio channels. The server crate is the only binary entry point, running dual TCP+UDS listeners on a shared axum router, dispatching CLI subcommands via clap, and broadcasting SSE events through a tokio broadcast channel. The CLI intelligently probes for a running daemon over UDS and falls back to direct read-only DB access when the daemon is not running.

**Major components:**
1. **core crate** -- Serde type modeling (discriminated unions with overflow capture), streaming JSONL parser with byte-offset tracking, version detection and drift event types
2. **store crate** -- SQLite schema with embedded migrations, record decomposer (all record types to normalized rows), artifact decomposer (file ops, git ops), incremental sync engine, FTS5 integration, file watcher (notify -> mpsc bridge), query builder
3. **server crate** -- axum HTTP API at /v1/, dual TCP+UDS listeners, clap CLI dispatch, SSE event broadcasting, daemon lifecycle (PID file, graceful shutdown via CancellationToken), CLI-to-daemon socket probing with read-only fallback

### Critical Pitfalls

1. **Blocking tokio with rusqlite (CRITICAL, Phase 1)** -- rusqlite is synchronous. Calling it directly inside async handlers starves the tokio runtime. Use tokio-rusqlite which spawns a dedicated OS thread per connection. This pattern must be established from the first database access; retrofitting changes every callsite.

2. **SQLite write contention from multiple connections (CRITICAL, Phase 1)** -- Multiple writer connections cause SQLITE_BUSY and roughly 20x performance degradation. Use a single dedicated writer connection + separate read-only pool in WAL mode. This is a foundational design decision that cannot be changed later without restructuring every database access path.

3. **serde_json StreamDeserializer halts on errors (CRITICAL, Phase 1)** -- StreamDeserializer stops permanently after one malformed line. JSONL files are not a "JSON stream" -- they are independent records per line. Use line-by-line parsing with per-line error recovery. This is non-negotiable; StreamDeserializer is structurally wrong for JSONL.

4. **Byte-offset tracking correctness (CRITICAL, Phase 1)** -- Partial final lines, file truncation, and encoding mismatches can corrupt resume state. Only advance stored offset after successfully parsing a complete line. Reset to 0 if stored offset exceeds file size. Hold back the last fragment until a trailing newline confirms completeness.

5. **notify platform behavior differences (MODERATE, Phase 2)** -- FSEvents (macOS) and inotify (Linux) have different event semantics. Do not rely on specific event types for correctness; treat any event as "file may have changed, re-check." Add a periodic poll fallback (30s) to catch missed events.

## Implications for Roadmap

### Phase 1: Core Foundation (Types, Parser, Schema, Decomposer)
**Rationale:** The entire system depends on correct serde modeling and a working ingestion pipeline. Architecture research explicitly identifies `core` as having zero dependencies on async runtime or database -- it can be built and tested first with JSONL fixture files. The store crate's schema and decomposer come next because they establish the data model that everything else queries.
**Delivers:** Cargo workspace structure, serde type modeling of all JSONL record types with overflow capture, streaming line-by-line JSONL parser with byte-offset tracking, SQLite schema with 11+ normalized tables and embedded migrations, record decomposer, incremental sync engine, bulk import
**Addresses:** JSONL parsing, session listing, incremental sync, bulk import, token analytics, tool usage stats, project-scoped queries
**Avoids:** Pitfalls 1-5 (tokio-rusqlite bridge, single writer, line-by-line parsing, byte-offset correctness, serde flatten decisions)

### Phase 2: Search and CLI
**Rationale:** FTS5 search is a table-stakes feature that users will reach for immediately. The CLI is the primary user interface for v1. Both depend on the store crate's query builder and the data already being in SQLite from Phase 1. Session export is low-complexity and high-value to ship alongside search.
**Delivers:** FTS5 full-text search, CLI interface (sync, sessions, search, query, stats, export), session export (JSON, Markdown), query builder for parameterized SQL
**Addresses:** Full-text search, CLI interface, session export
**Avoids:** Pitfall 7 (FTS5 index degradation -- plan external content tables in schema from Phase 1, optimize after bulk loads)

### Phase 3: HTTP API and Daemon Mode
**Rationale:** The HTTP API is the architectural moat -- it turns claude-history from a tool into infrastructure. But it depends on the query engine and data model being stable from Phases 1-2. The daemon lifecycle (PID file, graceful shutdown, dual listeners) is complex enough to deserve its own phase.
**Delivers:** axum HTTP API at /v1/, dual TCP+UDS listeners, daemon mode (`claude-history serve`), graceful shutdown via CancellationToken, CLI-to-daemon socket probing with read-only fallback
**Addresses:** HTTP API, Unix domain socket, daemon mode
**Avoids:** Pitfalls 8-9 (stale UDS socket cleanup, graceful shutdown data loss)

### Phase 4: Real-Time Ingestion and Events
**Rationale:** File watching and SSE depend on both the sync engine (Phase 1) and the HTTP API (Phase 3). The watcher bridges OS threads to tokio channels, which requires the async infrastructure to be established. SSE broadcasting requires the axum routes to exist.
**Delivers:** notify-based file watcher with debouncing, SSE event stream (record:added, schema:drift, sync:complete), real-time incremental ingestion while daemon is running
**Addresses:** File watcher, SSE event stream, real-time ingestion
**Avoids:** Pitfall 6 (notify platform differences -- event-type-agnostic design, periodic poll fallback)

### Phase 5: Artifact Layer (File Ops, Git Ops, Content Reconstruction)
**Rationale:** The artifact decomposer is the highest-complexity, highest-differentiation feature. It requires tool_use/tool_result cross-message matching (the hardest dependency in the system) and should only be built after core ingestion is proven stable. File content reconstruction depends on the artifact layer being correct. These are unique features no competitor offers, but they are also the riskiest to get wrong.
**Delivers:** Artifact decomposer (file operations, git operations), tool result matching by tool_use_id, file content reconstruction via operation replay, cross-session file provenance queries
**Addresses:** File operation tracking, git operation extraction, file content reconstruction, schema drift detection
**Avoids:** Building complex secondary decomposition before primary ingestion is battle-tested

### Phase 6: Polish and Integration
**Rationale:** Schema drift detection, conversation tree reconstruction, launchd/systemd configs, and MCP server mode are refinements that add value only after the core product works end-to-end.
**Delivers:** Schema drift detection and version monitoring, conversation tree reconstruction, launchd plist + systemd unit files, potential MCP server mode
**Addresses:** Schema drift, conversation trees, deployment automation, MCP integration

### Phase Ordering Rationale

- **Bottom-up by crate dependency:** core -> store -> server mirrors the Cargo dependency graph. Each phase produces a testable artifact before the next begins.
- **Table stakes before differentiators:** Phases 1-2 deliver every feature users expect (parsing, search, CLI, analytics). Phases 3-5 deliver the features no competitor has (API, daemon, artifact tracking).
- **Complexity deferred but not avoided:** The artifact layer (Phase 5) is the primary differentiation but also the hardest code. Building it on a proven foundation reduces risk. The cross-message tool_use/tool_result matching logic is explicitly called out in features research as "the hardest dependency in the system."
- **Critical pitfalls front-loaded:** Five of the seven critical pitfalls must be addressed in Phase 1. This is by design -- the foundational patterns (tokio-rusqlite bridge, single writer, line-by-line parsing, byte-offset tracking) cannot be retrofitted cheaply.

### Research Flags

Phases likely needing deeper research during planning:
- **Phase 1 (decomposer):** The JSONL schema is undocumented. Real fixture files from `~/.claude/projects/` should drive type modeling. The record type enumeration and content block variants need empirical discovery against actual session data.
- **Phase 5 (artifact layer):** Tool_use/tool_result cross-message matching and file content reconstruction via edit replay are novel logic with limited precedent. The heuristic parsing of Bash commands for file-touching and git operations needs real-world tuning against diverse session histories.

Phases with standard patterns (skip additional research):
- **Phase 2 (FTS5 + CLI):** SQLite FTS5 and clap are thoroughly documented. Standard patterns apply.
- **Phase 3 (HTTP API + daemon):** axum routing, dual listeners, and graceful shutdown are well-demonstrated in official examples. The Docker-style CLI-to-daemon socket probing is a known pattern.
- **Phase 4 (file watcher + SSE):** notify-to-tokio bridging and axum SSE are documented patterns with official examples.

## Confidence Assessment

| Area | Confidence | Notes |
|------|------------|-------|
| Stack | HIGH | Every dependency verified against docs.rs/crates.io with exact versions. Version compatibility cross-checked. All recommendations are ecosystem standard with no controversial choices. |
| Features | HIGH | Competitive analysis covers 10+ tools with feature-by-feature comparison. Table stakes vs differentiators clearly separated. Anti-features well-justified. Feature dependency graph is detailed and internally consistent. |
| Architecture | HIGH | Split writer/reader pattern backed by published benchmarks (20x improvement). tokio-rusqlite, dual listener, and notify bridge are all documented patterns with official examples. Build order follows crate dependency graph. |
| Pitfalls | HIGH | All critical pitfalls verified against official documentation or published benchmarks. Recovery costs assessed. Phase mapping aligns with architecture build order. |

**Overall confidence:** HIGH

### Gaps to Address

- **JSONL schema coverage:** The Claude Code JSONL format is undocumented and evolves between releases. Type modeling in Phase 1 must be driven by empirical analysis of real session files, not assumptions. The serde overflow pattern mitigates breakage but does not eliminate the need for discovery.
- **tokio-rusqlite vs spawn_blocking:** The research recommends tokio-rusqlite for the async bridge but notes that closures must be `FnOnce + Send + 'static`. If ergonomic friction during implementation is high, a spawn_blocking wrapper with a shared connection handle is a viable fallback. This should be assessed early in Phase 1.
- **FTS5 tokenizer selection:** Research identifies the need for code-content search (source code, file paths, tool names) but does not resolve which FTS5 tokenizer is optimal. `unicode61` is the default; `trigram` enables substring matching but at higher index cost. This decision should be made during Phase 2 planning with test queries against real data.
- **Daemon management approach:** The research recommends foreground process with launchd/systemd rather than self-daemonizing, but the `claude-history serve` workflow (user runs CLI, process backgrounds itself) may need a lightweight wrapper or documentation-only approach. The exact UX for starting/stopping the daemon needs design during Phase 3.
- **Large session file handling:** Sessions can grow to 2GB. The byte-offset incremental sync handles this for ongoing ingestion, but bulk import of many large files needs performance validation. Batch transaction sizing (every N files vs every N records) should be benchmarked during Phase 1.

## Sources

### Primary (HIGH confidence)
- Official crate documentation (docs.rs) for all recommended dependencies -- versions verified 2026-02-20
- [Evan Schwartz -- SQLite connection pool write performance benchmarks](https://emschwartz.me/psa-your-sqlite-connection-pool-might-be-ruining-your-write-performance/) -- single-writer architecture data
- [SQLite WAL documentation](https://sqlite.org/wal.html) -- concurrent read/write semantics
- [SQLite FTS5 documentation](https://www.sqlite.org/fts5.html) -- automerge, optimization, detail levels
- [axum official examples](https://github.com/tokio-rs/axum/tree/main/examples) -- UDS, graceful shutdown, SSE
- [tokio graceful shutdown guide](https://tokio.rs/tokio/topics/shutdown) -- CancellationToken + TaskTracker
- [serde enum representations](https://serde.rs/enum-representations.html) -- internally tagged + flatten pattern
- [Apple launchd documentation](https://developer.apple.com/library/archive/documentation/MacOSX/Conceptual/BPSystemStartup/Chapters/CreatingLaunchdJobs.html) -- daemon lifecycle constraints

### Secondary (MEDIUM confidence)
- [awesome-claude-code](https://github.com/hesreallyhim/awesome-claude-code) -- ecosystem survey, tool inventory
- Competing tool repositories (ccusage, claude-code-analytics, claude-code-chat-explorer, etc.) -- feature analysis
- [serde_json StreamDeserializer error recovery discussion](https://users.rust-lang.org/t/step-past-errors-in-serde-json-streamdeserializer/84228) -- JSONL parsing pitfall
- [notify-rs GitHub issues](https://github.com/notify-rs/notify/issues/596) -- platform behavior differences, kqueue file descriptor limits
- [tokio-listener crate](https://lib.rs/crates/tokio-listener) -- alternative to manual dual-listener setup

### Tertiary (LOW confidence)
- [kentgigger: Claude Code hidden history](https://kentgigger.com/posts/claude-code-conversation-history) -- community JSONL schema documentation (useful but may not be current)
- [serde_fast_flatten](https://crates.io/crates/serde_fast_flatten) -- performance alternative for serde flatten (noted as optimization option, not validated in production)

---
*Research completed: 2026-02-20*
*Ready for roadmap: yes*
