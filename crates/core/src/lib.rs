// claude-history-core: shared types and data models for the Claude history system.
//
// This crate provides:
// - Serde record types for all 7 JSONL record variants (record.rs)
// - Content block models and message types (message.rs)
// - Progress record types (progress.rs)
// - System record types (system.rs)
// - Streaming JSONL parser with byte-offset tracking (parser.rs)

pub mod message;
pub mod parser;
pub mod progress;
pub mod record;
pub mod system;
