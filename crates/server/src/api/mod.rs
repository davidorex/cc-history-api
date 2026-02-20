//! API module — HTTP handler types, error conversion, and route construction.
//!
//! Exposes endpoint handlers organized by resource (health, sessions, messages,
//! search, analytics, export, schema, files, git, artifacts, events) and a
//! `build_router` function that assembles all 28 routes into an axum Router
//! with shared application state and TraceLayer middleware for request/response
//! logging.

pub mod analytics;
pub mod artifacts_api;
pub mod error;
pub mod export_api;
pub mod files;
pub mod git;
pub mod health;
pub mod messages;
pub mod schema;
pub mod search;
pub mod sessions;

use axum::routing::{get, post};
use axum::Router;
use tower_http::trace::TraceLayer;

use crate::events;
use crate::state::SharedState;

/// Build the axum Router with all API routes and shared state.
///
/// Registers 28 endpoints across 11 resource groups with TraceLayer
/// middleware for structured request/response logging:
///
/// **Health:**
///   - GET /v1/health
///
/// **Sessions:**
///   - GET /v1/sessions
///   - GET /v1/sessions/{id}
///   - GET /v1/sessions/{id}/conversation
///   - GET /v1/sessions/{id}/tree
///   - GET /v1/sessions/{id}/agents
///   - GET /v1/sessions/{id}/summary
///
/// **Messages:**
///   - POST /v1/messages/query
///   - GET  /v1/messages/{uuid}
///
/// **Search:**
///   - GET /v1/search
///
/// **Analytics:**
///   - GET /v1/analytics/tokens
///   - GET /v1/analytics/tools
///   - GET /v1/analytics/models
///
/// **Export:**
///   - GET /v1/export/{session_id}
///
/// **Schema:**
///   - GET /v1/schema/versions
///   - GET /v1/schema/drift
///
/// **Files:** [API-17 through API-22]
///   - GET  /v1/files
///   - GET  /v1/files/search
///   - POST /v1/files/query
///   - GET  /v1/files/{file_id}
///   - GET  /v1/files/{file_id}/content
///   - GET  /v1/files/{file_id}/diff
///
/// **Git:** [API-23 through API-25]
///   - GET /v1/git
///   - GET /v1/git/commits
///   - GET /v1/git/commits/{session_id}
///
/// **Artifacts:** [API-26 and API-27]
///   - GET /v1/artifacts/{session_id}
///   - GET /v1/artifacts/{session_id}/timeline
///
/// **Events:**
///   - GET /v1/events (SSE stream)
pub fn build_router(state: SharedState) -> Router {
    Router::new()
        // Health
        .route("/v1/health", get(health::health))
        // Sessions
        .route("/v1/sessions", get(sessions::list))
        .route("/v1/sessions/{id}", get(sessions::detail))
        .route(
            "/v1/sessions/{id}/conversation",
            get(sessions::conversation),
        )
        .route("/v1/sessions/{id}/tree", get(sessions::tree))
        .route("/v1/sessions/{id}/agents", get(sessions::agents))
        .route("/v1/sessions/{id}/summary", get(sessions::summary))
        // Messages
        .route("/v1/messages/query", post(messages::query))
        .route("/v1/messages/{uuid}", get(messages::by_uuid))
        // Search
        .route("/v1/search", get(search::search))
        // Analytics
        .route("/v1/analytics/tokens", get(analytics::tokens))
        .route("/v1/analytics/tools", get(analytics::tools))
        .route("/v1/analytics/models", get(analytics::models))
        // Export
        .route("/v1/export/{session_id}", get(export_api::handler))
        // Schema
        .route("/v1/schema/versions", get(schema::versions))
        .route("/v1/schema/drift", get(schema::drift))
        // Files [API-17 through API-22]
        // IMPORTANT: /v1/files/search and /v1/files/query MUST be registered
        // BEFORE /v1/files/{file_id} to avoid path parameter capturing
        // "search" and "query" as file_id values.
        .route("/v1/files", get(files::list_files))
        .route("/v1/files/search", get(files::search_files))
        .route("/v1/files/query", post(files::query_files))
        .route("/v1/files/{file_id}", get(files::file_detail))
        .route("/v1/files/{file_id}/content", get(files::file_content))
        .route("/v1/files/{file_id}/diff", get(files::file_diff))
        // Git [API-23 through API-25]
        // IMPORTANT: /v1/git/commits MUST be registered BEFORE
        // /v1/git/commits/{session_id} to avoid path parameter capturing
        // "commits" as part of a different route.
        .route("/v1/git", get(git::list_git))
        .route("/v1/git/commits", get(git::git_commits))
        .route("/v1/git/commits/{session_id}", get(git::session_git_commits))
        // Artifacts [API-26 and API-27]
        .route(
            "/v1/artifacts/{session_id}",
            get(artifacts_api::session_artifacts),
        )
        .route(
            "/v1/artifacts/{session_id}/timeline",
            get(artifacts_api::session_timeline),
        )
        // Events (SSE)
        .route("/v1/events", get(events::events_handler))
        // Middleware
        .layer(TraceLayer::new_for_http())
        // Shared state
        .with_state(state)
}
