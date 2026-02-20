# Phase 4: Real-Time Ingestion and Events - Research

**Researched:** 2026-02-20
**Domain:** Filesystem watching, Server-Sent Events, async event broadcasting
**Confidence:** HIGH

<spec_constraints>
## Spec Constraints (IMMUTABLE -- from ROADMAP.md Success Criteria)

**Success Criteria** (what must be TRUE):
  1. While the daemon is running, starting a new Claude Code session causes the session and its messages to appear in API/CLI query results within seconds -- without manual sync
  2. A client connected to GET /v1/events receives record:added and session:started SSE events as new JSONL data is written by Claude Code
  3. schema:drift and version:changed SSE events fire when new overflow fields or Claude Code version changes are detected during live ingestion
  4. The file watcher debounces rapid writes (minimum 2-second gap per file) and recovers gracefully from transient filesystem errors

**Requirements**: WATCH-01, WATCH-02, WATCH-03, SSE-01, SSE-02, SSE-03, SSE-04, SSE-05

These are non-negotiable user-story outcomes. Research recommendations must not narrow below these.
</spec_constraints>

## Summary

Phase 4 adds two interconnected subsystems to the daemon: a file watcher that detects JSONL changes in real time, and an SSE broadcast layer that pushes events to connected HTTP clients. The file watcher uses the `notify` crate running on a blocking thread, forwarding filesystem events to tokio via an `mpsc` channel. The existing `sync_file` function from the store crate handles actual ingestion -- the watcher only needs to trigger it. The SSE layer uses axum's built-in `axum::response::sse` module backed by a `tokio::sync::broadcast` channel, which naturally supports multiple concurrent subscribers.

The architecture bridges three execution contexts: (1) the `notify` watcher callback on a blocking thread, (2) the tokio event loop processing file change events and running sync operations, and (3) axum SSE handlers streaming events to HTTP clients. The `tokio::sync::mpsc` channel (with `blocking_send` from the watcher thread) bridges the first two contexts. The `tokio::sync::broadcast` channel bridges the second and third -- the sync processor sends events to broadcast, and each SSE client subscribes to receive them.

**Primary recommendation:** Use `notify` 8.x (stable) with custom per-file debounce logic (HashMap<PathBuf, Instant>) rather than `notify-debouncer-full`, because the spec requires exact 2-second per-file debounce semantics that are simpler to implement and test with direct control. Use `tokio::sync::broadcast` for SSE fan-out, wrapped in `tokio_stream::wrappers::BroadcastStream` for the axum SSE handler.

## Standard Stack

### Core
| Library | Version | Purpose | Why Standard |
|---------|---------|---------|--------------|
| notify | 8.2.0 | Cross-platform filesystem event notifications | De facto Rust standard; used by cargo-watch, rust-analyzer, deno. Stable API, macOS FSEvents backend. |
| tokio::sync::broadcast | (tokio 1.x) | Multi-consumer event channel for SSE fan-out | Already in workspace. Broadcast semantics match SSE perfectly: each new subscriber gets events from subscription point forward. |
| tokio::sync::mpsc | (tokio 1.x) | Blocking thread -> async bridge for notify events | Already in workspace. `blocking_send` method designed for exactly this use case. |
| tokio-stream | 0.1.x | BroadcastStream wrapper for axum SSE integration | Wraps broadcast::Receiver as a futures::Stream, which axum::response::sse::Sse expects. |
| axum::response::sse | (axum 0.8.x) | SSE response type and Event builder | Already in workspace via axum 0.8. Native SSE support with keep-alive, event types, data fields. |

### Supporting
| Library | Version | Purpose | When to Use |
|---------|---------|---------|-------------|
| futures-util | 0.3.x | Stream combinators (.map, .filter_map) for SSE stream | Used when transforming BroadcastStream items into axum::response::sse::Event objects. |

### Alternatives Considered
| Instead of | Could Use | Tradeoff |
|------------|-----------|----------|
| notify 8.2.0 | notify 9.0.0-rc.1 | RC is too new (2026-01-25). 8.2.0 is battle-tested. Upgrade path when 9.0 stabilizes. |
| notify + custom debounce | notify-debouncer-full 0.7.0 | Debouncer-full provides rename tracking and smart event coalescing. But the spec needs exact 2-second per-file debounce, and JSONL files are append-only (no renames). Custom debounce is simpler, more testable, and exactly matches the requirement. |
| notify-debouncer-mini | notify + custom debounce | Mini provides per-file one-event-per-timeframe but lacks the fine control needed for error recovery and the exact 2-second contract. |
| tokio::sync::broadcast | tokio::sync::watch | Watch only sends latest value (not historical). SSE clients need every event, not just the latest state. |

**Installation:**
```toml
# Workspace Cargo.toml additions
notify = { version = "8.2", features = ["macos_fsevent"] }
tokio-stream = { version = "0.1", features = ["sync"] }
futures-util = "0.3"
```

## Architecture Patterns

### Recommended Module Structure
```
crates/
├── store/src/
│   └── sync.rs              # Existing -- sync_file used by watcher (no changes needed)
├── server/src/
│   ├── watcher.rs            # NEW: notify watcher + debounce + sync trigger loop
│   ├── events.rs             # NEW: SSE event types, broadcast sender, SSE handler
│   ├── serve.rs              # MODIFIED: spawn watcher task, inject broadcast sender
│   ├── state.rs              # MODIFIED: add broadcast::Sender<SseEvent> to AppState
│   └── api/
│       └── mod.rs            # MODIFIED: add GET /v1/events route
```

### Pattern 1: Blocking Watcher -> Async Bridge
**What:** The `notify` crate's `recommended_watcher` runs callbacks on an OS thread. The callback uses `tokio::sync::mpsc::Sender::blocking_send` to forward events to an async task.
**When to use:** Whenever bridging sync callbacks with tokio async code.
**Example:**
```rust
// Source: tokio docs on bridging sync code + notify crate API
use notify::{Event, RecursiveMode, Watcher};
use tokio::sync::mpsc;

pub fn spawn_watcher(
    projects_dir: PathBuf,
    tx: mpsc::Sender<notify::Result<Event>>,
) -> notify::Result<()> {
    // The watcher must be kept alive -- dropping it stops watching.
    // Move it into a thread that blocks forever.
    std::thread::spawn(move || {
        let mut watcher = notify::recommended_watcher(
            move |res: notify::Result<Event>| {
                // blocking_send is designed for use in non-async contexts.
                // If the channel is full or closed, the event is dropped.
                let _ = tx.blocking_send(res);
            }
        ).expect("failed to create watcher");

        watcher.watch(&projects_dir, RecursiveMode::Recursive)
            .expect("failed to watch projects directory");

        // Block this thread forever -- watcher dies if this function returns.
        std::thread::park();
    });
    Ok(())
}
```

### Pattern 2: Per-File Debounce with HashMap
**What:** Track last-synced time per file path. Skip processing if less than 2 seconds since last sync.
**When to use:** The spec requires "2-second minimum between syncs per file" (WATCH-02).
**Example:**
```rust
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Instant;

struct FileDebouncer {
    last_synced: HashMap<PathBuf, Instant>,
    debounce_duration: std::time::Duration,
}

impl FileDebouncer {
    fn new(debounce_secs: u64) -> Self {
        Self {
            last_synced: HashMap::new(),
            debounce_duration: std::time::Duration::from_secs(debounce_secs),
        }
    }

    /// Returns true if the file should be synced (debounce period elapsed).
    fn should_sync(&mut self, path: &PathBuf) -> bool {
        let now = Instant::now();
        if let Some(last) = self.last_synced.get(path) {
            if now.duration_since(*last) < self.debounce_duration {
                return false;
            }
        }
        self.last_synced.insert(path.clone(), now);
        true
    }
}
```

### Pattern 3: Broadcast -> SSE Fan-Out
**What:** A `tokio::sync::broadcast::Sender<SseEvent>` lives in AppState. The watcher loop sends events after each successful sync. Each SSE client handler subscribes a new Receiver and wraps it as a BroadcastStream.
**When to use:** Multiple concurrent SSE clients need to receive the same events.
**Example:**
```rust
// In events.rs
use axum::response::sse::{Event, KeepAlive, Sse};
use tokio::sync::broadcast;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::StreamExt;

#[derive(Clone, Debug, serde::Serialize)]
pub struct SseEvent {
    pub event_type: String,  // "record:added", "session:started", etc.
    pub data: serde_json::Value,
}

pub async fn events_handler(
    State(state): State<SharedState>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let rx = state.event_tx.subscribe();
    let stream = BroadcastStream::new(rx)
        .filter_map(|result| {
            // BroadcastStream yields Result<T, BroadcastStreamRecvError>
            // Lagged errors (slow consumer) are silently dropped.
            result.ok()
        })
        .map(|sse_event| {
            Ok(Event::default()
                .event(sse_event.event_type)
                .json_data(sse_event.data)
                .unwrap_or_else(|_| Event::default().data("serialization error")))
        });

    Sse::new(stream).keep_alive(KeepAlive::default())
}
```

### Pattern 4: Watcher Event Processing Loop
**What:** An async task that receives raw notify events, debounces per-file, triggers sync_file, inspects results, and emits SSE events via broadcast.
**When to use:** Central coordinator between file watching, sync engine, and SSE broadcast.
**Example:**
```rust
async fn watcher_loop(
    mut rx: mpsc::Receiver<notify::Result<notify::Event>>,
    conn: tokio_rusqlite::Connection,
    event_tx: broadcast::Sender<SseEvent>,
) {
    let mut debouncer = FileDebouncer::new(2);

    while let Some(result) = rx.recv().await {
        let event = match result {
            Ok(e) => e,
            Err(e) => {
                tracing::warn!("File watcher error: {}", e);
                continue; // Graceful recovery from transient errors
            }
        };

        // Filter to only .jsonl file modifications
        for path in &event.paths {
            if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            if !debouncer.should_sync(&path.to_path_buf()) {
                continue;
            }

            let session_id = match sync::extract_session_id(path) {
                Some(id) => id,
                None => continue,
            };

            // Detect if this is a new session (not yet in sync_metadata)
            let is_new_session = /* query sync_metadata for this file */;

            match sync::sync_file(&conn, path, &session_id).await {
                Ok(result) => {
                    if result.records_synced > 0 {
                        // Emit record:added events
                        let _ = event_tx.send(SseEvent {
                            event_type: "record:added".into(),
                            data: json!({
                                "session_id": session_id,
                                "records_synced": result.records_synced,
                                "file_path": result.file_path,
                            }),
                        });

                        // Emit session:started if new session
                        if is_new_session {
                            let _ = event_tx.send(SseEvent {
                                event_type: "session:started".into(),
                                data: json!({ "session_id": session_id }),
                            });
                        }

                        // Emit schema:drift if new overflow fields detected
                        if result.overflow_fields_logged > 0 {
                            let _ = event_tx.send(SseEvent {
                                event_type: "schema:drift".into(),
                                data: json!({
                                    "new_fields": result.overflow_fields_logged,
                                    "session_id": session_id,
                                }),
                            });
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!(file = %path.display(), error = %e, "Live sync failed");
                }
            }
        }
    }
}
```

### Pattern 5: Version Change Detection
**What:** Compare the Claude Code version from newly ingested records against the last known version. Emit `version:changed` SSE event when a new version appears.
**When to use:** SSE-05 requires version change detection.
**Example:**
```rust
// After sync_file succeeds, check if version changed
// The simplest approach: query the sessions table for the latest version
// and compare to a cached "last known version" in the watcher state.

struct WatcherState {
    debouncer: FileDebouncer,
    last_known_version: Option<String>,
}

// After syncing a file with records_synced > 0:
fn check_version_change(
    state: &mut WatcherState,
    conn: &Connection,
    session_id: &str,
    event_tx: &broadcast::Sender<SseEvent>,
) {
    if let Ok(version) = conn.query_row(
        "SELECT version FROM sessions WHERE session_id = ?1",
        [session_id],
        |row| row.get::<_, String>(0),
    ) {
        if state.last_known_version.as_ref() != Some(&version) {
            let old = state.last_known_version.replace(version.clone());
            let _ = event_tx.send(SseEvent {
                event_type: "version:changed".into(),
                data: json!({
                    "old_version": old,
                    "new_version": version,
                    "session_id": session_id,
                }),
            });
        }
    }
}
```

### Anti-Patterns to Avoid
- **Polling instead of watching:** Do not implement a loop that periodically calls `sync_all`. The `notify` crate provides native OS-level notifications that are far more efficient and lower-latency.
- **Blocking the tokio runtime:** The `notify` watcher callback MUST NOT run async code or block on futures. Use `blocking_send` only.
- **Holding the watcher in an async task:** The `notify::RecommendedWatcher` blocks its containing thread. It must live on a dedicated blocking thread (via `std::thread::spawn`), not on a tokio task (not even `spawn_blocking`, because the watcher needs to live indefinitely).
- **Full FTS rebuild on every file change:** The current `sync_all` rebuilds FTS after sync. For live ingestion, FTS rebuild must be deferred (e.g., periodic, or only when explicitly triggered) -- rebuilding on every file change would be extremely expensive.
- **Ignoring broadcast channel capacity:** `broadcast::channel(capacity)` will drop oldest events for slow consumers. Size the channel generously (e.g., 1024) and handle `RecvError::Lagged` in the SSE stream.

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| Filesystem event notification | Custom poll loop with stat() calls | `notify` crate | OS-native inotify/FSEvents/kqueue integration; handles symlinks, recursive watches, platform differences |
| SSE HTTP protocol | Manual chunked-transfer encoding | `axum::response::sse` | Correct SSE framing (data:/event:/id: prefixes), keep-alive, connection management |
| Broadcast fan-out to multiple consumers | Custom Vec<Sender> with manual iteration | `tokio::sync::broadcast` | Handles subscriber lifecycle, backpressure, lagged consumers automatically |
| Stream adaptation for SSE | Manual async fn that loops and yields | `tokio_stream::wrappers::BroadcastStream` | Properly implements the `Stream` trait that axum's `Sse` type requires |

**Key insight:** The entire event pipeline from filesystem notification to SSE client has mature library support at every stage. The custom code is the debounce logic (simple HashMap), the sync trigger (calls existing `sync_file`), and the event emission logic (determining which SSE events to emit based on sync results).

## Common Pitfalls

### Pitfall 1: Watcher Thread Lifetime
**What goes wrong:** The `notify::RecommendedWatcher` is dropped when the function that created it returns, silently stopping all file watching with no error.
**Why it happens:** The watcher must be kept alive for the entire daemon lifetime. Moving it into a closure or short-lived scope causes it to be dropped.
**How to avoid:** Spawn a dedicated `std::thread` that creates the watcher and then blocks forever (via `std::thread::park()` or a channel recv). The watcher lives as long as the thread.
**Warning signs:** File changes stop being detected after the watcher setup function returns, but no errors appear in logs.

### Pitfall 2: FTS Rebuild Overhead on Every File Change
**What goes wrong:** The current `sync_all` does an FTS rebuild after syncing. If the watcher triggers FTS rebuild for every single file change, performance degrades catastrophically on large databases.
**Why it happens:** The FTS5 external-content rebuild re-indexes ALL message_content rows, not just new ones. On a 100K+ row database, this takes seconds.
**How to avoid:** Do NOT call `rebuild_fts_index` in the per-file watcher path. Instead, implement a deferred FTS rebuild strategy: either on a periodic timer (e.g., every 30 seconds if new data was ingested), or skip it entirely during live ingestion and rely on the next manual `sync` command to rebuild. Document that FTS search results may lag a few seconds behind live ingestion.
**Warning signs:** CPU spikes every time Claude Code writes a message, search latency increases under active use.

### Pitfall 3: Broadcast Channel Capacity and Lagged Consumers
**What goes wrong:** When a slow SSE client cannot keep up with event production, the broadcast channel drops events for that consumer and returns a `RecvError::Lagged(n)` error.
**Why it happens:** `tokio::sync::broadcast` is bounded. When the buffer wraps around, slow consumers lose events.
**How to avoid:** Size the broadcast channel generously (1024+ slots). In the SSE stream, silently drop `Lagged` errors via `filter_map` on the BroadcastStream -- the SSE protocol has no delivery guarantee, and clients can re-sync via the regular API if needed.
**Warning signs:** SSE clients see gaps in event delivery, or the SSE stream terminates unexpectedly.

### Pitfall 4: macOS FSEvents Coalescing
**What goes wrong:** macOS FSEvents may batch and coalesce multiple filesystem events into a single notification, potentially missing intermediate states.
**Why it happens:** FSEvents is designed for efficiency, not real-time granularity. Multiple rapid writes may be reported as a single modification event.
**How to avoid:** This is actually fine for our use case. We only need to know "this file changed" to trigger `sync_file`, which reads from the last byte offset. We do not need to know about every individual write. The debounce logic further reduces sensitivity to FSEvents coalescing.
**Warning signs:** None expected -- this behavior is compatible with our sync architecture.

### Pitfall 5: Stale HashMap Entries in Debouncer
**What goes wrong:** The debouncer HashMap grows unbounded as new session files are created over time, causing slow memory growth.
**Why it happens:** Files are added to the HashMap when first seen but never removed.
**How to avoid:** Periodically prune entries older than N minutes (e.g., 10 minutes) from the HashMap. Claude Code sessions that are inactive for >10 minutes are unlikely to write rapidly. A simple sweep during the event loop is sufficient.
**Warning signs:** Memory usage of the daemon slowly increases over weeks of continuous operation.

### Pitfall 6: Watcher Error Recovery
**What goes wrong:** Transient filesystem errors (unmounted volumes, permission changes, etc.) cause the notify watcher to emit errors. If not handled, these could crash the daemon or stall the event loop.
**Why it happens:** The spec requires "recovers gracefully from transient filesystem errors" (success criterion 4).
**How to avoid:** In the watcher loop, log errors at warn level and continue processing. The notify crate's `recommended_watcher` is resilient to individual file errors -- the watch continues for other files. For catastrophic errors (the watcher itself dies), log at error level and optionally attempt to restart the watcher after a backoff delay.
**Warning signs:** Repeated error logs in the watcher loop, but the daemon should continue operating.

## Code Examples

### Complete SSE Event Type Definitions
```rust
// Source: spec requirements SSE-01 through SSE-05
use serde::{Deserialize, Serialize};

/// Event types emitted through the SSE broadcast channel.
/// Each variant corresponds to a spec requirement.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "event_type", content = "data")]
pub enum SseEvent {
    /// SSE-02: New record(s) ingested from a JSONL file.
    #[serde(rename = "record:added")]
    RecordAdded {
        session_id: String,
        records_synced: usize,
        file_path: String,
    },

    /// SSE-03: New session detected (first record from a previously unseen session).
    #[serde(rename = "session:started")]
    SessionStarted {
        session_id: String,
    },

    /// SSE-04: New overflow fields detected during decomposition.
    #[serde(rename = "schema:drift")]
    SchemaDrift {
        new_fields: usize,
        session_id: String,
    },

    /// SSE-05: Claude Code version changed between records.
    #[serde(rename = "version:changed")]
    VersionChanged {
        old_version: Option<String>,
        new_version: String,
        session_id: String,
    },
}
```

### AppState Extension
```rust
// Modified state.rs
use tokio::sync::broadcast;

pub struct AppState {
    pub conn: tokio_rusqlite::Connection,
    pub version: String,
    pub db_path: PathBuf,
    /// Broadcast sender for SSE events. Clone and call .subscribe()
    /// in each SSE handler to get a new Receiver.
    pub event_tx: broadcast::Sender<SseEvent>,
}
```

### Watcher Integration in serve.rs
```rust
// Modified serve.rs -- additions to run_server
pub async fn run_server(
    state: SharedState,
    port: u16,
    socket_path: PathBuf,
    projects_dir: PathBuf,  // NEW parameter
) -> anyhow::Result<()> {
    let app = api::build_router(state.clone());
    let token = CancellationToken::new();

    // --- Spawn file watcher ---
    let (watch_tx, watch_rx) = tokio::sync::mpsc::channel(256);
    let watcher_projects = projects_dir.clone();
    watcher::spawn_watcher(watcher_projects, watch_tx)?;

    // --- Spawn watcher processing loop ---
    let watcher_conn = state.conn.clone(); // tokio-rusqlite Connection is Clone
    let watcher_event_tx = state.event_tx.clone();
    let watcher_token = token.clone();
    tokio::spawn(async move {
        watcher::watcher_loop(
            watch_rx,
            watcher_conn,
            watcher_event_tx,
            watcher_token,
        ).await;
    });

    // ... rest of TCP/UDS listener setup unchanged ...
}
```

### Detecting New Sessions
```rust
// Query to check if a session already exists before syncing.
// If the session does NOT exist in sync_metadata, it's new.
async fn is_new_file(
    conn: &tokio_rusqlite::Connection,
    file_path: &str,
) -> bool {
    let fp = file_path.to_string();
    conn.call(move |conn| {
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sync_metadata WHERE file_path = ?1",
                [&fp],
                |row| row.get(0),
            )
            .unwrap_or(0);
        Ok(count == 0)
    })
    .await
    .unwrap_or(true)
}
```

## State of the Art

| Old Approach | Current Approach | When Changed | Impact |
|--------------|------------------|--------------|--------|
| notify 4.x with debounce built-in | notify 8.x + separate debouncer crates | notify 5.0 (2022) | Debouncing moved to opt-in crates. Custom debounce is now the norm for specific timing requirements. |
| Custom SSE implementation | axum::response::sse built-in | axum 0.6 (2022) | No need for tower middleware or manual chunked encoding. SSE is first-class in axum. |
| std::sync::mpsc for watcher bridge | tokio::sync::mpsc with blocking_send | tokio 1.0 (2021) | blocking_send designed specifically for sync->async bridging. Avoids runtime panics. |

**Deprecated/outdated:**
- notify 4.x debounce mode: removed in notify 5.0. Do not use.
- `warp::sse`: This project uses axum, not warp. axum's SSE is more ergonomic.
- `actix-web::HttpResponse::streaming`: Not applicable; we use axum.

## Open Questions

1. **FTS rebuild strategy during live ingestion**
   - What we know: FTS5 external-content rebuild is O(total_rows), not O(new_rows). Calling it per-file is unacceptable.
   - What's unclear: Whether FTS search must be real-time during live ingestion, or if a few seconds of lag is acceptable.
   - Recommendation: Implement a timer-based rebuild (every 30 seconds if new data was ingested). Accept that FTS search may lag behind live API queries by up to 30 seconds. This is a reasonable tradeoff. The `record:added` SSE event gives consumers immediate notification; they can use the non-FTS API endpoints for real-time data.

2. **Broadcast channel capacity sizing**
   - What we know: Claude Code writes messages at human interaction speed (seconds between writes). Even with multiple concurrent sessions, event rates are likely <10/second.
   - What's unclear: Whether there are burst scenarios (e.g., automated session replay) that could overwhelm the broadcast channel.
   - Recommendation: Start with capacity 1024. This provides ~100 seconds of buffer at 10 events/second. Log when consumers lag.

3. **Should watcher_loop live in the store crate or server crate?**
   - What we know: The watcher uses `sync_file` from store but emits SSE events which are a server concern. The debounce logic is purely watcher-specific.
   - What's unclear: Whether a clean separation is possible without circular dependencies.
   - Recommendation: Place watcher.rs and events.rs in the server crate. The server crate already depends on the store crate, so calling `sync_file` is natural. The SseEvent types are server-layer concerns.

4. **tokio-rusqlite Connection cloneability**
   - What we know: `tokio_rusqlite::Connection` implements `Clone` -- it's internally `Arc<Mutex<Connection>>`. The watcher loop needs a connection handle.
   - What's unclear: Whether heavy concurrent use (watcher writing + API reading) will cause lock contention.
   - Recommendation: This should be fine. WAL mode allows concurrent reads during writes, and tokio-rusqlite serializes access through the blocking thread pool. If contention becomes an issue, the existing architecture already separates write and read connections conceptually.

<phase_requirements>
## Phase Requirements

| ID | Description | Research Support |
|----|-------------|-----------------|
| WATCH-01 | notify crate watching ~/.claude/projects/ recursively for .jsonl changes | Use `notify::recommended_watcher` with `RecursiveMode::Recursive` on a blocking thread. Filter events to `.jsonl` extension in the processing loop. Standard pattern with HIGH confidence -- notify 8.2.0 is production-grade. |
| WATCH-02 | Debounced event processing (2-second minimum between syncs per file) | Custom `FileDebouncer` struct using `HashMap<PathBuf, Instant>` with 2-second threshold. Simpler and more testable than `notify-debouncer-full` for this specific per-file timing requirement. |
| WATCH-03 | notify watcher in blocking thread, events forwarded via tokio channel | `std::thread::spawn` + `tokio::sync::mpsc::Sender::blocking_send`. The watcher callback uses `blocking_send` to bridge to the async event loop. The thread parks indefinitely to keep the watcher alive. |
| SSE-01 | GET /v1/events -- SSE stream endpoint | Add `events_handler` to server crate using `axum::response::sse::Sse` backed by `BroadcastStream`. Register at `/v1/events` in `build_router`. Keep-alive enabled via `KeepAlive::default()`. |
| SSE-02 | record:added event when new record ingested | After `sync_file` returns with `records_synced > 0`, emit `SseEvent::RecordAdded` through the broadcast channel with session_id, records_synced, and file_path. |
| SSE-03 | session:started event when new session detected | Before calling `sync_file`, check `sync_metadata` for the file path. If no entry exists, this is a new session file. After successful sync, emit `SseEvent::SessionStarted`. |
| SSE-04 | schema:drift event when new overflow fields detected | After `sync_file` returns with `overflow_fields_logged > 0`, emit `SseEvent::SchemaDrift` through the broadcast channel. The existing drift logging in the decomposer already captures the fields; we just need to surface the count as an event. |
| SSE-05 | version:changed event when Claude Code version changes | Track `last_known_version` in watcher state. After syncing, query the session's version from the sessions table. If different from last_known_version, emit `SseEvent::VersionChanged`. |
</phase_requirements>

## Sources

### Primary (HIGH confidence)
- [notify 8.2.0 API docs](https://docs.rs/notify/8.2.0/notify/) - Watcher trait, Event/EventKind types, RecursiveMode, recommended_watcher
- [axum SSE module docs](https://docs.rs/axum/latest/axum/response/sse/) - Sse, Event, KeepAlive types and methods
- [tokio::sync::broadcast docs](https://docs.rs/tokio/latest/tokio/sync/broadcast/index.html) - Broadcast channel semantics, capacity, lagged errors
- [tokio::sync::mpsc docs](https://docs.rs/tokio/latest/tokio/sync/mpsc/index.html) - blocking_send method for sync->async bridge
- [BroadcastStream docs](https://docs.rs/tokio-stream/latest/tokio_stream/wrappers/struct.BroadcastStream.html) - Stream wrapper for broadcast::Receiver
- [axum SSE example (GitHub)](https://github.com/tokio-rs/axum/blob/main/examples/sse/src/main.rs) - Official SSE handler pattern

### Secondary (MEDIUM confidence)
- [axum SSE + broadcast channel discussion](https://github.com/tokio-rs/axum/discussions/1670) - Community pattern for broadcast -> SSE
- [axum-sse-from-channel example](https://github.com/mouton0815/axum-sse-from-channel) - Working example of broadcast channel -> SSE in axum
- [notify-debouncer-full docs](https://docs.rs/notify-debouncer-full/latest/notify_debouncer_full/) - Evaluated but not recommended
- [Tokio bridging sync code guide](https://tokio.rs/tokio/topics/bridging) - blocking_send pattern

### Tertiary (LOW confidence)
- None. All findings verified against official documentation.

## Metadata

**Confidence breakdown:**
- Standard stack: HIGH - All libraries are well-documented, in active maintenance, and already partially in the workspace dependency tree. notify 8.2.0 has 62M+ downloads.
- Architecture: HIGH - The blocking thread -> mpsc -> broadcast -> SSE pipeline is a well-established pattern documented in official tokio and axum resources. The existing sync_file function fits naturally as the sync trigger.
- Pitfalls: HIGH - FTS rebuild overhead and watcher lifetime are well-understood issues documented in the crate ecosystem. Debounce semantics are straightforward to implement and test.

**Research date:** 2026-02-20
**Valid until:** 2026-04-20 (90 days -- stable ecosystem, no major changes expected)
