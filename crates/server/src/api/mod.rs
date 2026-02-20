//! API module — HTTP handler types, error conversion, and route construction.
//!
//! Exposes endpoint handlers organized by resource (health, sessions, messages,
//! search, analytics, export, schema) and a `build_router` function that
//! assembles all 16 routes into an axum Router with shared application state
//! and TraceLayer middleware for request/response logging.

pub mod analytics;
pub mod error;
pub mod export_api;
pub mod health;
pub mod messages;
pub mod schema;
pub mod search;
pub mod sessions;

use axum::routing::{get, post};
use axum::Router;
use tower_http::trace::TraceLayer;

use crate::state::SharedState;

/// Build the axum Router with all API routes and shared state.
///
/// Registers 16 endpoints across 7 resource groups with TraceLayer
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
        // Middleware
        .layer(TraceLayer::new_for_http())
        // Shared state
        .with_state(state)
}
