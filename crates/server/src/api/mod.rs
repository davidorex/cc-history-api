//! API module — HTTP handler types, error conversion, and route construction.
//!
//! Exposes endpoint handlers organized by resource (health, sessions, messages,
//! search) and a `build_router` function that assembles all routes into an
//! axum Router with shared application state.

pub mod analytics;
pub mod error;
pub mod health;
pub mod messages;
pub mod schema;
pub mod search;
pub mod sessions;

use axum::routing::{get, post};
use axum::Router;

use crate::state::SharedState;

/// Build the axum Router with all API routes and shared state.
///
/// Registers 10 endpoints across 4 resource groups:
///
/// **Health:**
///   - GET /v1/health
///
/// **Sessions:**
///   - GET /v1/sessions
///   - GET /v1/sessions/:id
///   - GET /v1/sessions/:id/conversation
///   - GET /v1/sessions/:id/tree
///   - GET /v1/sessions/:id/agents
///   - GET /v1/sessions/:id/summary
///
/// **Messages:**
///   - POST /v1/messages/query
///   - GET  /v1/messages/:uuid
///
/// **Search:**
///   - GET /v1/search
///
/// Additional endpoints (analytics, export, schema) will be added in Plan 3.
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
        // Plan 3 will add: analytics, export, schema endpoints
        .with_state(state)
}
