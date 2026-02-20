//! Progress record types.
//!
//! Progress records are the most common record type (~302K observed).
//! The `data` field contains one of 8+ known subtypes (hook_progress,
//! agent_progress, bash_progress, etc.) with widely varying shapes.
//!
//! In Phase 1, the data field is stored as raw JSON Value. The decomposer
//! will extract `data.type` as a string for indexing while preserving the
//! full data object. Typed variants for each data.type may be added in
//! a future phase if query patterns demand it.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::record::RecordBase;

/// Progress record — full-base record with polymorphic data payload.
///
/// The data field is intentionally stored as serde_json::Value because
/// the 8+ data.type variants have very different shapes and the cost
/// of modeling them all is not justified in Phase 1.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProgressRecord {
    #[serde(flatten)]
    pub base: RecordBase,
    /// The entire progress data object (contains a "type" discriminator
    /// and variant-specific fields)
    pub data: serde_json::Value,
    /// Catches any unknown fields at the progress record level
    #[serde(flatten)]
    pub overflow: HashMap<String, serde_json::Value>,
}

#[cfg(test)]
mod tests {
    use crate::record::JSONLRecord;

    #[test]
    fn test_progress_record_hook_progress() {
        let json = r#"{
            "type": "progress",
            "uuid": "prog-001",
            "timestamp": "2026-02-20T01:40:00.000Z",
            "sessionId": "sess-001",
            "version": "2.1.49",
            "cwd": "/home/user/project",
            "isSidechain": false,
            "userType": "external",
            "gitBranch": "main",
            "data": {
                "type": "hook_progress",
                "hookEvent": "pre-commit",
                "hookName": "lint",
                "command": "eslint ."
            }
        }"#;
        let record: JSONLRecord = serde_json::from_str(json).expect("should deserialize progress record");
        match record {
            JSONLRecord::Progress(r) => {
                assert_eq!(r.base.uuid, "prog-001");
                assert_eq!(r.data["type"], "hook_progress");
                assert_eq!(r.data["hookEvent"], "pre-commit");
            }
            _ => panic!("Expected Progress variant"),
        }
    }

    #[test]
    fn test_progress_record_bash_progress() {
        let json = r#"{
            "type": "progress",
            "uuid": "prog-002",
            "timestamp": "2026-02-20T01:41:00.000Z",
            "sessionId": "sess-001",
            "version": "2.1.49",
            "cwd": "/home/user/project",
            "isSidechain": false,
            "userType": "external",
            "gitBranch": "main",
            "data": {
                "type": "bash_progress",
                "output": "total 42\ndrwxr-xr-x  5 user  staff   160 Feb 20 01:41 .",
                "fullOutput": true,
                "elapsedTimeSeconds": 0.5,
                "totalLines": 2
            }
        }"#;
        let record: JSONLRecord = serde_json::from_str(json).expect("should deserialize bash progress");
        match record {
            JSONLRecord::Progress(r) => {
                assert_eq!(r.data["type"], "bash_progress");
                assert_eq!(r.data["elapsedTimeSeconds"], 0.5);
            }
            _ => panic!("Expected Progress variant"),
        }
    }

    #[test]
    fn test_progress_record_with_overflow() {
        let json = r#"{
            "type": "progress",
            "uuid": "prog-003",
            "timestamp": "2026-02-20T01:42:00.000Z",
            "sessionId": "sess-001",
            "version": "2.1.49",
            "cwd": "/home/user/project",
            "isSidechain": false,
            "userType": "external",
            "gitBranch": "main",
            "data": {"type": "agent_progress", "message": "working..."},
            "unknownProgressField": "some_value"
        }"#;
        let record: JSONLRecord = serde_json::from_str(json).expect("should deserialize progress record with overflow");
        match record {
            JSONLRecord::Progress(r) => {
                assert!(r.overflow.contains_key("unknownProgressField"), "unknown field should be in overflow");
            }
            _ => panic!("Expected Progress variant"),
        }
    }
}
