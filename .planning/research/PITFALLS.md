# Pitfalls Research

**Domain:** Rust JSONL-to-SQLite ingestion daemon with FTS5 search and HTTP/UDS API
**Researched:** 2026-02-20
**Confidence:** HIGH (verified against official docs for all major claims)

## Critical Pitfalls

### Pitfall 1: Blocking the Tokio Runtime with rusqlite

**What goes wrong:**
rusqlite is synchronous. Calling `conn.execute()` or `conn.query_row()` directly inside an async task blocks the tokio worker thread. With the default multi-threaded runtime (typically 4-8 worker threads), even a few concurrent blocking SQLite calls can starve the entire runtime --- HTTP handlers stop responding, file-watch events queue up, shutdown signals get delayed.

**Why it happens:**
rusqlite's `Connection` is `!Sync` and all operations are blocking I/O or CPU-bound (especially FTS5 indexing). Developers coming from async-native database libraries (sqlx, deadpool) instinctively call rusqlite inline without wrapping it.

**How to avoid:**
Use [tokio-rusqlite](https://docs.rs/tokio-rusqlite) which spawns a dedicated background thread per connection and communicates via mpsc/oneshot channels. Every database call goes through `conn.call(|conn| { ... }).await`. This keeps blocking work off the async executor entirely. Alternatively, `tokio::task::spawn_blocking` works but creates a new thread per call from tokio's blocking pool, which is less efficient for frequent small operations.

**Warning signs:**
- HTTP latency spikes that correlate with ingestion batches
- `tokio-console` showing worker threads stuck in blocking state
- Graceful shutdown taking unexpectedly long (signal handler can not run because workers are blocked)

**Phase to address:**
Phase 1 (Core Foundation). The async/sync bridge pattern must be established in the very first database access code. Retrofitting is painful because it changes every callsite signature.

---

### Pitfall 2: SQLite Write Contention from Multiple Connections

**What goes wrong:**
Opening multiple SQLite connections and writing from more than one causes SQLITE_BUSY errors, long busy-timeout waits, and severely degraded write throughput. In [documented benchmarks](https://emschwartz.me/psa-your-sqlite-connection-pool-might-be-ruining-your-write-performance/), using a shared pool for writes was ~20x slower than a single-writer architecture (1.93s vs 83ms for 10,000 inserts).

**Why it happens:**
SQLite in WAL mode permits concurrent readers but exactly one writer at a time. If you use a connection pool (bb8-rusqlite, r2d2) for both reads and writes, multiple tasks contend for the exclusive write lock at the SQLite level. Worse, if a task `.await`s while holding a write transaction, it yields the async runtime thread while blocking the SQLite lock --- other writers spin on `busy_timeout` (default 5s) and eventually fail.

**How to avoid:**
Adopt a **split reader/writer architecture**:
- One dedicated writer connection (via tokio-rusqlite). All writes serialize through it at the application level, eliminating SQLite-level lock contention entirely.
- A separate pool of read-only connections for HTTP query handlers. These can run concurrently with the writer under WAL mode.

Set essential PRAGMAs on connection open:
```sql
PRAGMA journal_mode = WAL;          -- concurrent read/write
PRAGMA busy_timeout = 5000;         -- wait before SQLITE_BUSY
PRAGMA synchronous = NORMAL;        -- safe with WAL, better perf than FULL
PRAGMA foreign_keys = ON;           -- enforce referential integrity
PRAGMA cache_size = -32000;         -- 32MB cache (negative = KiB)
PRAGMA wal_autocheckpoint = 1000;   -- default is 1000, tune if needed
```

Sources: [SQLite WAL docs](https://sqlite.org/wal.html), [SQLite pragma cheatsheet](https://cj.rs/blog/sqlite-pragma-cheatsheet-for-performance-and-consistency/), [Write perf analysis](https://emschwartz.me/psa-your-sqlite-connection-pool-might-be-ruining-your-write-performance/)

**Warning signs:**
- Log entries showing `SQLITE_BUSY` errors or "database is locked"
- Write latency spikes that scale with concurrent HTTP requests
- sqlx/rusqlite busy-timeout warnings where elapsed time approaches your configured timeout

**Phase to address:**
Phase 1 (Core Foundation). The connection architecture (single writer + reader pool) is a foundational design decision. Changing it later requires restructuring every database access path.

---

### Pitfall 3: serde_json StreamDeserializer Error Recovery in JSONL

**What goes wrong:**
`serde_json::StreamDeserializer` halts permanently after encountering a deserialization error. Once it hits a malformed line, [it returns that same error indefinitely](https://users.rust-lang.org/t/step-past-errors-in-serde-json-streamdeserializer/84228) on subsequent `.next()` calls. For Claude Code's JSONL files --- which are append-only, may contain evolving schemas, and could have truncated final lines from interrupted writes --- this means a single bad record stops all ingestion.

**Why it happens:**
StreamDeserializer is designed for well-formed JSON streams, not line-delimited records that may individually fail. It tracks internal byte offsets for parsing state, and a structural error corrupts that state. JSONL is not technically "a JSON stream" --- it is newline-delimited independent JSON objects.

**How to avoid:**
Do NOT use `StreamDeserializer` for JSONL. Instead, read lines individually:
```rust
for line in reader.lines() {
    let line = line?;
    if line.trim().is_empty() { continue; }
    match serde_json::from_str::<Record>(&line) {
        Ok(record) => process(record),
        Err(e) => {
            log::warn!("Skipping malformed line at offset {}: {}", offset, e);
            // record the skip for observability, continue
        }
    }
    offset += line.len() + 1; // +1 for newline
}
```
This isolates errors to individual lines and maintains accurate byte offsets for resume capability.

**Warning signs:**
- Ingestion silently stops making progress (byte offset stops advancing)
- Error logs showing the same error repeatedly for a file
- Missing recent records despite the file growing

**Phase to address:**
Phase 1 (Ingestion Core). The line-by-line parsing strategy is fundamental and must be correct from the start.

---

### Pitfall 4: serde(flatten) Performance Penalty for Unknown Field Capture

**What goes wrong:**
Using `#[serde(flatten)] pub extra: HashMap<String, Value>` to capture unknown fields (essential for schema evolution) incurs roughly [2x deserialization overhead](https://crates.io/crates/serde_fast_flatten) compared to non-flattened structs. For high-volume ingestion of large JSONL files (10MB+), this measurably slows batch processing.

**Why it happens:**
serde's derive macro for `flatten` must buffer unrecognized fields from the parent structure and re-deserialize them for the child. Every field in the input is checked against all known fields first, then remainders are collected into the map. This prevents serde from using its normal zero-copy optimizations.

**How to avoid:**
- Accept the overhead initially --- correctness and schema resilience matter more than peak throughput in Phase 1.
- If profiling reveals this as a bottleneck, consider [serde_fast_flatten](https://crates.io/crates/serde_fast_flatten) which provides more efficient flattening.
- Alternatively, deserialize to `serde_json::Value` first, extract known fields manually, and store the remainder. This gives you full control over the performance/flexibility tradeoff.
- `#[serde(flatten)]` is incompatible with `#[serde(deny_unknown_fields)]` --- do not combine them.

**Warning signs:**
- Ingestion throughput noticeably below expectations when profiling
- flamegraph showing time spent in serde deserialization disproportionate to I/O

**Phase to address:**
Phase 1 (Schema Design). Decide the unknown-field capture strategy early. Changing from `flatten` to manual extraction later touches every struct definition.

---

### Pitfall 5: Byte-Offset Tracking Correctness (Partial Lines, Truncation)

**What goes wrong:**
The daemon tracks how far it has read into each JSONL file (byte offset) to resume after restart. Three failure modes break this:
1. **Partial final line**: File ends mid-write (Claude Code was interrupted). Reading to EOF captures an incomplete JSON object.
2. **File truncation**: If the file is ever truncated (unlikely for append-only, but possible during Claude Code updates), the stored offset points past the new end.
3. **Encoding mismatch**: Byte offset calculated from Rust `String` (UTF-8) may differ from raw file bytes if the file contains non-UTF-8 sequences or BOM.

**Why it happens:**
Append-only files seem safe but the "last line" problem is fundamental: there is no guarantee the file ends with a complete line at any given moment. File watchers trigger on any write, including partial flushes.

**How to avoid:**
- Only advance the stored byte offset after successfully parsing a complete line with a trailing newline.
- On startup, if stored offset exceeds file size, reset to 0 (file was truncated/rotated) and re-ingest.
- Read in chunks, split on newlines, and hold back the last fragment until the next read confirms it ends with `\n`.
- Persist offsets atomically (write to temp, rename) to avoid corruption on crash.
- Use `BufReader` on the raw file handle, not on a `String`-decoded stream, to keep byte offsets accurate.

**Warning signs:**
- Deserialization errors exclusively on the last line of files
- Duplicate records after daemon restart (offset was not persisted)
- Missing records after daemon restart (offset was persisted past actual successful parse)

**Phase to address:**
Phase 1 (Ingestion Core). The offset tracking logic is the core state machine of the ingestion engine. Getting it wrong causes data loss or duplication that compounds silently.

---

### Pitfall 6: notify Crate Platform Behavior Differences (macOS vs Linux)

**What goes wrong:**
The [notify](https://github.com/notify-rs/notify) crate uses FSEvents on macOS and inotify on Linux. These have fundamentally different event semantics:
- **FSEvents** monitors directories and [coalesces rapid changes temporally](https://github.com/notify-rs/notify/pull/371). Multiple writes in quick succession may produce a single event. Rename events lack the "cookie" mechanism to associate old/new paths.
- **inotify** monitors individual inodes and produces per-event notifications. It can [coalesce identical consecutive events](https://man7.org/linux/man-pages/man7/inotify.7.html) but is generally more granular.
- **Debouncing**: notify's default API debounces events; the raw API does not. Debounced mode fires once at the end of a quiet period, potentially delaying ingestion by the debounce interval.

**Why it happens:**
The kernel-level filesystem notification APIs are architecturally different across operating systems. notify abstracts over them but cannot fully hide the differences.

**How to avoid:**
- Do NOT rely on specific event types (Create vs Modify vs Rename) for correctness. Treat any event on a watched file as "file may have changed, re-check byte offset and read new data."
- Use notify's raw/non-debounced API for low-latency ingestion, then implement your own debouncing with a short timer (e.g., 100ms coalesce window) that batches filesystem events before triggering a read.
- On macOS, prefer FSEvents backend over kqueue. kqueue opens a file descriptor per watched file and [hits "too many open files" limits](https://github.com/notify-rs/notify/issues/596) with many files.
- Always have a periodic poll fallback (e.g., every 30s) that checks for changes even if no events fire. This catches edge cases where events are lost.

**Warning signs:**
- Ingestion works on Linux but misses changes on macOS (or vice versa)
- Files with rapid successive writes only partially ingested
- "Too many open files" errors on macOS with kqueue backend

**Phase to address:**
Phase 2 (File Watching). Design the watcher to be event-type-agnostic from the start. The periodic poll fallback should ship in the same phase.

---

### Pitfall 7: FTS5 Index Performance Degradation at Scale

**What goes wrong:**
FTS5 uses a [multi-level B-tree merge strategy](https://www.sqlite.org/fts5.html). Without tuning, the index accumulates many small segments during rapid inserts. Queries slow down because they must scan across all unmerged segments. At 100k+ records with frequent incremental inserts (the daemon's pattern), query latency can increase noticeably.

**Why it happens:**
FTS5's default `automerge=4` triggers a merge when 4+ segments exist at the same level. For workloads with many small insert batches (one per file-watch event), segments accumulate faster than they merge.

**How to avoid:**
- Batch FTS5 inserts within transactions (e.g., all records from one ingestion pass in a single transaction). This dramatically reduces segment count.
- Tune `automerge`: set to 8 or higher for write-heavy workloads (faster writes, slightly slower queries). Source: [FTS5 docs](https://www.sqlite.org/fts5.html).
- Use incremental optimization during idle periods:
  ```sql
  INSERT INTO fts_table(fts_table, rank) VALUES('merge', 500);
  ```
- Consider `detail=column` or `detail=none` if you do not need phrase/NEAR queries. This reduces index size by 54-82% per SQLite benchmarks.
- Use `content=` (external content table) to avoid storing text twice --- once in the main table and once in FTS.
- After bulk import, run `INSERT INTO fts_table(fts_table) VALUES('optimize');` to merge all segments.

**Warning signs:**
- Search queries that were fast initially get progressively slower
- Database file size growing faster than expected
- Write latency spikes during FTS merge operations

**Phase to address:**
Phase 2 or 3 (Search/FTS). Initial implementation can use defaults, but tuning should happen before the dataset grows past a few thousand records. External content tables should be planned in the schema from Phase 1.

---

## Moderate Pitfalls

### Pitfall 8: Stale Unix Domain Socket File on Startup

**What goes wrong:**
When the daemon crashes or is killed without cleanup, the UDS socket file remains on disk. On next startup, `UnixListener::bind()` fails with "address already in use" because the file exists.

**Prevention:**
Follow [axum's own UDS example pattern](https://github.com/tokio-rs/axum/blob/main/examples/unix-domain-socket/src/main.rs): remove the socket file before binding:
```rust
let _ = tokio::fs::remove_file(&socket_path).await; // ignore if not exists
tokio::fs::create_dir_all(socket_path.parent().unwrap()).await?;
let uds = UnixListener::bind(&socket_path)?;
```
Also consider file permissions on the socket --- default umask may make it inaccessible to other users. Use `tokio-listener` crate for higher-level UDS management if needed.

**Phase to address:**
Phase 2 (HTTP/UDS Server). This is a startup concern that should be handled when the server component is first implemented.

---

### Pitfall 9: Graceful Shutdown Incomplete --- Data Loss on SIGTERM

**What goes wrong:**
The daemon receives SIGTERM (from systemd stop, launchd unload, or Ctrl+C) while mid-ingestion. If shutdown is not graceful, partially-written transactions roll back (good) but the byte-offset checkpoint may not be persisted (bad --- causes re-ingestion or data loss on restart).

**Prevention:**
Use tokio's [CancellationToken + TaskTracker pattern](https://tokio.rs/tokio/topics/shutdown):
1. Catch SIGINT and SIGTERM via `tokio::signal`.
2. Cancel a shared `CancellationToken`.
3. Each subsystem (watcher, ingester, HTTP server) checks the token in its loop and enters cleanup.
4. Ingester flushes current transaction and persists byte offsets before exiting.
5. `TaskTracker::wait()` ensures all tasks complete before the process exits.
6. Set a hard timeout (e.g., 10s) after which the process force-exits, to avoid hanging on a stuck subsystem.

**Phase to address:**
Phase 2 (Daemon Management). Shutdown paths should be designed when the daemon lifecycle is formalized.

---

### Pitfall 10: Cross-Compilation with Bundled SQLite

**What goes wrong:**
Cross-compiling to `x86_64-unknown-linux-musl` (common for deploying static Linux binaries from macOS) without `rusqlite`'s `bundled` feature causes [segfaults or linker errors](https://github.com/rusqlite/rusqlite/issues/914) because the build tries to dynamically link a system SQLite that does not exist or is incompatible.

**Prevention:**
Always use the `bundled` feature for rusqlite, which compiles SQLite from the included amalgamation source via the `cc` crate:
```toml
[dependencies]
rusqlite = { version = "0.32", features = ["bundled", "fts5"] }
```
This produces a fully self-contained binary with no runtime SQLite dependency. If the project uses any TLS (e.g., for future HTTPS), prefer `rustls` over `openssl` to avoid the same class of cross-compilation pain with native C dependencies. OpenSSL cross-compilation is [widely documented as painful](https://users.rust-lang.org/t/cant-cross-compile-project-with-openssl/70922).

**Phase to address:**
Phase 1 (Project Setup). Cargo.toml feature flags should be set correctly from the first commit. Discovering this during CI/CD setup is frustrating but recoverable.

---

### Pitfall 11: serde_json Rejects NaN/Infinity (Valid in Some Producers)

**What goes wrong:**
JSON spec does not include NaN or Infinity. serde_json will reject them during deserialization. If Claude Code (or any upstream) ever emits these values (common in JavaScript-origin JSON where `JSON.stringify` behavior varies), the entire line fails to parse.

**Prevention:**
- Use line-by-line parsing (Pitfall 3) so a single bad value does not halt ingestion.
- If NaN/Infinity values are encountered in practice, consider a preprocessing step that sanitizes the raw line (regex replace `NaN` -> `null`, `Infinity` -> `null`) before deserialization. This is a pragmatic tradeoff.
- serde_json converts NaN/Infinity to `null` during *serialization* but [fails on deserialization from the literal strings "NaN"/"Infinity"](https://github.com/serde-rs/json/issues/202). This asymmetry is a known serde_json behavior.

**Phase to address:**
Phase 1 (Ingestion). Build the error-tolerant line parser from the start.

---

## Technical Debt Patterns

| Shortcut | Immediate Benefit | Long-term Cost | When Acceptable |
|----------|-------------------|----------------|-----------------|
| Single connection for reads+writes | Simpler code, no pool management | Write contention, SQLITE_BUSY under concurrent HTTP queries | Never --- split from the start, the cost is minimal |
| `unwrap()` on rusqlite errors | Faster prototyping | Panics crash the daemon on any DB error (disk full, corruption) | Phase 1 only, replace with proper error handling before daemon mode |
| No byte-offset persistence | Simpler ingestion loop | Full re-ingestion on every restart, duplicates | Phase 1 prototype only, must be added before any real use |
| StreamDeserializer instead of line-by-line | Slightly less code | Single bad line halts all ingestion permanently | Never --- line-by-line is barely more code and fundamentally more correct |
| Storing full JSON text in FTS5 content | Simpler schema | 2x storage (data in main table + FTS shadow tables) | Phase 1 if external content tables add complexity; migrate before scale |
| Hardcoded socket path | No configuration needed | Conflicts with multiple instances, inflexible deployment | Phase 1 only, make configurable before daemon management phase |

## Integration Gotchas

| Integration | Common Mistake | Correct Approach |
|-------------|----------------|------------------|
| Claude Code JSONL files | Assuming stable schema across versions | Use `serde(flatten)` or `Value` for unknown fields; store raw JSON alongside parsed fields for forward compatibility |
| tokio-rusqlite `call()` closures | Capturing non-`Send` types in the closure | The closure runs on a background thread --- everything captured must be `Send + 'static`. Clone/move data in, do not reference async task state |
| notify file watcher + tokio | Using blocking `Watcher::new()` event loop | Use `notify::RecommendedWatcher` with a channel, then receive events in an async task via `tokio::sync::mpsc` bridge |
| axum UDS + TCP dual listen | Trying to serve both from a single `axum::serve` call | Run two separate `axum::serve` tasks (one for TCP, one for UDS) on the same `Router`, each in its own spawned task |
| PID file management | Writing PID file but not cleaning up on exit | Use a RAII guard or shutdown hook that removes the PID file. Better yet, let systemd/launchd manage the process and skip PID files entirely |

## Performance Traps

| Trap | Symptoms | Prevention | When It Breaks |
|------|----------|------------|----------------|
| One-record-per-transaction FTS5 inserts | Slow ingestion, excessive I/O | Batch inserts in transactions of 100-1000 records | Noticeable above ~1000 records/session |
| Unoptimized FTS5 index | Search queries get progressively slower | Run incremental `merge` during idle, `optimize` after bulk loads | Above ~50k records without optimization |
| serde(flatten) on hot path | Deserialization is ~2x slower than without flatten | Profile; switch to manual extraction if bottleneck | When ingesting files >5MB with many fields |
| Blocking FTS5 queries during write transactions | HTTP search requests timeout during ingestion | Separate reader connections (WAL allows concurrent read+write) | Under concurrent query + ingestion load |
| Watching too many files with kqueue backend | macOS "too many open files" (EMFILE) | Use FSEvents backend on macOS (default for notify), set appropriate ulimits | When watching >200 files on macOS with kqueue |
| No PRAGMA optimize before close | Query planner uses stale statistics | Call `PRAGMA optimize;` on connection close | After significant data changes |

## Security Mistakes

| Mistake | Risk | Prevention |
|---------|------|------------|
| UDS socket with world-readable permissions | Any local user can query session history (which contains conversation content) | Set restrictive permissions (0600) on the socket file; verify with `stat` in tests |
| Storing raw JSONL paths in SQL without sanitization | Path traversal in API queries | Validate file paths are within expected Claude Code directory before storing |
| No rate limiting on search endpoint | Local DoS via expensive FTS5 queries | Implement basic request throttling even for local-only APIs |
| PID file in world-writable directory | PID file hijacking/symlink attacks | Use `$XDG_RUNTIME_DIR` or user-owned directories for PID files |

## "Looks Done But Isn't" Checklist

- [ ] **Ingestion**: Handles partial final line --- verify by truncating a test JSONL file mid-line and confirming the daemon does not error or lose data
- [ ] **Byte offset**: Survives daemon restart --- verify by restarting mid-ingestion and confirming no duplicates and no missing records
- [ ] **File rotation**: Detects when a file is replaced --- verify by moving a JSONL file and creating a new one at the same path
- [ ] **FTS5 search**: Returns results for records ingested during the current session --- verify FTS content table is synchronized
- [ ] **Shutdown**: Persists all state before exit --- verify by sending SIGTERM during active ingestion and confirming offset checkpoint is current
- [ ] **UDS socket**: Cleaned up on normal exit AND accessible after abnormal exit (stale file removed on startup) --- verify both paths
- [ ] **Schema evolution**: New unknown fields in JSONL do not cause errors --- verify by adding arbitrary fields to test data
- [ ] **Cross-platform**: Ingestion works identically on macOS and Linux --- verify with CI on both platforms, especially file-watch event handling
- [ ] **WAL mode**: Actually enabled --- verify with `PRAGMA journal_mode;` query (must be set per-connection, not persistent in some configurations)

## Recovery Strategies

| Pitfall | Recovery Cost | Recovery Steps |
|---------|---------------|----------------|
| Blocked runtime (Pitfall 1) | MEDIUM | Refactor all DB calls through tokio-rusqlite `call()`. Mechanical but touches many files. |
| Write contention (Pitfall 2) | HIGH | Restructure connection management. Requires new abstractions, retesting all DB paths. |
| StreamDeserializer stuck (Pitfall 3) | LOW | Replace with line-by-line reader. Localized to ingestion module. |
| Byte offset corruption (Pitfall 5) | MEDIUM | Re-ingest all files from scratch (slow but correct). Fix offset logic. Add integrity checks. |
| FTS5 degradation (Pitfall 7) | LOW | Run `optimize` command. For schema changes (external content), requires migration. |
| Stale socket (Pitfall 8) | LOW | Add `remove_file` before bind. One-line fix. |
| Shutdown data loss (Pitfall 9) | MEDIUM | Add cancellation token plumbing. Requires touching each subsystem's main loop. |
| Cross-compile failure (Pitfall 10) | LOW | Add `bundled` feature flag. Rebuild. |

## Pitfall-to-Phase Mapping

| Pitfall | Prevention Phase | Verification |
|---------|------------------|--------------|
| Blocking runtime (1) | Phase 1: Core Foundation | `tokio-console` shows no blocked workers during ingestion |
| Write contention (2) | Phase 1: Core Foundation | Load test with concurrent reads during writes --- no SQLITE_BUSY |
| StreamDeserializer (3) | Phase 1: Ingestion Core | Test with malformed lines interspersed in valid JSONL |
| serde(flatten) perf (4) | Phase 1: Schema Design, optimize in Phase 3 | Profile deserialization; compare with and without flatten |
| Byte offset (5) | Phase 1: Ingestion Core | Kill daemon mid-ingestion, restart, verify no duplicates/gaps |
| notify platform diffs (6) | Phase 2: File Watching | CI tests on both macOS and Linux with rapid file writes |
| FTS5 at scale (7) | Phase 2-3: Search | Benchmark queries after ingesting 100k records |
| Stale socket (8) | Phase 2: Server | Integration test: start, kill -9, start again --- no bind error |
| Graceful shutdown (9) | Phase 2: Daemon | Send SIGTERM during ingestion, verify offset persisted |
| Cross-compilation (10) | Phase 1: Project Setup | CI matrix with `x86_64-unknown-linux-musl` target |
| NaN/Infinity (11) | Phase 1: Ingestion | Test with JSONL containing literal NaN/Infinity values |

## Sources

- [tokio-rusqlite docs](https://docs.rs/tokio-rusqlite) --- architecture, thread-per-connection model
- [Tokio: Bridging sync and async](https://tokio.rs/tokio/topics/bridging) --- patterns for blocking code in async context
- [SQLite WAL mode docs](https://sqlite.org/wal.html) --- concurrent read/write semantics
- [Your SQLite Connection Pool Might Be Ruining Write Performance](https://emschwartz.me/psa-your-sqlite-connection-pool-might-be-ruining-your-write-performance/) --- single-writer architecture benchmarks
- [SQLite pragma cheatsheet](https://cj.rs/blog/sqlite-pragma-cheatsheet-for-performance-and-consistency/) --- recommended PRAGMA settings
- [SQLite FTS5 Extension docs](https://www.sqlite.org/fts5.html) --- automerge, optimization, detail levels
- [serde_json StreamDeserializer](https://docs.rs/serde_json/latest/serde_json/struct.StreamDeserializer.html) --- byte_offset, error behavior
- [StreamDeserializer error recovery discussion](https://users.rust-lang.org/t/step-past-errors-in-serde-json-streamdeserializer/84228) --- workarounds for stuck stream
- [serde flatten docs](https://serde.rs/attr-flatten.html) --- deny_unknown_fields incompatibility
- [serde_fast_flatten](https://crates.io/crates/serde_fast_flatten) --- performance alternative for flatten
- [serde_json NaN/Infinity issue](https://github.com/serde-rs/json/issues/202) --- serialization asymmetry
- [notify-rs GitHub](https://github.com/notify-rs/notify) --- platform backends, FSEvents vs kqueue
- [notify kqueue file descriptor issue](https://github.com/notify-rs/notify/issues/596) --- macOS "too many open files"
- [FSEvents rename handling PR](https://github.com/notify-rs/notify/pull/371) --- RenameMode::Any for macOS
- [axum UDS example](https://github.com/tokio-rs/axum/blob/main/examples/unix-domain-socket/src/main.rs) --- stale socket cleanup pattern
- [Tokio graceful shutdown guide](https://tokio.rs/tokio/topics/shutdown) --- CancellationToken + TaskTracker
- [rusqlite musl segfault issue](https://github.com/rusqlite/rusqlite/issues/914) --- bundled feature requirement
- [libsqlite3-sys crate](https://crates.io/crates/libsqlite3-sys) --- bundled feature, SQLCipher/OpenSSL linking
- [OpenSSL cross-compilation pain](https://users.rust-lang.org/t/cant-cross-compile-project-with-openssl/70922) --- prefer rustls

---
*Pitfalls research for: Rust JSONL-to-SQLite ingestion daemon*
*Researched: 2026-02-20*
