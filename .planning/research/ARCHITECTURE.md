# Architecture Research

**Domain:** Rust CLI daemon with JSONL ingestion, SQLite store, and multi-interface API
**Researched:** 2026-02-20
**Confidence:** HIGH

## System Overview

```
                       ┌──────────────────────────────────────────────────────────────────┐
                       │                         Consumers                                │
                       │    scripts, hooks, dashboards, Claude Code hooks, MCP servers    │
                       └──────┬─────────────────────┬──────────────────────┬──────────────┘
                              │ HTTP/JSON            │ Unix Socket          │ CLI (stdout)
                              ▼                      ▼                      ▼
┌──────────────────────────────────────────────────────────────────────────────────────────┐
│                              claude-history binary                                       │
│                                                                                          │
│  ┌─────────────────────────────────┐    ┌──────────────────────────────────────────┐     │
│  │         server crate             │    │              CLI dispatch                 │     │
│  │  ┌──────────┐  ┌──────────────┐ │    │  clap parser → subcommand routing         │     │
│  │  │ axum     │  │ UDS listener │ │    │  daemon-aware: socket if running,          │     │
│  │  │ (TCP)    │  │ (tokio UDS)  │ │    │  direct DB if not                          │     │
│  │  └────┬─────┘  └──────┬───────┘ │    └──────────────────┬───────────────────────┘     │
│  │       │               │          │                       │                             │
│  │       └───────┬───────┘          │                       │                             │
│  │               │                  │                       │                             │
│  │        ┌──────▼──────┐           │                       │                             │
│  │        │   Routes    │           │                       │                             │
│  │        │ + SSE hub   │           │                       │                             │
│  │        └──────┬──────┘           │                       │                             │
│  └───────────────┼──────────────────┘                       │                             │
│                  │                                          │                             │
│                  ▼                                          ▼                             │
│  ┌──────────────────────────────────────────────────────────────────────────────────┐    │
│  │                              store crate                                         │    │
│  │                                                                                  │    │
│  │   ┌──────────────┐  ┌──────────────┐  ┌────────────┐  ┌──────────────────┐      │    │
│  │   │ Decomposer   │  │ Query Engine │  │ FTS5 index │  │ File Watcher     │      │    │
│  │   │ + Artifacts   │  │ + builder   │  │            │  │ (notify → mpsc)  │      │    │
│  │   └──────┬───────┘  └──────┬───────┘  └─────┬──────┘  └───────┬──────────┘      │    │
│  │          │                 │                 │                 │                  │    │
│  │          ▼                 ▼                 ▼                 │                  │    │
│  │   ┌──────────────────────────────────────────────────────┐    │                  │    │
│  │   │                  DbPool                               │    │                  │    │
│  │   │   writer: single Connection (tokio-rusqlite)          │◄───┘                  │    │
│  │   │   readers: pool of N read-only Connections            │                       │    │
│  │   └────────────────────────┬─────────────────────────────┘                       │    │
│  └────────────────────────────┼─────────────────────────────────────────────────────┘    │
│                               ▼                                                          │
│  ┌──────────────────────────────────────────────────────────────────────────────────┐    │
│  │                              core crate                                           │    │
│  │   serde models · JSONL parser · version detection · drift capture                 │    │
│  └──────────────────────────────────────────────────────────────────────────────────┘    │
│                               ▼                                                          │
│               SQLite (rusqlite + FTS5, WAL mode)                                         │
└──────────────────────────────────────────────────────────────────────────────────────────┘
         ▲ watches
         │
  ~/.claude/projects/**/*.jsonl
```

### Component Responsibilities

| Component | Responsibility | Typical Implementation |
|-----------|----------------|------------------------|
| **core crate** | Serde type modeling, JSONL streaming parser, version detection, schema drift capture | Pure library: no IO, no async runtime. `serde`, `serde_json`, `semver`. Blocking `std::io::BufRead` for parser. |
| **store crate** | SQLite schema/migrations, record decomposition, artifact extraction, incremental byte-offset sync, file watcher, FTS5 integration, query builder | `rusqlite` (bundled, fts5 features), `tokio-rusqlite` for async bridge, `notify` for file system events, `walkdir` for bulk scan |
| **server crate** | HTTP API (axum), Unix domain socket listener, CLI subcommand dispatch (clap), SSE event broadcasting, daemon lifecycle, graceful shutdown | `axum`, `tokio`, `clap`, `tower-http`, `hyper-util`. Single `main.rs` binary entry point. |
| **DbPool** | Manages split writer/reader connections to SQLite; serializes all writes through a single connection, fans out reads across a pool | `tokio-rusqlite` for the writer connection; reader pool of N `tokio-rusqlite` connections opened with `PRAGMA query_only = ON` |
| **File Watcher** | Watches `~/.claude/projects/` for JSONL changes, debounces events, triggers incremental sync via the writer connection | `notify::recommended_watcher` on a dedicated OS thread, events sent to tokio via `mpsc::channel`, debounce logic in the async receiver |
| **SSE Hub** | Broadcast channel for real-time events (record:added, schema:drift, sync:complete); SSE clients subscribe | `tokio::sync::broadcast` channel; axum SSE response wraps a broadcast receiver |
| **CLI Dispatch** | Parses subcommands, decides whether to connect to running daemon (via UDS) or open DB directly in read-only mode | `clap` derive API. Socket probe: attempt UDS connect → if refused, fall back to direct `rusqlite` read-only connection. |

## Recommended Project Structure

```
claude-history/
├── Cargo.toml                        # [workspace] with members and shared deps
├── Cargo.lock
├── crates/
│   ├── core/
│   │   ├── Cargo.toml                # serde, serde_json, semver, thiserror
│   │   └── src/
│   │       ├── lib.rs                # pub mod declarations
│   │       ├── record.rs             # JSONLRecord enum (top-level discriminated union)
│   │       ├── message.rs            # ContentBlock, MessageContent, UsageStats
│   │       ├── config.rs             # .claude.json schema types
│   │       ├── parser.rs             # Streaming JSONL reader (byte-offset resume)
│   │       └── version.rs            # Version detection, drift event types
│   │
│   ├── store/
│   │   ├── Cargo.toml                # rusqlite (bundled+fts5), tokio-rusqlite, notify, walkdir
│   │   └── src/
│   │       ├── lib.rs                # pub mod + DbPool struct
│   │       ├── pool.rs               # Writer/reader split pool (see Pattern 1)
│   │       ├── schema.rs             # DDL, embedded migrations, PRAGMA setup
│   │       ├── decompose.rs          # Record → normalized rows
│   │       ├── decompose_artifacts.rs # File ops, git ops extraction
│   │       ├── reconstruct.rs        # File content replay at any message point
│   │       ├── sync.rs               # Incremental byte-offset sync engine
│   │       ├── watcher.rs            # notify-based file watcher → mpsc bridge
│   │       ├── query.rs              # Query builder → parameterized SQL
│   │       └── fts.rs                # FTS5 index maintenance and search
│   │
│   └── server/
│       ├── Cargo.toml                # axum, tokio, clap, tower-http, hyper-util
│       └── src/
│           ├── main.rs               # #[tokio::main], clap CLI entry, subcommand routing
│           ├── daemon.rs             # Daemon lifecycle: start, pidfile, socket cleanup
│           ├── serve.rs              # Dual listener setup (TCP + UDS), graceful shutdown
│           ├── state.rs              # AppState: DbPool + broadcast::Sender + config
│           ├── client.rs             # CLI-to-daemon HTTP client (over UDS)
│           ├── routes/
│           │   ├── mod.rs            # Router assembly
│           │   ├── sessions.rs
│           │   ├── messages.rs
│           │   ├── search.rs
│           │   ├── analytics.rs
│           │   ├── files.rs          # Artifact: files, content reconstruction, diff
│           │   ├── git.rs            # Artifact: git operations
│           │   ├── artifacts.rs      # Combined artifact views, timeline
│           │   ├── schema.rs         # Version, drift endpoints
│           │   ├── export.rs
│           │   └── health.rs
│           └── events.rs             # SSE broadcast hub, event types
│
├── tests/
│   ├── fixtures/                     # Real JSONL samples (anonymized)
│   └── integration/                  # End-to-end tests against real DB
│
├── contrib/
│   ├── com.claude-history.daemon.plist   # macOS launchd plist
│   └── claude-history.service            # Linux systemd unit
│
└── openapi.yaml                      # API contract (source of truth)
```

### Structure Rationale

- **crates/core/:** Zero-dependency on async runtime or database. Pure serde types + synchronous parser. Any crate can depend on it without pulling in tokio. This is the foundational layer — build it first, test it independently against real JSONL fixtures.
- **crates/store/:** Owns the database. All SQL lives here. Exposes an async-friendly API via `tokio-rusqlite` but the internal decomposition logic is synchronous (runs inside `Connection::call` closures). The file watcher bridges OS threads to tokio channels.
- **crates/server/:** The only crate with `#[tokio::main]`. Owns the HTTP API, CLI dispatch, and daemon lifecycle. Depends on both `core` and `store`.
- **contrib/:** Daemon management configs ship with the binary but are not compiled into it. Users install them as needed.

## Architectural Patterns

### Pattern 1: Split Writer/Reader Pool for SQLite

**What:** Mirror SQLite's internal concurrency model at the application level. One dedicated writer connection serializes all mutations. A separate pool of read-only connections handles concurrent queries.

**When to use:** Always, for any SQLite application serving concurrent async requests. SQLite allows unlimited concurrent readers in WAL mode but only one writer at a time. If you pool multiple writer connections, async tasks holding the write lock will yield to the runtime, allowing other tasks to attempt writes, hit `SQLITE_BUSY`, and degrade performance dramatically.

**Confidence:** HIGH — This pattern is well-documented. Evan Schwartz's 2024 benchmarks showed the split-pool approach was approximately 20x faster than a 50-connection pool for write-heavy workloads (83ms vs 1.93s).

**Trade-offs:**
- Pro: Eliminates write contention entirely. Reads never block writes and vice versa (WAL mode).
- Pro: Makes the architectural intent explicit — you can see which code paths write vs read.
- Con: Slightly more complex pool setup. Two connection handles to manage rather than one.
- Con: If a read endpoint accidentally needs to write, it must be refactored to use the writer.

**Implementation sketch:**

```rust
// crates/store/src/pool.rs

use tokio_rusqlite::Connection;

pub struct DbPool {
    writer: Connection,           // single connection, all writes serialize here
    readers: Vec<Connection>,     // N read-only connections
    next_reader: AtomicUsize,     // round-robin index
}

impl DbPool {
    pub async fn open(path: &Path, reader_count: usize) -> Result<Self> {
        let writer = Connection::open(path).await?;
        writer.call(|conn| {
            conn.pragma_update(None, "journal_mode", "WAL")?;
            conn.pragma_update(None, "busy_timeout", "5000")?;
            conn.pragma_update(None, "synchronous", "NORMAL")?;
            conn.pragma_update(None, "foreign_keys", "ON")?;
            Ok(())
        }).await?;

        let mut readers = Vec::with_capacity(reader_count);
        for _ in 0..reader_count {
            let r = Connection::open(path).await?;
            r.call(|conn| {
                conn.pragma_update(None, "query_only", "ON")?;
                conn.pragma_update(None, "journal_mode", "WAL")?;
                Ok(())
            }).await?;
            readers.push(r);
        }

        Ok(Self { writer, readers, next_reader: AtomicUsize::new(0) })
    }

    pub fn writer(&self) -> &Connection { &self.writer }

    pub fn reader(&self) -> &Connection {
        let idx = self.next_reader.fetch_add(1, Ordering::Relaxed) % self.readers.len();
        &self.readers[idx]
    }
}
```

### Pattern 2: Blocking-to-Async Bridge via `tokio-rusqlite`

**What:** `tokio-rusqlite` spawns a dedicated OS thread per connection. The `Connection::call()` method sends a boxed closure to that thread via crossbeam channel, executes it synchronously on the rusqlite `Connection`, and returns the result via a oneshot channel. This avoids blocking the tokio executor.

**When to use:** Everywhere rusqlite is accessed from async code. The alternative (`tokio::task::spawn_blocking`) is lower-level and doesn't give you a stable `Connection` handle. `tokio-rusqlite` wraps this pattern with a clonable handle and proper lifecycle management.

**Confidence:** HIGH — `tokio-rusqlite` is at version 0.7.0, actively maintained, and this is the standard pattern for async-SQLite in Rust.

**Trade-offs:**
- Pro: `Connection` is `Clone` + `Send` — can be stored in axum `State` and shared across handlers.
- Pro: No risk of blocking the tokio runtime. Each connection has its own OS thread.
- Con: Each connection costs one OS thread. For this application (3-5 connections), this is negligible.
- Con: Closures passed to `call()` must be `FnOnce + Send + 'static`, which means you cannot borrow from the surrounding async scope. All data must be moved or cloned into the closure.

**Why not `deadpool-sqlite`:** deadpool-sqlite (v0.13.0) provides a full async pool abstraction, but for this project it adds unnecessary indirection. The split writer/reader pattern is better expressed explicitly. `tokio-rusqlite` gives direct control over which connections are read-only vs read-write. deadpool-sqlite would be a better fit if the project needed dynamic pool resizing or multi-backend support, neither of which applies here.

### Pattern 3: Dual Listener (TCP + UDS) via Separate Tokio Tasks

**What:** Spawn two independent `axum::serve` instances — one bound to a `TcpListener`, one to a `UnixListener` — sharing the same `Router` and `AppState`. Both are governed by the same graceful shutdown signal.

**When to use:** When the daemon must accept connections from both localhost HTTP clients (browsers, curl, scripts in any language) and local Unix domain socket clients (the CLI tool, fast IPC).

**Confidence:** HIGH — Axum's `serve` function accepts any type implementing the `Listener` trait. The official axum unix-domain-socket example demonstrates UDS serving. Running two listeners in separate tasks is a straightforward tokio pattern.

**Trade-offs:**
- Pro: Same router, same state, same handlers for both transports. No code duplication.
- Pro: UDS is faster for local IPC (no TCP overhead), while TCP is accessible to any HTTP client.
- Con: Two listeners means two tasks to manage during shutdown.
- Con: UDS socket file must be cleaned up on shutdown and checked for stale files on startup.

**Implementation sketch:**

```rust
// crates/server/src/serve.rs

pub async fn run_server(state: AppState, config: &ServerConfig) -> Result<()> {
    let app = build_router(state);
    let shutdown = shutdown_signal();

    let tcp = TcpListener::bind(&config.bind_addr).await?;
    let uds_path = config.socket_path();

    // Clean stale socket file
    if uds_path.exists() {
        std::fs::remove_file(&uds_path)?;
    }
    let uds = UnixListener::bind(&uds_path)?;

    let tcp_server = axum::serve(tcp, app.clone())
        .with_graceful_shutdown(shutdown.clone());
    let uds_server = axum::serve(uds, app)
        .with_graceful_shutdown(shutdown);

    tokio::select! {
        r = tcp_server => r?,
        r = uds_server => r?,
    }

    // Cleanup socket file
    let _ = std::fs::remove_file(&uds_path);
    Ok(())
}
```

Note: The actual shutdown signal sharing requires a `CancellationToken` or similar mechanism rather than a cloned future. `tokio_util::sync::CancellationToken` is the standard approach.

### Pattern 4: CLI-to-Daemon Socket Probing

**What:** When the user runs a query subcommand (e.g., `claude-history query sessions`), the CLI first attempts to connect to the daemon's Unix domain socket. If the connection succeeds, it forwards the request as HTTP-over-UDS. If the connection is refused or the socket file does not exist, the CLI opens the SQLite database directly in read-only mode and executes the query locally.

**When to use:** Every read-only CLI subcommand. Write operations (like `claude-history sync`) should always go through the daemon if running, to avoid write contention.

**Confidence:** MEDIUM — This is the Docker-style pattern (docker CLI talks to dockerd over `/var/run/docker.sock`). The probe-and-fallback approach is application-level logic rather than a library feature, so the implementation details may need iteration.

**Trade-offs:**
- Pro: CLI works even when the daemon is not running. Graceful degradation.
- Pro: When daemon is running, CLI gets live data including in-progress sync results.
- Con: Two code paths for the same operation (HTTP client vs direct DB). Risk of divergence.
- Con: Direct DB access means the CLI must know the DB path and how to open it correctly.

**Mitigation for code path divergence:** The query builder in the `store` crate is the shared implementation. Both the HTTP handler and the CLI-direct path call the same `store` functions. The HTTP path just adds serialization/deserialization overhead.

### Pattern 5: File Watcher Bridge (OS Thread to Tokio)

**What:** The `notify` crate's recommended watcher runs on an OS thread (it uses platform-specific APIs: FSEvents on macOS, inotify on Linux). Events are sent into the tokio ecosystem via `tokio::sync::mpsc::Sender::blocking_send()`, then processed in an async task with debouncing.

**When to use:** For the daemon's real-time file watching. The watcher thread must stay alive for the lifetime of the daemon.

**Confidence:** HIGH — This is the documented pattern for integrating `notify` with tokio. The `blocking_send` method exists specifically for this cross-thread scenario.

**Trade-offs:**
- Pro: Decouples the watcher's blocking nature from the async runtime.
- Pro: Debouncing in the async receiver is straightforward with `tokio::time::Instant`.
- Con: The watcher thread is parked with `std::thread::park()` — it exists solely to keep the watcher alive. This is slightly unusual but correct.
- Con: If the mpsc channel fills (bounded at 100), events are dropped. For file watching with debounce, this is acceptable — a missed event just means a slightly delayed sync.

## Data Flow

### Sync Flow (Daemon Mode — Primary Path)

```
JSONL file modified on disk
         │
         ▼
notify::Watcher (OS thread, FSEvents/inotify)
         │ blocking_send()
         ▼
tokio::sync::mpsc::Receiver (async task)
         │ debounce (2s window per path)
         ▼
IncrementalSync::sync_file(path)
         │ reads bytes from last_offset → EOF
         ▼
core::parser::parse_jsonl(path, offset)
         │ returns Vec<JSONLRecord> + warnings
         ▼
store::Decomposer::decompose(record, &tx)    ← runs inside writer Connection::call()
         │ INSERT into messages, content, usage, tools, etc.
         │ ArtifactDecomposer extracts file_ops, git_ops
         │ DriftDetector logs overflow fields
         ▼
UPDATE sync_metadata SET last_byte_offset = new_offset
         │
         ▼
broadcast::Sender::send(Event::RecordAdded { ... })
         │
         ▼
SSE clients receive real-time notification
```

### Query Flow (HTTP Request)

```
HTTP GET /v1/sessions?project=myapp&limit=10
         │
         ▼
axum Router → sessions::list handler
         │ extracts query params
         ▼
AppState.pool.reader()
         │ round-robin selects a read-only Connection
         ▼
reader.call(|conn| {
    QueryBuilder::sessions()
        .project("myapp")
        .limit(10)
        .execute(conn)
})
         │ returns Vec<Session>
         ▼
axum::Json(sessions) → HTTP 200 response
```

### CLI Query Flow (No Daemon Running)

```
claude-history query sessions --project myapp --limit 10
         │
         ▼
clap parses subcommand + args
         │
         ▼
Probe UDS socket at ~/.claude/.claude-history.sock
         │ connect() fails → ECONNREFUSED or ENOENT
         ▼
Open SQLite DB directly: read-only mode
         │ Connection::open_with_flags(SQLITE_OPEN_READ_ONLY)
         ▼
QueryBuilder::sessions()
    .project("myapp")
    .limit(10)
    .execute(&conn)
         │
         ▼
Serialize to JSON → stdout
```

### CLI Query Flow (Daemon Running)

```
claude-history query sessions --project myapp --limit 10
         │
         ▼
clap parses subcommand + args
         │
         ▼
Probe UDS socket at ~/.claude/.claude-history.sock
         │ connect() succeeds
         ▼
HTTP GET /v1/sessions?project=myapp&limit=10 over UDS
         │ using hyper HTTP client with Unix connector
         ▼
Parse JSON response → format to stdout
```

### Key Data Flows

1. **Ingestion pipeline:** JSONL file change → notify event → debounce → parse → decompose → SQLite (writer connection only). This is the write-hot path; all mutations flow through the single writer.
2. **Query pipeline:** HTTP/CLI request → read-only connection from pool → query builder → SQLite → JSON response. Reads never contend with writes (WAL mode).
3. **Real-time events:** Decomposer emits events post-commit → `broadcast::Sender` → all subscribed SSE clients receive updates. The broadcast channel is bounded; slow clients miss events (acceptable for this use case).
4. **Bulk import:** `claude-history sync` walks all JSONL files, calling `sync_file` for each. Same writer connection, same decomposer. Progress reported to stdout or via SSE if daemon.

## Scaling Considerations

This is a single-user, local-only tool. "Scaling" means handling large JSONL histories efficiently, not multi-tenant load.

| Concern | At 100 sessions | At 10K sessions | At 100K+ sessions |
|---------|------------------|-----------------|--------------------|
| Initial bulk import | Seconds. Single transaction per file. | Minutes. May want progress bar + batch commits (every N files). | Consider chunked transactions (1000 records per commit) to avoid WAL file growth. |
| Incremental sync | Instant. Byte offsets mean only new data is read. | Instant per file. The watcher only triggers on changed files. | Same — byte offsets make this O(new bytes), not O(total bytes). |
| FTS5 search | Fast. SQLite FTS5 is highly optimized for local datasets. | Fast. FTS5 handles millions of rows well. | May need `optimize` command run periodically. |
| Database file size | ~5-20 MB | ~200 MB - 1 GB | 1-5 GB. Consider content truncation policies for large tool outputs. |
| Reader pool size | 2 readers sufficient | 2-4 readers | 4 readers. Unlikely to be the bottleneck; SQLite read throughput is very high for local SSD. |

### Scaling Priorities

1. **First bottleneck — bulk import speed:** When a user first installs and runs `claude-history sync` against a large existing history, the initial import is the slowest operation. Mitigation: batch transactions (commit every N files rather than per-file), progress reporting, and optionally a `--parallel` flag that uses multiple parser threads feeding into the single writer.
2. **Second bottleneck — database size:** Large tool outputs (multi-thousand-line file contents, base64 images) inflate the DB. Mitigation: configurable content truncation, optional external storage for large blobs, and a `claude-history vacuum` subcommand.

## Anti-Patterns

### Anti-Pattern 1: Multiple Writer Connections in a Pool

**What people do:** Create a deadpool or r2d2 pool of 10+ read-write connections to SQLite, thinking this improves write throughput.
**Why it's wrong:** SQLite allows exactly one writer at a time. In an async context, a task holding the write lock yields to the executor, which schedules another task that tries to write, hits `SQLITE_BUSY`, waits for `busy_timeout`, and either errors or degrades performance. The original task cannot progress because it is parked. Benchmarks show approximately 20x slowdown compared to a single writer.
**Do this instead:** Single writer connection + multiple read-only connections (Pattern 1 above).

### Anti-Pattern 2: Running rusqlite Directly on the Tokio Runtime

**What people do:** Call `rusqlite::Connection` methods directly inside an `async fn` handler without `spawn_blocking` or `tokio-rusqlite`.
**Why it's wrong:** rusqlite operations are blocking system calls (file I/O). Running them on the tokio runtime blocks the executor thread, starving other tasks. Even fast queries (sub-millisecond) can cause cascading delays under load.
**Do this instead:** Use `tokio-rusqlite` (which manages a dedicated thread per connection) or explicitly wrap every DB call in `tokio::task::spawn_blocking`. The former is strongly preferred because it provides a stable, clonable connection handle.

### Anti-Pattern 3: Shared Mutable State for Event Broadcasting

**What people do:** Use `Arc<Mutex<Vec<Sender>>>` to track SSE subscribers, locking the mutex on every event send and every subscribe/unsubscribe.
**Why it's wrong:** Lock contention on the subscriber list. The mutex must be held while iterating and sending, which blocks new subscriptions during event broadcast.
**Do this instead:** Use `tokio::sync::broadcast` channel. It is lock-free for sending, supports multiple receivers via `subscribe()`, and automatically cleans up dropped receivers. The bounded buffer means slow clients miss events rather than causing backpressure — which is the correct behavior for SSE event streams.

### Anti-Pattern 4: Daemon That Forks (Daemonize Itself)

**What people do:** Have the binary call `fork()` and have the parent exit, traditional Unix daemon style.
**Why it's wrong:** Apple's `launchd` documentation explicitly states: "A daemon launched by launchd MUST NOT call `daemon(3)` or do the moral equivalent by calling `fork(2)` and have the parent process exit." Modern init systems (launchd, systemd) manage the daemon lifecycle. Double-forking conflicts with their process tracking.
**Do this instead:** Run in the foreground. Let `launchd` (macOS) or `systemd` (Linux) manage backgrounding, restart-on-failure, and log capture. The binary should run `serve` as a normal foreground process.

## Integration Points

### External Services

| Service | Integration Pattern | Notes |
|---------|---------------------|-------|
| Claude Code JSONL files | Read-only file system access + `notify` watcher | The binary never writes to Claude Code's files. Read-only is a hard constraint. |
| macOS launchd | `com.claude-history.daemon.plist` in `~/Library/LaunchAgents/` | `KeepAlive: true`, `RunAtLoad: true`. stdout/stderr to log files. Socket path in plist for potential socket activation. |
| Linux systemd | `claude-history.service` user unit in `~/.config/systemd/user/` | `Type=exec` (not forking). `Restart=on-failure`. Optionally `ListenStream=` for socket activation via `listenfd`. |
| MCP servers / scripts | HTTP API or UDS | Any language that speaks HTTP can consume the API. The UDS path is the preferred local IPC channel for performance. |

### Internal Boundaries

| Boundary | Communication | Notes |
|----------|---------------|-------|
| core → store | Function calls (sync, in-process) | `store` depends on `core` types. Parser returns `Vec<JSONLRecord>` consumed by decomposer. |
| store → server | Async method calls via `DbPool` | Server holds `AppState` containing `DbPool` + `broadcast::Sender`. Handlers call `pool.reader().call(...)` or `pool.writer().call(...)`. |
| File Watcher → Sync Engine | `tokio::sync::mpsc` channel | OS thread sends `notify::Event`, async task receives, debounces, calls `sync_file()`. |
| Sync Engine → SSE Hub | `tokio::sync::broadcast` channel | After successful sync, sends `Event::RecordAdded` etc. SSE handlers subscribe to this channel. |
| CLI → Daemon | HTTP over UDS (or fallback to direct DB) | CLI uses `hyper` client with Unix socket connector. Falls back to opening DB read-only. |

## Build Order (Dependency Chain)

The crate dependency graph dictates build order. Each phase should be independently testable before the next begins.

```
Phase 1: core crate
   │      ↓ no dependencies on store or server
   │      Testable with: JSONL fixture files, unit tests for serde roundtrips
   │
Phase 2: store crate (depends on core)
   │      ↓ schema + decomposer + sync engine
   │      Testable with: in-memory SQLite, fixture JSONL → verify decomposed rows
   │
Phase 3: store crate — watcher + FTS + query builder
   │      ↓ adds runtime components to store
   │      Testable with: temp directories + real file watching, FTS search tests
   │
Phase 4: server crate — CLI + HTTP API (depends on core + store)
   │      ↓ axum routes, clap subcommands, dual listener
   │      Testable with: integration tests hitting HTTP endpoints against test DB
   │
Phase 5: server crate — SSE + daemon lifecycle + CLI-to-daemon
          ↓ real-time events, graceful shutdown, launchd/systemd configs
          Testable with: end-to-end tests: start daemon, CLI queries, SSE subscription
```

**Key dependency observations:**
- `core` has zero dependencies on async runtime or database — it can be built and tested first with zero infrastructure.
- `store` depends on `core` types but not on `server`. The decomposer and sync engine are testable without HTTP.
- `server` depends on both `core` and `store`. It is the integration layer and should be built last.
- The file watcher and SSE hub can be developed in parallel once `store` basics are in place.
- The CLI-to-daemon socket probing is a Phase 5 concern — get the HTTP API working first, then add the smart dispatch.

## Graceful Shutdown Strategy

The daemon has multiple concurrent subsystems that must shut down cleanly:

```
SIGTERM / SIGINT received
         │
         ▼
CancellationToken::cancel()
         │
         ├──→ TCP listener stops accepting new connections
         ├──→ UDS listener stops accepting new connections
         ├──→ In-flight HTTP requests drain (with timeout)
         ├──→ File watcher thread receives shutdown signal (channel drop)
         ├──→ SSE broadcast channel is dropped (clients get stream end)
         ├──→ Version monitor loop exits
         │
         ▼
await all tasks with timeout (e.g., 10 seconds)
         │
         ▼
DbPool::close() — flush WAL, close connections
         │
         ▼
Remove UDS socket file
Remove PID file
Exit 0
```

Use `tokio_util::sync::CancellationToken` to coordinate shutdown across all subsystems. Each long-running task checks `token.cancelled()` or uses `token.cancelled().await` in a `tokio::select!` block.

## Sources

- [Evan Schwartz — "Your SQLite Connection Pool Might Be Ruining Your Write Performance"](https://emschwartz.me/psa-your-sqlite-connection-pool-might-be-ruining-your-write-performance/) — HIGH confidence, benchmark data for split writer/reader pattern
- [tokio-rusqlite documentation (v0.7.0)](https://docs.rs/tokio-rusqlite) — HIGH confidence, official crate docs
- [deadpool-sqlite documentation (v0.13.0)](https://docs.rs/deadpool-sqlite) — HIGH confidence, official crate docs
- [axum graceful shutdown example](https://github.com/tokio-rs/axum/blob/main/examples/graceful-shutdown/src/main.rs) — HIGH confidence, official axum repo
- [axum unix domain socket example](https://github.com/tokio-rs/axum/blob/main/examples/unix-domain-socket/src/main.rs) — HIGH confidence, official axum repo
- [SQLite WAL documentation](https://sqlite.org/wal.html) — HIGH confidence, official SQLite docs
- [tokio-listener crate (v0.5.2)](https://lib.rs/crates/tokio-listener) — MEDIUM confidence, third-party crate; noted as alternative to manual dual-listener setup
- [Apple launchd documentation](https://developer.apple.com/library/archive/documentation/MacOSX/Conceptual/BPSystemStartup/Chapters/CreatingLaunchdJobs.html) — HIGH confidence, official Apple docs
- [Armin Ronacher — "What is systemfd/listenfd?"](https://lucumr.pocoo.org/2025/1/19/what-is-systemfd/) — MEDIUM confidence, reputable author, covers socket activation pattern for Rust
- [axum SSE documentation](https://docs.rs/axum/latest/axum/response/sse/) — HIGH confidence, official crate docs

---
*Architecture research for: Rust CLI daemon with JSONL ingestion, SQLite store, and multi-interface API*
*Researched: 2026-02-20*
