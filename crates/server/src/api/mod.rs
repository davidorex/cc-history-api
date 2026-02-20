//! API module — HTTP handler types, error conversion, and route construction.
//!
//! Exposes endpoint handlers organized by resource (health, sessions, messages,
//! search) and a `build_router` function that assembles all routes into an
//! axum Router.

pub mod error;
pub mod health;
pub mod sessions;
