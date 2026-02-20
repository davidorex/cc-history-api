//! System record types.
//!
//! System records were not in the original spec but are the 4th most common
//! record type (~14K observed). They use a `subtype` field for discrimination
//! with 6 known subtypes: stop_hook_summary, turn_duration, compact_boundary,
//! api_error, local_command, microcompact_boundary.
//!
//! Since subtypes share very few fields, a single struct with overflow is used
//! rather than a nested enum. Subtype-specific fields land in the overflow
//! HashMap for drift detection and future typed extraction.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::record::RecordBase;

/// System record — full-base record with subtype discrimination.
///
/// Known subtypes and their characteristic fields:
/// - `stop_hook_summary`: hookCount, hookInfos, hookErrors, preventedContinuation, stopReason, hasOutput
/// - `turn_duration`: durationMs, isMeta
/// - `compact_boundary`: content, level, logicalParentUuid, compactMetadata
/// - `api_error`: level, error, retryInMs, retryAttempt, maxRetries
/// - `local_command`: content, level, isMeta
/// - `microcompact_boundary`: microcompactMetadata
///
/// Only a few commonly-shared fields are modeled explicitly;
/// the rest land in overflow for schema drift detection.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemRecord {
    #[serde(flatten)]
    pub base: RecordBase,
    pub subtype: String,
    #[serde(default)]
    pub level: Option<String>,
    #[serde(default)]
    pub duration_ms: Option<u64>,
    #[serde(default)]
    pub hook_count: Option<u32>,
    #[serde(default)]
    pub content: Option<String>,
    /// Catches subtype-specific fields: hookInfos, hookErrors,
    /// preventedContinuation, stopReason, hasOutput, compactMetadata, etc.
    #[serde(flatten)]
    pub overflow: HashMap<String, serde_json::Value>,
}

#[cfg(test)]
mod tests {
    use crate::record::JSONLRecord;

    #[test]
    fn test_system_record_turn_duration() {
        let json = r#"{
            "type": "system",
            "uuid": "sys-td-001",
            "timestamp": "2026-02-20T01:50:00.000Z",
            "sessionId": "sess-001",
            "version": "2.1.49",
            "cwd": "/home/user/project",
            "isSidechain": false,
            "userType": "external",
            "gitBranch": "main",
            "subtype": "turn_duration",
            "durationMs": 4532
        }"#;
        let record: JSONLRecord = serde_json::from_str(json).expect("should deserialize turn_duration system record");
        match record {
            JSONLRecord::System(r) => {
                assert_eq!(r.subtype, "turn_duration");
                assert_eq!(r.duration_ms, Some(4532));
                assert!(r.level.is_none());
            }
            _ => panic!("Expected System variant"),
        }
    }

    #[test]
    fn test_system_record_compact_boundary() {
        let json = r#"{
            "type": "system",
            "uuid": "sys-cb-001",
            "timestamp": "2026-02-20T01:51:00.000Z",
            "sessionId": "sess-001",
            "version": "2.1.49",
            "cwd": "/home/user/project",
            "isSidechain": false,
            "userType": "external",
            "gitBranch": "main",
            "subtype": "compact_boundary",
            "content": "Context window compacted",
            "level": "info",
            "logicalParentUuid": "parent-uuid-001",
            "compactMetadata": {"strategy": "truncate", "removedTokens": 5000}
        }"#;
        let record: JSONLRecord = serde_json::from_str(json).expect("should deserialize compact_boundary system record");
        match record {
            JSONLRecord::System(r) => {
                assert_eq!(r.subtype, "compact_boundary");
                assert_eq!(r.content, Some("Context window compacted".to_string()));
                assert_eq!(r.level, Some("info".to_string()));
                assert!(r.overflow.contains_key("logicalParentUuid"), "logicalParentUuid should be in overflow");
                assert!(r.overflow.contains_key("compactMetadata"), "compactMetadata should be in overflow");
            }
            _ => panic!("Expected System variant"),
        }
    }

    #[test]
    fn test_system_record_api_error() {
        let json = r#"{
            "type": "system",
            "uuid": "sys-ae-001",
            "timestamp": "2026-02-20T01:52:00.000Z",
            "sessionId": "sess-001",
            "version": "2.1.49",
            "cwd": "/home/user/project",
            "isSidechain": false,
            "userType": "external",
            "gitBranch": "main",
            "subtype": "api_error",
            "level": "error",
            "error": "overloaded_error",
            "retryInMs": 5000,
            "retryAttempt": 1,
            "maxRetries": 3
        }"#;
        let record: JSONLRecord = serde_json::from_str(json).expect("should deserialize api_error system record");
        match record {
            JSONLRecord::System(r) => {
                assert_eq!(r.subtype, "api_error");
                assert_eq!(r.level, Some("error".to_string()));
                // "error" field should be in overflow since it's not a named field on SystemRecord
                // (only "content" is modeled, not "error" for the api_error subtype)
                assert!(r.overflow.contains_key("error"), "error should be in overflow");
                assert!(r.overflow.contains_key("retryInMs"), "retryInMs should be in overflow");
                assert!(r.overflow.contains_key("retryAttempt"), "retryAttempt should be in overflow");
                assert!(r.overflow.contains_key("maxRetries"), "maxRetries should be in overflow");
            }
            _ => panic!("Expected System variant"),
        }
    }
}
