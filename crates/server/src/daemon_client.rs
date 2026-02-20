//! HTTP-over-UDS client for communicating with the claude-history daemon.
//!
//! `DaemonClient` sends HTTP/1.1 requests over a Unix domain socket to the
//! daemon API, deserializing responses into the same types used by the store
//! layer. This enables CLI subcommands to transparently route through the
//! daemon when it is running, rather than opening a direct DB connection.
//!
//! `ConnectionMode` is the top-level enum that CLI dispatch code pattern-matches
//! on to choose between daemon-routed and direct-DB code paths.
//!
//! `resolve_socket_path` follows the CLI arg > env var > default resolution
//! pattern for determining where the daemon socket lives.
//!
//! `detect_connection_mode` probes the socket with a health check to verify
//! the daemon is actually responsive before committing to daemon mode,
//! falling back to direct DB on any failure (stale socket, daemon crashed, etc.).
//!
//! Requirement IDs: CLI-15

use std::fmt;
use std::path::{Path, PathBuf};

use http_body_util::{BodyExt, Full};
use hyper::body::Bytes;
use hyper::client::conn::http1;
use hyper::Request;
use hyper_util::rt::TokioIo;
use serde::de::DeserializeOwned;
use tokio::net::UnixStream;

use crate::api::health::HealthResponse;
use claude_history_store::fts::SearchResult;
use claude_history_store::query::{
    DriftEntry, MessageResult, ModelStats, SessionSummary, TokenStats, ToolStats, VersionEntry,
};

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// Errors that can occur when communicating with the daemon over UDS.
#[derive(Debug)]
pub enum DaemonError {
    /// Failed to connect to the Unix domain socket (e.g., socket missing,
    /// permission denied, connection refused).
    Connection(std::io::Error),

    /// HTTP protocol-level error from hyper (e.g., malformed response,
    /// connection reset during transfer).
    Hyper(hyper::Error),

    /// The daemon returned a non-2xx HTTP status code. The body field
    /// contains the response body text for diagnostic purposes.
    Api { status: u16, body: String },

    /// Failed to deserialize the response body as JSON into the expected type.
    Json(serde_json::Error),
}

impl fmt::Display for DaemonError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DaemonError::Connection(e) => write!(f, "daemon connection failed: {}", e),
            DaemonError::Hyper(e) => write!(f, "HTTP protocol error: {}", e),
            DaemonError::Api { status, body } => {
                write!(f, "daemon returned HTTP {}: {}", status, body)
            }
            DaemonError::Json(e) => write!(f, "JSON deserialization error: {}", e),
        }
    }
}

impl std::error::Error for DaemonError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            DaemonError::Connection(e) => Some(e),
            DaemonError::Hyper(e) => Some(e),
            DaemonError::Api { .. } => None,
            DaemonError::Json(e) => Some(e),
        }
    }
}

impl From<std::io::Error> for DaemonError {
    fn from(err: std::io::Error) -> Self {
        DaemonError::Connection(err)
    }
}

impl From<hyper::Error> for DaemonError {
    fn from(err: hyper::Error) -> Self {
        DaemonError::Hyper(err)
    }
}

impl From<serde_json::Error> for DaemonError {
    fn from(err: serde_json::Error) -> Self {
        DaemonError::Json(err)
    }
}

// ---------------------------------------------------------------------------
// ConnectionMode
// ---------------------------------------------------------------------------

/// Selects whether CLI subcommands route through the daemon HTTP API or
/// open a direct database connection.
///
/// CLI dispatch code pattern-matches on this enum at startup, then calls
/// either DaemonClient methods (Daemon variant) or store layer functions
/// via conn.call (Direct variant) for each subcommand.
pub enum ConnectionMode {
    /// The daemon is running and responsive. Requests go through HTTP-over-UDS.
    Daemon(DaemonClient),

    /// No daemon available. The CLI opens the database directly.
    Direct {
        /// Async SQLite connection for direct store layer access.
        conn: tokio_rusqlite::Connection,
        /// Path to the database file (retained for diagnostics/logging).
        db_path: PathBuf,
    },
}

// ---------------------------------------------------------------------------
// DaemonClient
// ---------------------------------------------------------------------------

/// HTTP client that communicates with the claude-history daemon over a
/// Unix domain socket.
///
/// Each method maps to a specific `/v1/` endpoint, constructs the
/// appropriate HTTP request, sends it over UDS via hyper, and deserializes
/// the JSON response into the same store-layer types used by direct DB access.
///
/// A new UnixStream connection is opened for each request. This is
/// acceptable for CLI one-shot commands where connection pooling would add
/// complexity without meaningful benefit.
pub struct DaemonClient {
    /// Path to the Unix domain socket the daemon is listening on.
    socket_path: PathBuf,
}

impl DaemonClient {
    /// Create a new DaemonClient targeting the given socket path.
    pub fn new(socket_path: PathBuf) -> Self {
        Self { socket_path }
    }

    /// Return the socket path this client is configured to connect to.
    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    // -------------------------------------------------------------------
    // Private transport layer
    // -------------------------------------------------------------------

    /// Send an HTTP request over the Unix domain socket and return the
    /// raw response body bytes.
    ///
    /// Opens a new UnixStream, performs the hyper HTTP/1.1 handshake,
    /// sends the request, and collects the full response body. If the
    /// response status is not 2xx, returns `DaemonError::Api`.
    async fn request(
        &self,
        method: &str,
        path: &str,
        body: Option<&[u8]>,
    ) -> Result<Vec<u8>, DaemonError> {
        // Open a Unix domain socket connection to the daemon.
        let stream = UnixStream::connect(&self.socket_path).await?;
        let io = TokioIo::new(stream);

        // Perform the HTTP/1.1 handshake over the UDS connection.
        let (mut sender, conn) = http1::handshake(io).await?;

        // Spawn a task to drive the connection to completion. If the
        // connection encounters an error after the response is received,
        // it is logged but does not affect the already-collected response.
        tokio::spawn(async move {
            if let Err(e) = conn.await {
                tracing::warn!("daemon client connection error: {}", e);
            }
        });

        // Build the HTTP request. The Host header is set to "localhost"
        // as required by HTTP/1.1 even though we're communicating over UDS.
        let http_method = method
            .parse::<hyper::Method>()
            .unwrap_or(hyper::Method::GET);

        let req = if let Some(body_bytes) = body {
            Request::builder()
                .method(http_method)
                .uri(path)
                .header("Host", "localhost")
                .header("Content-Type", "application/json")
                .body(Full::new(Bytes::from(body_bytes.to_vec())))
                .expect("building HTTP request should not fail with valid inputs")
        } else {
            Request::builder()
                .method(http_method)
                .uri(path)
                .header("Host", "localhost")
                .body(Full::new(Bytes::new()))
                .expect("building HTTP request should not fail with valid inputs")
        };

        // Send the request and await the response.
        let response = sender.send_request(req).await?;

        let status = response.status().as_u16();

        // Collect the response body into memory.
        let body_bytes = response.into_body().collect().await?.to_bytes().to_vec();

        // Check for non-2xx status codes.
        if !(200..300).contains(&status) {
            let body_text = String::from_utf8_lossy(&body_bytes).into_owned();
            return Err(DaemonError::Api {
                status,
                body: body_text,
            });
        }

        Ok(body_bytes)
    }

    /// Send a GET request and deserialize the JSON response.
    async fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T, DaemonError> {
        let bytes = self.request("GET", path, None).await?;
        let value = serde_json::from_slice(&bytes)?;
        Ok(value)
    }

    /// Send a POST request with a JSON body and deserialize the JSON response.
    async fn post<T: DeserializeOwned>(
        &self,
        path: &str,
        body: &[u8],
    ) -> Result<T, DaemonError> {
        let bytes = self.request("POST", path, Some(body)).await?;
        let value = serde_json::from_slice(&bytes)?;
        Ok(value)
    }

    // -------------------------------------------------------------------
    // Public endpoint methods
    // -------------------------------------------------------------------

    /// GET /v1/health — server health check.
    ///
    /// Returns status, db_size, record_count, and version from the daemon.
    pub async fn health(&self) -> Result<HealthResponse, DaemonError> {
        self.get("/v1/health").await
    }

    /// GET /v1/sessions — list sessions with optional filters.
    ///
    /// Constructs query string from the provided optional parameters.
    pub async fn sessions(
        &self,
        project: Option<&str>,
        after: Option<&str>,
        before: Option<&str>,
        limit: Option<usize>,
    ) -> Result<Vec<SessionSummary>, DaemonError> {
        let mut params = Vec::new();
        if let Some(p) = project {
            params.push(format!("project={}", urlencoded(p)));
        }
        if let Some(a) = after {
            params.push(format!("after={}", urlencoded(a)));
        }
        if let Some(b) = before {
            params.push(format!("before={}", urlencoded(b)));
        }
        if let Some(l) = limit {
            params.push(format!("limit={}", l));
        }
        let path = if params.is_empty() {
            "/v1/sessions".to_string()
        } else {
            format!("/v1/sessions?{}", params.join("&"))
        };
        self.get(&path).await
    }

    /// GET /v1/search?q=... — full-text search across message content.
    ///
    /// Constructs the query string with q, limit, and offset parameters.
    pub async fn search(
        &self,
        query: &str,
        limit: Option<usize>,
        offset: Option<usize>,
    ) -> Result<Vec<SearchResult>, DaemonError> {
        let mut params = vec![format!("q={}", urlencoded(query))];
        if let Some(l) = limit {
            params.push(format!("limit={}", l));
        }
        if let Some(o) = offset {
            params.push(format!("offset={}", o));
        }
        let path = format!("/v1/search?{}", params.join("&"));
        self.get(&path).await
    }

    /// POST /v1/messages/query — query messages with JSON filter body.
    ///
    /// Serializes the filter parameters as a JSON body and POSTs to the
    /// daemon. The daemon compiles these into parameterized SQL.
    pub async fn query_messages(
        &self,
        session_id: Option<&str>,
        message_type: Option<&str>,
        model: Option<&str>,
        tool: Option<&str>,
        after: Option<&str>,
        before: Option<&str>,
        limit: Option<usize>,
    ) -> Result<Vec<MessageResult>, DaemonError> {
        let body = serde_json::json!({
            "session_id": session_id,
            "message_type": message_type,
            "model": model,
            "tool": tool,
            "after": after,
            "before": before,
            "limit": limit,
        });
        let body_bytes = serde_json::to_vec(&body)?;
        self.post("/v1/messages/query", &body_bytes).await
    }

    /// GET /v1/analytics/tokens — token usage statistics.
    ///
    /// Supports group_by parameter ("model", "session", "day") and optional
    /// session_id filter for session-level grouping.
    pub async fn stats_tokens(
        &self,
        group_by: Option<&str>,
        session_id: Option<&str>,
    ) -> Result<Vec<TokenStats>, DaemonError> {
        let mut params = Vec::new();
        if let Some(g) = group_by {
            params.push(format!("group_by={}", urlencoded(g)));
        }
        if let Some(s) = session_id {
            params.push(format!("session_id={}", urlencoded(s)));
        }
        let path = if params.is_empty() {
            "/v1/analytics/tokens".to_string()
        } else {
            format!("/v1/analytics/tokens?{}", params.join("&"))
        };
        self.get(&path).await
    }

    /// GET /v1/analytics/tools — tool invocation frequency and error rates.
    pub async fn stats_tools(&self) -> Result<Vec<ToolStats>, DaemonError> {
        self.get("/v1/analytics/tools").await
    }

    /// GET /v1/analytics/models — model usage breakdown.
    pub async fn stats_models(&self) -> Result<Vec<ModelStats>, DaemonError> {
        self.get("/v1/analytics/models").await
    }

    /// GET /v1/export/:session_id — export a session in the specified format.
    ///
    /// Returns the raw response body bytes. The caller is responsible for
    /// writing the bytes to stdout or a file. Unlike other methods, this
    /// does not deserialize JSON — the response may be markdown or CSV
    /// depending on the format parameter.
    pub async fn export_session(
        &self,
        session_id: &str,
        format: Option<&str>,
    ) -> Result<Vec<u8>, DaemonError> {
        let mut params = Vec::new();
        if let Some(f) = format {
            params.push(format!("format={}", urlencoded(f)));
        }
        let path = if params.is_empty() {
            format!("/v1/export/{}", urlencoded(session_id))
        } else {
            format!(
                "/v1/export/{}?{}",
                urlencoded(session_id),
                params.join("&")
            )
        };
        // Use request() directly instead of get() because the response
        // may not be JSON (could be markdown or CSV).
        self.request("GET", &path, None).await
    }

    /// GET /v1/schema/versions — Claude Code version history.
    pub async fn version_history(&self) -> Result<Vec<VersionEntry>, DaemonError> {
        self.get("/v1/schema/versions").await
    }

    /// GET /v1/schema/drift — schema drift log entries.
    ///
    /// Supports optional record_type filter and limit parameters.
    pub async fn schema_drift(
        &self,
        record_type: Option<&str>,
        limit: Option<usize>,
    ) -> Result<Vec<DriftEntry>, DaemonError> {
        let mut params = Vec::new();
        if let Some(rt) = record_type {
            params.push(format!("record_type={}", urlencoded(rt)));
        }
        if let Some(l) = limit {
            params.push(format!("limit={}", l));
        }
        let path = if params.is_empty() {
            "/v1/schema/drift".to_string()
        } else {
            format!("/v1/schema/drift?{}", params.join("&"))
        };
        self.get(&path).await
    }
}

// ---------------------------------------------------------------------------
// Socket path resolution
// ---------------------------------------------------------------------------

/// Resolve the daemon socket path using CLI arg > env var > default priority.
///
/// 1. If `cli_arg` is Some, use that path directly.
/// 2. If the `CLAUDE_HISTORY_SOCKET` environment variable is set and non-empty,
///    use that path.
/// 3. Fall back to `/tmp/claude-history.sock`.
pub fn resolve_socket_path(cli_arg: Option<&Path>) -> PathBuf {
    if let Some(p) = cli_arg {
        return p.to_path_buf();
    }

    if let Ok(p) = std::env::var("CLAUDE_HISTORY_SOCKET") {
        if !p.is_empty() {
            return PathBuf::from(p);
        }
    }

    PathBuf::from("/tmp/claude-history.sock")
}

// ---------------------------------------------------------------------------
// Connection mode detection
// ---------------------------------------------------------------------------

/// Detect whether to use daemon mode or direct DB mode.
///
/// If the socket file exists, attempts a health check GET /v1/health to
/// verify the daemon is actually responsive. This avoids committing to
/// daemon mode when a stale socket file exists from a crashed daemon.
///
/// If the health check succeeds, returns `ConnectionMode::Daemon`.
/// If the socket does not exist, or the health check fails for any reason,
/// falls back to `ConnectionMode::Direct` by opening the database at
/// the resolved path.
///
/// The `db_path_arg` is passed through to `init_db` for the direct fallback.
/// `resolve_db_path` is called by the caller — this function receives the
/// already-resolved db_path.
pub async fn detect_connection_mode(
    socket_path: &Path,
    db_path: PathBuf,
) -> Result<ConnectionMode, String> {
    // Check if socket file exists on disk.
    if socket_path.exists() {
        // Attempt a health check to verify the daemon is responsive.
        let client = DaemonClient::new(socket_path.to_path_buf());
        match client.health().await {
            Ok(health) => {
                tracing::info!(
                    socket = %socket_path.display(),
                    daemon_version = %health.version,
                    "Connected to daemon via UDS"
                );
                return Ok(ConnectionMode::Daemon(client));
            }
            Err(e) => {
                tracing::debug!(
                    socket = %socket_path.display(),
                    error = %e,
                    "Daemon health check failed, falling back to direct DB"
                );
                // Fall through to direct mode.
            }
        }
    }

    // Direct mode: open the database.
    if !db_path.exists() {
        return Err(format!(
            "Database file does not exist: {}\n\
             Run `claude-history sync` first to create the database.",
            db_path.display()
        ));
    }

    let conn = claude_history_store::db::init_db(&db_path)
        .await
        .map_err(|e| format!("Failed to open database at {}: {}", db_path.display(), e))?;

    tracing::debug!(
        db = %db_path.display(),
        "Connection mode: Direct DB (no responsive daemon)"
    );

    Ok(ConnectionMode::Direct {
        conn,
        db_path,
    })
}

// ---------------------------------------------------------------------------
// URL encoding helper
// ---------------------------------------------------------------------------

/// Minimal percent-encoding for query parameter values.
///
/// Encodes characters that are not unreserved in RFC 3986 (letters, digits,
/// hyphen, period, underscore, tilde). This is sufficient for the query
/// parameter values used by this client (session IDs, search terms, etc.).
///
/// A full URL-encoding crate (like `percent-encoding`) is intentionally
/// avoided to keep the dependency count low for this simple use case.
fn urlencoded(input: &str) -> String {
    let mut encoded = String::with_capacity(input.len());
    for byte in input.bytes() {
        match byte {
            b'A'..=b'Z'
            | b'a'..=b'z'
            | b'0'..=b'9'
            | b'-'
            | b'.'
            | b'_'
            | b'~' => {
                encoded.push(byte as char);
            }
            _ => {
                encoded.push_str(&format!("%{:02X}", byte));
            }
        }
    }
    encoded
}
