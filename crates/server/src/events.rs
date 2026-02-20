//! SSE event types and handler for the GET /v1/events endpoint.
//!
//! Defines the `SseEvent` enum representing the seven event types emitted by the
//! system:
//!   - record:added, session:started, schema:drift, version:changed (Phase 4)
//!   - file:written, file:edited, git:commit (Phase 5 artifact events)
//!
//! The `events_handler` function subscribes to the broadcast channel and streams
//! events to connected SSE clients.
//!
//! The broadcast channel in AppState allows fan-out to multiple concurrent SSE
//! clients. Each handler invocation creates its own Receiver via `subscribe()`.
//! Slow consumers that fall behind the channel capacity (1024) will have lagged
//! events silently dropped rather than blocking the producer.
//!
//! Requirement IDs: SSE-01, SSE-02, SSE-03, SSE-04, SSE-05, SSE-06, SSE-07

use std::convert::Infallible;

use axum::extract::State;
use axum::response::sse::{Event, KeepAlive, Sse};
use futures_util::stream::Stream;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::StreamExt;

use crate::state::SharedState;

/// Server-Sent Event variants matching the seven spec event types.
///
/// Each variant maps to a distinct SSE `event:` name via `event_type()` and
/// carries structured data serialized as the SSE `data:` payload via
/// `to_json_data()`.
///
/// The enum is not tagged via serde attributes because the SSE event name is
/// set through axum's `Event::event()` method rather than being embedded in the
/// JSON payload. The `data:` field contains only the variant's struct fields.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub enum SseEvent {
    /// A new record (or batch of records) was ingested for a session.
    RecordAdded {
        session_id: String,
        records_synced: usize,
        file_path: String,
    },
    /// A new session directory was detected and ingestion started.
    SessionStarted {
        session_id: String,
    },
    /// Unknown fields were detected during ingestion (schema drift).
    SchemaDrift {
        new_fields: usize,
        session_id: String,
    },
    /// The Claude Code version string changed between records.
    VersionChanged {
        old_version: Option<String>,
        new_version: String,
        session_id: String,
    },
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
    /// Return the SSE event type name (the `event:` field in the SSE frame).
    ///
    /// These names match the spec requirement identifiers exactly:
    /// record:added, session:started, schema:drift, version:changed,
    /// file:written, file:edited, git:commit.
    pub fn event_type(&self) -> &'static str {
        match self {
            SseEvent::RecordAdded { .. } => "record:added",
            SseEvent::SessionStarted { .. } => "session:started",
            SseEvent::SchemaDrift { .. } => "schema:drift",
            SseEvent::VersionChanged { .. } => "version:changed",
            SseEvent::FileWritten { .. } => "file:written",
            SseEvent::FileEdited { .. } => "file:edited",
            SseEvent::GitCommit { .. } => "git:commit",
        }
    }

    /// Serialize the variant's data fields as a JSON value for the SSE `data:` payload.
    ///
    /// This intentionally produces a flat JSON object containing only the fields
    /// of the active variant, without the variant name wrapper that serde's default
    /// enum serialization would add.
    pub fn to_json_data(&self) -> serde_json::Value {
        match self {
            SseEvent::RecordAdded {
                session_id,
                records_synced,
                file_path,
            } => serde_json::json!({
                "session_id": session_id,
                "records_synced": records_synced,
                "file_path": file_path,
            }),
            SseEvent::SessionStarted { session_id } => serde_json::json!({
                "session_id": session_id,
            }),
            SseEvent::SchemaDrift {
                new_fields,
                session_id,
            } => serde_json::json!({
                "new_fields": new_fields,
                "session_id": session_id,
            }),
            SseEvent::VersionChanged {
                old_version,
                new_version,
                session_id,
            } => serde_json::json!({
                "old_version": old_version,
                "new_version": new_version,
                "session_id": session_id,
            }),
            SseEvent::FileWritten {
                session_id,
                file_path,
                message_uuid,
            } => serde_json::json!({
                "session_id": session_id,
                "file_path": file_path,
                "message_uuid": message_uuid,
            }),
            SseEvent::FileEdited {
                session_id,
                file_path,
                message_uuid,
            } => serde_json::json!({
                "session_id": session_id,
                "file_path": file_path,
                "message_uuid": message_uuid,
            }),
            SseEvent::GitCommit {
                session_id,
                commit_message,
                branch,
                message_uuid,
            } => serde_json::json!({
                "session_id": session_id,
                "commit_message": commit_message,
                "branch": branch,
                "message_uuid": message_uuid,
            }),
        }
    }
}

/// SSE endpoint handler for GET /v1/events.
///
/// Subscribes to the broadcast channel in AppState and streams events to the
/// client as Server-Sent Events. Each connected client gets its own Receiver,
/// enabling fan-out from a single producer (the file watcher in Plan 02).
///
/// Behavior:
/// - Lagged events (when a slow client falls behind the 1024-event buffer) are
///   silently dropped via `filter_map` on `BroadcastStreamRecvError::Lagged`.
/// - Each `SseEvent` is mapped to an axum `Event` with the appropriate `event:`
///   name and JSON `data:` payload.
/// - Keep-alive pings are sent at the default interval to prevent connection
///   timeouts from intermediate proxies or load balancers.
pub async fn events_handler(
    State(state): State<SharedState>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let rx = state.event_tx.subscribe();
    let stream = BroadcastStream::new(rx);

    let sse_stream = stream.filter_map(|result| match result {
        Ok(sse_event) => {
            let event = Event::default()
                .event(sse_event.event_type())
                .json_data(sse_event.to_json_data())
                .unwrap_or_else(|_| Event::default().data("serialization error"));
            Some(Ok(event))
        }
        // Lagged errors indicate the client fell behind the broadcast buffer.
        // Silently drop — the client will receive the next available event.
        Err(_) => None,
    });

    Sse::new(sse_stream).keep_alive(KeepAlive::default())
}
