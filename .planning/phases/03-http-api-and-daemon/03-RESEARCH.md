# Phase 3: HTTP API and Daemon - Research

**Researched:** 2026-02-20
**Domain:** Rust HTTP API (axum), Unix domain socket dual-serving, daemon lifecycle, graceful shutdown
**Confidence:** HIGH

<spec_constraints>
## Spec Constraints (IMMUTABLE -- from ROADMAP.md Success Criteria)

1. `curl http://localhost:7424/v1/health` returns status, db_size, record_count, and version after running `claude-history serve`
2. GET endpoints for sessions, messages, search, analytics, and schema return correct JSON responses matching the data visible through the CLI
3. POST /v1/messages/query accepts a structured query body and returns filtered results with parameterized SQL compilation (no injection)
4. The same API is accessible over the Unix domain socket at /tmp/claude-history.sock (or configured path), and CLI commands automatically connect to the daemon socket when available
5. `claude-history serve` runs as a foreground daemon with graceful shutdown on SIGTERM/SIGINT -- in-flight requests complete, no data loss

These are non-negotiable user-story outcomes. Research recommendations must not narrow below these.
</spec_constraints>

<phase_requirements>
## Phase Requirements

| ID | Description | Research Support |
|----|-------------|-----------------|
| API-01 | GET /v1/health -- status, db_size, record_count, version | axum GET handler returning Json<HealthResponse>; db_size from SQLite `page_count * page_size` pragma; record_count from `SELECT COUNT(*) FROM messages` |
| API-02 | GET /v1/sessions -- list with filters (status, project, after, before, limit) | Wraps existing `store::query::list_sessions`; axum Query extractor for filter params |
| API-03 | GET /v1/sessions/:id -- single session detail | New store function `get_session(conn, id)` querying sessions table; axum Path extractor |
| API-04 | GET /v1/sessions/:id/conversation -- ordered messages with optional thinking/tool_io | New store function returning messages + content blocks ordered by timestamp; Query params for `include_thinking`, `include_tool_io` |
| API-05 | GET /v1/sessions/:id/tree -- conversation tree with sidechain structure | New store function building tree from parent_uuid + is_sidechain columns; returns nested JSON |
| API-06 | GET /v1/sessions/:id/agents -- agent hierarchy for session | Query agents table filtered by session_id; return agent_id, first_seen, last_seen |
| API-07 | GET /v1/sessions/:id/summary -- session summary (token totals, tool counts, duration) | Combine existing `token_stats_by_session` + `tool_frequency` + timestamp delta; single JSON response |
| API-08 | POST /v1/messages/query -- flexible query body compiled to parameterized SQL | Serde struct for query body; compile to parameterized SQL using existing `query_messages` pattern with Box<dyn ToSql> |
| API-09 | GET /v1/messages/:uuid -- single message by UUID | New store function `get_message(conn, uuid)` with content blocks and token usage |
| API-10 | GET /v1/search?q= -- FTS5 search across all content | Wraps existing `store::fts::search_messages`; Query extractor for `q`, `limit`, `offset` |
| API-11 | GET /v1/analytics/tokens -- token analysis with grouping | Wraps existing `token_stats_by_model`; Query param for `group_by` (session, day, model) |
| API-12 | GET /v1/analytics/tools -- tool frequency and error rates | Wraps existing `store::query::tool_frequency` |
| API-13 | GET /v1/analytics/models -- model usage breakdown | Wraps existing `store::query::model_breakdown` |
| API-14 | GET /v1/export/:session_id -- streamed export (json, markdown, csv) | Wraps existing export functions; Query param for `format`; streaming response body |
| API-15 | GET /v1/schema/versions -- tracked Claude Code versions | Wraps existing `store::query::version_history` |
| API-16 | GET /v1/schema/drift -- detected schema drift events | Wraps existing `store::query::schema_drift_list` with optional `record_type` filter |
| UDS-01 | Same HTTP API over Unix domain socket at $CLAUDE_HISTORY_SOCKET or /tmp/claude-history.sock | axum natively supports `UnixListener` via its `Listener` trait; spawn second `axum::serve` on UnixListener |
| UDS-02 | Lower-latency alternative for local consumers | Same router, different listener; no additional code for latency benefit |
| INFRA-04 | Default HTTP port: 7424 | `TcpListener::bind("127.0.0.1:7424")` |
| INFRA-05 | Daemon mode as foreground process | `claude-history serve` blocks on `axum::serve(...).await`; no daemonization logic; launchd/systemd manages backgrounding |
| INFRA-06 | Graceful shutdown with tokio CancellationToken | `tokio_util::sync::CancellationToken` + `axum::serve(...).with_graceful_shutdown(token.cancelled())` |
| CLI-01 | claude-history serve -- start daemon | New `Serve` variant in Commands enum with `--port`, `--socket` options |
| CLI-15 | CLI connects to daemon socket if available, otherwise opens DB read-only | Check socket existence at startup; if present, HTTP client over UDS; otherwise current direct DB path |
</phase_requirements>

## Summary

Phase 3 adds an HTTP API layer on top of the existing store crate using axum 0.8, the standard Rust web framework built on tokio and tower. The existing store layer (query.rs, fts.rs, export.rs) already implements the data access patterns for most endpoints -- the API layer primarily wraps these functions in axum handlers with appropriate extractors and JSON serialization.

The architecture is straightforward: a shared `AppState` struct holding a `tokio_rusqlite::Connection` (wrapped in `Arc`) is injected into all handlers via axum's `State` extractor. Each handler calls into the store layer via `conn.call(|conn| ...)` exactly as the CLI does today. The existing store functions take `&rusqlite::Connection` and return `Result<Vec<T>, rusqlite::Error>` where T is `Serialize` -- these return values become `Json<T>` responses directly.

The dual-listener requirement (TCP + Unix domain socket) is well-supported in axum 0.8: both `TcpListener` and `UnixListener` implement the `Listener` trait natively, so `axum::serve` works with either. Two `axum::serve` calls are spawned concurrently via `tokio::spawn`, sharing the same `Router`. Graceful shutdown uses `tokio_util::sync::CancellationToken` with `axum::serve(...).with_graceful_shutdown(token.cancelled())`, which is the pattern specified by INFRA-06 and demonstrated in axum's official examples.

**Primary recommendation:** Add axum 0.8, tower-http 0.6 (trace feature), and tokio-util 0.7 to workspace dependencies. Structure the server crate with `api/` module containing route handlers grouped by resource, a `state.rs` module for AppState, and `serve.rs` for the dual-listener startup logic. Extend the existing CLI `Commands` enum with a `Serve` variant. Approximately 6-8 new store query functions are needed for endpoints that do not map to existing CLI functions (session detail, conversation, tree, agents, single message).

## Standard Stack

### Core

| Library | Version | Purpose | Why Standard |
|---------|---------|---------|--------------|
| axum | 0.8.8 | HTTP routing and request handling | Official tokio-rs web framework; native tower integration; Listener trait supports both TcpListener and UnixListener |
| tower-http | 0.6.8 | HTTP middleware (tracing, request-id) | Canonical tower middleware collection; TraceLayer for request logging |
| tokio-util | 0.7.18 | CancellationToken and TaskTracker | Part of tokio ecosystem; CancellationToken is the spec-mandated shutdown mechanism (INFRA-06) |
| serde | 1.0 (workspace) | JSON request/response serialization | Already in workspace; all store types derive Serialize |
| serde_json | 1.0 (workspace) | JSON encoding | Already in workspace |

### Supporting

| Library | Version | Purpose | When to Use |
|---------|---------|---------|-------------|
| hyper-util | (transitive via axum) | HTTP protocol utilities | Not needed as direct dependency -- axum 0.8 handles UDS natively |
| uuid | 1.x | UUID validation for path parameters | Validate :uuid and :session_id path params before querying store |

### Alternatives Considered

| Instead of | Could Use | Tradeoff |
|------------|-----------|----------|
| axum 0.8 | actix-web 4 | actix has its own runtime model; axum integrates naturally with existing tokio + tower stack |
| axum 0.8 | poem | poem is lighter but less ecosystem support; axum has canonical tokio-rs backing |
| tower-http TraceLayer | custom tracing middleware | TraceLayer provides structured request/response logging out of the box; no reason to hand-roll |
| tokio_util::sync::CancellationToken | tokio::sync::Notify | CancellationToken is spec-mandated (INFRA-06); also more ergonomic for multi-task fan-out |
| direct axum::serve for UDS | tokio-listener crate | axum 0.8 natively supports UnixListener; no extra crate needed |

**Installation (additions to workspace Cargo.toml):**
```toml
[workspace.dependencies]
axum = { version = "0.8", features = ["tokio"] }
tower-http = { version = "0.6", features = ["trace", "request-id"] }
tokio-util = { version = "0.7", features = ["rt"] }
uuid = { version = "1", features = ["v4"] }
```

And in `crates/server/Cargo.toml`:
```toml
axum = { workspace = true }
tower-http = { workspace = true }
tokio-util = { workspace = true }
uuid = { workspace = true }
```

## Architecture Patterns

### Recommended Project Structure
```
crates/server/src/
  main.rs              # CLI entry point (existing) -- add Serve command
  export.rs            # existing export logic
  output.rs            # existing CLI output formatting
  serve.rs             # NEW: dual-listener startup, graceful shutdown
  state.rs             # NEW: AppState struct, db_size helper
  api/
    mod.rs             # NEW: router construction (all routes)
    health.rs          # NEW: GET /v1/health
    sessions.rs        # NEW: GET /v1/sessions, /v1/sessions/:id, /v1/sessions/:id/*
    messages.rs        # NEW: POST /v1/messages/query, GET /v1/messages/:uuid
    search.rs          # NEW: GET /v1/search
    analytics.rs       # NEW: GET /v1/analytics/tokens, tools, models
    export_api.rs      # NEW: GET /v1/export/:session_id
    schema.rs          # NEW: GET /v1/schema/versions, drift
    error.rs           # NEW: ApiError enum implementing IntoResponse
```

### Pattern 1: Shared AppState via Arc
**What:** A struct holding the `tokio_rusqlite::Connection` and metadata, wrapped in `Arc`, passed to all handlers via `State` extractor.
**When to use:** Every handler needs database access.
**Example:**
```rust
// Source: axum State extractor documentation
use std::sync::Arc;
use axum::extract::State;

struct AppState {
    conn: tokio_rusqlite::Connection,
    version: String,      // crate version for /v1/health
    db_path: std::path::PathBuf,
}

type SharedState = Arc<AppState>;

async fn health(State(state): State<SharedState>) -> Json<HealthResponse> {
    let (db_size, record_count) = state.conn.call(|conn| {
        let page_count: i64 = conn.pragma_query_value(None, "page_count", |r| r.get(0))?;
        let page_size: i64 = conn.pragma_query_value(None, "page_size", |r| r.get(0))?;
        let count: i64 = conn.query_row("SELECT COUNT(*) FROM messages", [], |r| r.get(0))?;
        Ok((page_count * page_size, count))
    }).await.unwrap_or((0, 0));

    Json(HealthResponse {
        status: "ok".to_string(),
        db_size,
        record_count,
        version: state.version.clone(),
    })
}

let app = Router::new()
    .route("/v1/health", get(health))
    .with_state(Arc::new(app_state));
```

### Pattern 2: Dual-Listener Startup with Graceful Shutdown
**What:** Two `axum::serve` instances sharing the same Router, one on TCP and one on Unix domain socket, both wired to the same CancellationToken.
**When to use:** The serve command startup.
**Example:**
```rust
// Source: axum graceful-shutdown example + unix-domain-socket example
use tokio::net::{TcpListener, UnixListener};
use tokio_util::sync::CancellationToken;

let token = CancellationToken::new();
let app = build_router(state);

// TCP listener
let tcp = TcpListener::bind(("127.0.0.1", port)).await?;
let tcp_token = token.clone();
let tcp_app = app.clone();
let tcp_handle = tokio::spawn(async move {
    axum::serve(tcp, tcp_app)
        .with_graceful_shutdown(tcp_token.cancelled())
        .await
});

// Unix domain socket listener
let _ = std::fs::remove_file(&socket_path); // clean stale socket
let uds = UnixListener::bind(&socket_path)?;
let uds_token = token.clone();
let uds_app = app.clone();
let uds_handle = tokio::spawn(async move {
    axum::serve(uds, uds_app)
        .with_graceful_shutdown(uds_token.cancelled())
        .await
});

// Wait for shutdown signal
shutdown_signal().await;
token.cancel();

// Wait for both servers to drain
let _ = tcp_handle.await;
let _ = uds_handle.await;
// Clean up socket file
let _ = std::fs::remove_file(&socket_path);
```

### Pattern 3: Handler Calling Store Layer
**What:** Each HTTP handler calls into the existing store functions via `conn.call(|conn| ...)`, exactly as CLI handlers do.
**When to use:** Every data-fetching handler.
**Example:**
```rust
// Source: existing CLI pattern in main.rs, adapted for axum
use axum::extract::{Path, Query, State};

#[derive(Deserialize)]
struct SessionsParams {
    project: Option<String>,
    after: Option<String>,
    before: Option<String>,
    limit: Option<usize>,
}

async fn list_sessions(
    State(state): State<SharedState>,
    Query(params): Query<SessionsParams>,
) -> Result<Json<Vec<SessionSummary>>, ApiError> {
    let limit = params.limit.unwrap_or(50);
    let results = state.conn.call(move |conn| {
        claude_history_store::query::list_sessions(
            conn,
            params.project.as_deref(),
            params.after.as_deref(),
            params.before.as_deref(),
            limit,
        )
    }).await.map_err(ApiError::from)?;

    Ok(Json(results))
}
```

### Pattern 4: POST Query Body to Parameterized SQL
**What:** Deserialize a JSON body into a struct, then compile it into a dynamic WHERE clause with `Box<dyn ToSql>` parameters -- exactly matching the existing `query_messages` pattern.
**When to use:** POST /v1/messages/query (API-08).
**Example:**
```rust
#[derive(Deserialize)]
struct MessageQuery {
    session_id: Option<String>,
    message_type: Option<String>,
    model: Option<String>,
    tool: Option<String>,
    after: Option<String>,
    before: Option<String>,
    limit: Option<usize>,
}

async fn query_messages(
    State(state): State<SharedState>,
    Json(body): Json<MessageQuery>,
) -> Result<Json<Vec<MessageResult>>, ApiError> {
    let limit = body.limit.unwrap_or(100);
    let results = state.conn.call(move |conn| {
        claude_history_store::query::query_messages(
            conn,
            body.session_id.as_deref(),
            body.message_type.as_deref(),
            body.model.as_deref(),
            body.tool.as_deref(),
            body.after.as_deref(),
            body.before.as_deref(),
            limit,
        )
    }).await.map_err(ApiError::from)?;

    Ok(Json(results))
}
```

### Pattern 5: Unified Error Type with IntoResponse
**What:** A single `ApiError` enum implementing `IntoResponse` that maps store errors, tokio-rusqlite errors, and validation errors to appropriate HTTP status codes + JSON error body.
**When to use:** All handler return types should be `Result<Json<T>, ApiError>`.
**Example:**
```rust
use axum::response::{IntoResponse, Response};
use axum::http::StatusCode;

enum ApiError {
    NotFound(String),
    BadRequest(String),
    Internal(String),
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            ApiError::NotFound(msg) => (StatusCode::NOT_FOUND, msg),
            ApiError::BadRequest(msg) => (StatusCode::BAD_REQUEST, msg),
            ApiError::Internal(msg) => (StatusCode::INTERNAL_SERVER_ERROR, msg),
        };
        let body = serde_json::json!({ "error": message });
        (status, Json(body)).into_response()
    }
}
```

### Anti-Patterns to Avoid
- **Opening a new DB connection per request:** tokio-rusqlite wraps a single SQLite connection with async dispatch. Use the shared connection from AppState. SQLite is single-writer anyway; the WAL mode already allows concurrent reads.
- **Blocking the tokio runtime with synchronous SQLite calls:** Always use `conn.call(|conn| ...)` to run synchronous rusqlite operations on the dedicated DB thread. Never call `conn.execute()` directly from an async handler.
- **Returning raw rusqlite errors to clients:** Map all internal errors to ApiError with sanitized messages. Never expose SQL queries or internal paths in error responses.
- **Spawning the UDS listener without cleaning up the socket file:** Always `std::fs::remove_file` the socket path before binding and after shutdown. A stale socket file from a crashed process will prevent binding.

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| HTTP routing and extraction | Custom request parser | axum Router + extractors | Battle-tested, type-safe, compile-time checked |
| Request/response logging | Custom logging middleware | tower-http TraceLayer | Structured tracing integration with span-per-request |
| Graceful shutdown coordination | Custom channel-based shutdown | CancellationToken + with_graceful_shutdown | Spec-mandated (INFRA-06); axum has first-class support |
| JSON error responses | Ad-hoc status code + string | ApiError enum implementing IntoResponse | Consistent error format across all endpoints |
| Unix domain socket serving | Manual hyper server with UDS | axum::serve with UnixListener | axum 0.8 Listener trait natively supports UnixListener |
| Query string parsing | Manual URL parsing | axum Query<T> extractor with serde | Handles Option, defaults, validation automatically |
| Signal handling (SIGTERM/SIGINT) | Raw libc signal handlers | tokio::signal::ctrl_c + signal(SignalKind::terminate) | Async-safe, integrates with tokio runtime |

**Key insight:** axum 0.8's Listener trait unification means TCP and UDS serving use identical code paths. The only difference is which listener type you bind. This was not always the case -- earlier axum versions required hyper-util boilerplate for UDS.

## Common Pitfalls

### Pitfall 1: Stale Unix Socket File
**What goes wrong:** If the daemon crashes or is killed with SIGKILL, the socket file at `/tmp/claude-history.sock` persists. On next startup, `UnixListener::bind` fails with "address already in use."
**Why it happens:** Unix domain sockets are filesystem entries that are not automatically cleaned up by the OS on process exit.
**How to avoid:** Always call `std::fs::remove_file(&socket_path)` before binding. This is safe -- if another daemon is actually running, the CLI should detect it before starting a new one (or warn the user).
**Warning signs:** "Address already in use" errors on startup.

### Pitfall 2: Forgetting to Clone Router for Second Listener
**What goes wrong:** The Router is moved into the first `axum::serve` call, making it unavailable for the second listener.
**Why it happens:** Rust ownership semantics. `axum::serve` takes ownership of the service.
**How to avoid:** Call `app.clone()` before passing to each serve call. axum Router is Clone.
**Warning signs:** Compile error about moved value.

### Pitfall 3: tokio-rusqlite Error Mapping
**What goes wrong:** `conn.call()` returns `Result<T, tokio_rusqlite::Error>`, not `Result<T, rusqlite::Error>`. The error variants include `ConnectionClosed` and `Close` in addition to the wrapped `rusqlite::Error`.
**Why it happens:** tokio-rusqlite wraps the underlying errors in its own enum.
**How to avoid:** Implement `From<tokio_rusqlite::Error>` for ApiError, matching on the variant to extract the inner error or return a generic internal error for connection failures.
**Warning signs:** Type mismatch errors when using `?` operator in handlers.

### Pitfall 4: Blocking the DB Thread with Large Exports
**What goes wrong:** The export endpoint (API-14) loads an entire session into memory inside `conn.call()`, blocking the single DB thread for the duration.
**Why it happens:** The existing export pattern collects all messages into a Vec inside the closure.
**How to avoid:** For the HTTP API, consider streaming: load messages in batches inside `conn.call()` and write each batch to the response. Alternatively, accept the blocking for Phase 3 and optimize later -- the existing CLI export pattern works and sessions are typically bounded (< 10K messages).
**Warning signs:** Other API requests stalling during large exports.

### Pitfall 5: Missing Content-Type on POST /v1/messages/query
**What goes wrong:** Clients send POST without `Content-Type: application/json` header, and axum's Json extractor rejects with 422.
**Why it happens:** axum Json extractor requires the content-type header by default.
**How to avoid:** Document the requirement. Optionally, implement a custom extractor that tries JSON parsing regardless of content-type, but this is non-standard. The standard behavior (requiring the header) is correct for a JSON API.
**Warning signs:** 422 responses from curl commands missing `-H "Content-Type: application/json"`.

### Pitfall 6: Socket Path Permissions and Cleanup
**What goes wrong:** On macOS, `/tmp` is actually `/private/tmp`, and socket files may have unexpected permissions preventing other processes from connecting.
**Why it happens:** OS-level filesystem behavior.
**How to avoid:** Use the `$CLAUDE_HISTORY_SOCKET` environment variable as override, defaulting to `/tmp/claude-history.sock`. Set appropriate permissions on the socket file after creation. Document the socket location.
**Warning signs:** "Permission denied" when CLI tries to connect to the daemon socket.

## Code Examples

Verified patterns from official sources:

### Graceful Shutdown Signal Handler
```rust
// Source: axum/examples/graceful-shutdown/src/main.rs
async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install signal handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
}
```

### Router Construction with Nested Routes
```rust
// Source: axum documentation
use axum::{routing::{get, post}, Router};

fn build_router(state: SharedState) -> Router {
    Router::new()
        // Health
        .route("/v1/health", get(health::handler))
        // Sessions
        .route("/v1/sessions", get(sessions::list))
        .route("/v1/sessions/:id", get(sessions::detail))
        .route("/v1/sessions/:id/conversation", get(sessions::conversation))
        .route("/v1/sessions/:id/tree", get(sessions::tree))
        .route("/v1/sessions/:id/agents", get(sessions::agents))
        .route("/v1/sessions/:id/summary", get(sessions::summary))
        // Messages
        .route("/v1/messages/query", post(messages::query))
        .route("/v1/messages/:uuid", get(messages::by_uuid))
        // Search
        .route("/v1/search", get(search::handler))
        // Analytics
        .route("/v1/analytics/tokens", get(analytics::tokens))
        .route("/v1/analytics/tools", get(analytics::tools))
        .route("/v1/analytics/models", get(analytics::models))
        // Export
        .route("/v1/export/:session_id", get(export_api::handler))
        // Schema
        .route("/v1/schema/versions", get(schema::versions))
        .route("/v1/schema/drift", get(schema::drift))
        .with_state(state)
}
```

### axum Query Extractor with Optional Parameters
```rust
// Source: axum Query extractor documentation
#[derive(Deserialize)]
struct SearchParams {
    q: String,
    #[serde(default = "default_limit")]
    limit: usize,
    #[serde(default)]
    offset: usize,
}

fn default_limit() -> usize { 20 }

async fn search(
    State(state): State<SharedState>,
    Query(params): Query<SearchParams>,
) -> Result<Json<Vec<SearchResult>>, ApiError> {
    let results = state.conn.call(move |conn| {
        claude_history_store::fts::search_messages(
            conn, &params.q, params.limit, params.offset,
        )
    }).await.map_err(ApiError::from)?;

    Ok(Json(results))
}
```

### CLI Socket Detection (CLI-15)
```rust
// Pattern for CLI-15: connect to daemon if available
fn socket_path() -> std::path::PathBuf {
    std::env::var("CLAUDE_HISTORY_SOCKET")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::path::PathBuf::from("/tmp/claude-history.sock"))
}

async fn connect_or_direct(db_path: Option<PathBuf>) -> ConnectionMode {
    let sock = socket_path();
    if sock.exists() {
        // Daemon is (likely) running -- use HTTP over UDS
        ConnectionMode::Daemon(sock)
    } else {
        // No daemon -- open DB directly (read-only)
        ConnectionMode::Direct(resolve_db_path(db_path))
    }
}
```
Note: Full CLI-15 implementation (HTTP client over UDS for all subcommands) is non-trivial. The minimum viable approach is to detect the socket and print a suggestion to the user ("daemon is running, consider using the API"). A full implementation would require an HTTP client (reqwest with unix-socket feature or manual hyper client) making the equivalent API calls. The planner should scope this carefully.

## New Store Functions Required

Several API endpoints require new query functions in `crates/store/src/query.rs` that do not exist yet. These must be added as part of Phase 3:

| Endpoint | New Store Function | Description |
|----------|--------------------|-------------|
| API-03 | `get_session(conn, session_id) -> Option<SessionDetail>` | Single session with project_path, timestamps, version, message_count, model |
| API-04 | `session_conversation(conn, session_id, include_thinking, include_tool_io, limit, offset) -> Vec<ConversationMessage>` | Messages with content blocks, ordered by timestamp; filters for block types |
| API-05 | `session_tree(conn, session_id) -> Vec<TreeNode>` | Messages with parent_uuid and is_sidechain for client-side tree construction |
| API-06 | `session_agents(conn, session_id) -> Vec<AgentEntry>` | Agents for a specific session from agents table |
| API-07 | `session_summary(conn, session_id) -> SessionSummaryStats` | Aggregated token totals, tool count, duration (first to last timestamp) |
| API-09 | `get_message(conn, uuid) -> Option<MessageDetail>` | Single message with content blocks and token usage |
| API-11 (day grouping) | `token_stats_by_day(conn) -> Vec<TokenStats>` | Token stats grouped by DATE(timestamp) |

The existing store functions (`list_sessions`, `query_messages`, `search_messages`, `token_stats_by_model`, `tool_frequency`, `model_breakdown`, `version_history`, `schema_drift_list`, `session_messages_for_export`) are directly reusable by the API handlers.

## State of the Art

| Old Approach | Current Approach | When Changed | Impact |
|--------------|------------------|--------------|--------|
| axum 0.7 with hyper-util for UDS | axum 0.8 Listener trait: UnixListener works natively with axum::serve | axum 0.8.0 (2024) | Eliminates hyper-util boilerplate for Unix domain socket serving |
| Manual shutdown coordination via channels | CancellationToken + with_graceful_shutdown | axum 0.7+ / tokio-util 0.7 | First-class graceful shutdown; no manual connection draining |
| tower-http 0.5 | tower-http 0.6 (compatible with hyper 1.0 / axum 0.7-0.8) | 2024 | Required for axum 0.8 compatibility |

**Deprecated/outdated:**
- `hyper::Server` (pre-hyper 1.0): Removed. Use `axum::serve` instead.
- axum `axum::Server` (pre-0.7): Removed. Use `axum::serve` instead.
- Manual `hyper_util` + `TokioIo` wrapping for UDS: No longer needed with axum 0.8 Listener trait.

## Open Questions

1. **CLI-15 Implementation Depth**
   - What we know: The spec says "CLI connects to daemon socket if available, otherwise opens DB read-only." The socket detection is straightforward.
   - What's unclear: Does "connects to daemon socket" mean all CLI subcommands should work via HTTP when the daemon is running? That requires an HTTP client and mapping every CLI command to an API call. This is a significant scope item.
   - Recommendation: Implement socket detection and direct-DB fallback for all subcommands. For daemon mode, implement a simple `is_daemon_running()` check. Full CLI-over-HTTP can be staged: start with direct DB for all subcommands but add daemon awareness (e.g., sync triggers daemon re-sync via API). The planner should decide scope boundary.

2. **Export Streaming (API-14)**
   - What we know: The existing export writes to a `Vec<u8>` buffer inside `conn.call()`. For HTTP, a streaming response would be more memory-efficient.
   - What's unclear: Whether axum's `Body` streaming can interleave with `conn.call()` batches without blocking the DB thread excessively.
   - Recommendation: Start with the buffered approach (load full export into memory, return as response body). This matches the existing CLI pattern and avoids complexity. Optimize to streaming in a later phase if memory becomes an issue.

3. **Conversation Tree (API-05) Data Model**
   - What we know: The `messages` table has `parent_uuid` and `is_sidechain` columns. These encode tree structure.
   - What's unclear: Whether to return a flat list with parent references (client builds the tree) or a nested JSON structure (server builds the tree).
   - Recommendation: Return a flat list with `parent_uuid`, `is_sidechain`, and `children_count` fields. This is simpler on the server, transfers efficiently, and gives clients flexibility. Tree construction in the client is trivial.

4. **Token Analytics Day Grouping (API-11)**
   - What we know: The spec says "token analysis with grouping (session, day, model)." We have `token_stats_by_model` and `token_stats_by_session`. We lack `token_stats_by_day`.
   - What's unclear: Whether "day" grouping means DATE(timestamp) on the message or the session's first_seen_at.
   - Recommendation: Group by DATE(messages.timestamp) via a new store function. Use a `group_by` query parameter on the API endpoint to select the grouping dimension.

## Sources

### Primary (HIGH confidence)
- [axum 0.8.8 documentation](https://docs.rs/axum/0.8.8/axum/) -- Router, State extractor, serve function, Listener trait
- [axum Listener trait](https://docs.rs/axum/0.8.8/axum/serve/trait.Listener.html) -- Confirms TcpListener and UnixListener implementations
- [axum graceful-shutdown example](https://github.com/tokio-rs/axum/blob/main/examples/graceful-shutdown/src/main.rs) -- Signal handling + with_graceful_shutdown pattern
- [axum unix-domain-socket example](https://github.com/tokio-rs/axum/blob/main/examples/unix-domain-socket/src/main.rs) -- UDS serving pattern
- [tokio-util CancellationToken](https://docs.rs/tokio-util/latest/tokio_util/sync/struct.CancellationToken.html) -- API for cancel, cancelled, child_token
- [tokio graceful shutdown guide](https://tokio.rs/tokio/topics/shutdown) -- CancellationToken + TaskTracker patterns
- [tower-http 0.6.8 documentation](https://docs.rs/tower-http/latest/tower_http/) -- TraceLayer, middleware listing
- [axum Json extractor](https://docs.rs/axum/0.8.8/axum/struct.Json.html) -- Request body deserialization and response serialization
- [axum Query extractor](https://docs.rs/axum/0.8.8/axum/extract/struct.Query.html) -- Query string deserialization with serde

### Secondary (MEDIUM confidence)
- [axum multiple listeners discussion](https://github.com/tokio-rs/axum/discussions/2949) -- Confirms tokio::spawn approach for dual listeners
- [axum state sharing discussion](https://github.com/tokio-rs/axum/discussions/964) -- SQLite connection sharing via Arc<AppState>
- [axum WithGracefulShutdown](https://docs.rs/axum/0.8.8/axum/serve/struct.WithGracefulShutdown.html) -- Struct returned by with_graceful_shutdown

### Tertiary (LOW confidence)
- None. All findings verified against primary documentation.

## Metadata

**Confidence breakdown:**
- Standard stack: HIGH -- axum 0.8 is the canonical Rust web framework for tokio; all features verified against official docs
- Architecture: HIGH -- patterns directly follow axum examples and existing codebase conventions (conn.call pattern, store function signatures)
- Pitfalls: HIGH -- UDS socket cleanup, error mapping, and shutdown coordination are well-documented in official examples and community discussions
- New store functions: MEDIUM -- the SQL queries are straightforward (they follow the same patterns as existing query.rs functions), but the exact return types and edge cases will be finalized during implementation

**Research date:** 2026-02-20
**Valid until:** 2026-03-20 (axum 0.8 is stable; no breaking changes expected within 30 days)
