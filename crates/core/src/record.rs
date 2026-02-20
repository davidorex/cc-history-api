//! Top-level JSONL record types.
//!
//! Every line in a Claude Code JSONL session file deserializes to one of the
//! 7 variants in [`JSONLRecord`]. The type field in the JSON object acts as
//! the discriminator via `serde(tag = "type")`.
//!
//! Records fall into three structural tiers:
//! - **Full-base** (user, assistant, progress, system): carry RecordBase fields
//! - **Partial** (queue-operation): has sessionId but no uuid
//! - **Lightweight** (summary, file-history-snapshot): no uuid or sessionId

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::message::{AssistantMessage, UserMessage};
use crate::progress::ProgressRecord;
use crate::system::SystemRecord;

/// Top-level discriminated union for all JSONL line types.
///
/// Deserialized via `serde(tag = "type")` — the JSON "type" field selects the variant.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum JSONLRecord {
    #[serde(rename = "user")]
    User(UserRecord),

    #[serde(rename = "assistant")]
    Assistant(AssistantRecord),

    #[serde(rename = "progress")]
    Progress(ProgressRecord),

    #[serde(rename = "system")]
    System(SystemRecord),

    #[serde(rename = "queue-operation")]
    QueueOperation(QueueOperationRecord),

    #[serde(rename = "summary")]
    Summary(SummaryRecord),

    #[serde(rename = "file-history-snapshot")]
    FileHistorySnapshot(FileHistorySnapshotRecord),
}

/// Shared base fields present on all full-base record types
/// (user, assistant, progress, system).
///
/// Uses camelCase to match the JSON field names emitted by Claude Code.
/// No overflow HashMap here — only ONE overflow per struct is allowed,
/// and it belongs on the outermost containing struct (e.g. UserRecord)
/// to avoid serde(flatten) ambiguity between nested levels.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordBase {
    pub uuid: String,
    pub timestamp: String,
    pub session_id: String,
    pub version: String,
    pub cwd: String,
    #[serde(default)]
    pub parent_uuid: Option<String>,
    #[serde(default)]
    pub is_sidechain: bool,
    #[serde(default)]
    pub user_type: String,
    #[serde(default)]
    pub git_branch: String,
    // Optional fields present on many but not all records
    #[serde(default)]
    pub slug: Option<String>,
    #[serde(default)]
    pub agent_id: Option<String>,
    #[serde(default)]
    pub team_name: Option<String>,
    #[serde(default)]
    pub is_meta: Option<bool>,
}

/// User record — full-base record with message content.
///
/// The overflow HashMap captures fields like isVisibleInTranscriptOnly,
/// isCompactSummary, sourceToolUseID, mcpMeta, imagePasteIds, and any
/// future fields that Claude Code may add.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserRecord {
    #[serde(flatten)]
    pub base: RecordBase,
    pub message: UserMessage,
    #[serde(default, rename = "sourceToolAssistantUUID")]
    pub source_tool_assistant_uuid: Option<String>,
    #[serde(default)]
    pub tool_use_result: Option<serde_json::Value>,
    #[serde(default)]
    pub thinking_metadata: Option<serde_json::Value>,
    #[serde(default)]
    pub todos: Option<serde_json::Value>,
    #[serde(default)]
    pub permission_mode: Option<String>,
    /// Catches unknown/rare fields (isVisibleInTranscriptOnly, isCompactSummary, etc.)
    #[serde(flatten)]
    pub overflow: HashMap<String, serde_json::Value>,
}

/// Assistant record — full-base record with the API response message.
///
/// The overflow HashMap captures fields like apiError, and any future
/// fields that Claude Code may add to the outer record envelope.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssistantRecord {
    #[serde(flatten)]
    pub base: RecordBase,
    pub message: AssistantMessage,
    #[serde(default)]
    pub request_id: Option<String>,
    #[serde(default)]
    pub is_api_error_message: Option<bool>,
    #[serde(default)]
    pub error: Option<String>,
    /// Catches unknown fields (apiError, duplicated teamName, etc.)
    #[serde(flatten)]
    pub overflow: HashMap<String, serde_json::Value>,
}

/// Queue-operation record — partial structure (has sessionId but no uuid).
///
/// These records track message queue operations (enqueue, dequeue, remove, popAll).
/// Content is only present for enqueue operations (~48.3% of records).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueueOperationRecord {
    pub operation: String,
    pub timestamp: String,
    pub session_id: String,
    #[serde(default)]
    pub content: Option<String>,
    /// Catches any unknown fields
    #[serde(flatten)]
    pub overflow: HashMap<String, serde_json::Value>,
}

/// Summary record — lightweight structure (no uuid or sessionId).
///
/// Contains a human-readable summary of a conversation segment and
/// a reference to the last message in the summarized sequence.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SummaryRecord {
    pub summary: String,
    pub leaf_uuid: String,
    /// Catches any unknown fields
    #[serde(flatten)]
    pub overflow: HashMap<String, serde_json::Value>,
}

/// File-history-snapshot record — lightweight structure (no uuid or sessionId).
///
/// Contains file backup metadata. The snapshot field is stored as raw JSON
/// because it contains complex nested structures (trackedFileBackups map)
/// that are better handled as opaque data in Phase 1.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileHistorySnapshotRecord {
    pub message_id: String,
    pub snapshot: serde_json::Value,
    #[serde(default)]
    pub is_snapshot_update: bool,
    /// Catches any unknown fields
    #[serde(flatten)]
    pub overflow: HashMap<String, serde_json::Value>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::MessageContent;

    /// Test: User record with plain string content deserializes (MessageContent::Text)
    #[test]
    fn test_user_record_string_content() {
        let json = r#"{
            "type": "user",
            "uuid": "abc-123",
            "timestamp": "2026-02-20T01:28:38.896Z",
            "sessionId": "sess-001",
            "version": "2.1.49",
            "cwd": "/home/user/project",
            "parentUuid": null,
            "isSidechain": false,
            "userType": "external",
            "gitBranch": "main",
            "message": {
                "role": "user",
                "content": "Hello, Claude!"
            }
        }"#;
        let record: JSONLRecord = serde_json::from_str(json).expect("should deserialize user record with string content");
        match record {
            JSONLRecord::User(r) => {
                assert_eq!(r.base.uuid, "abc-123");
                assert_eq!(r.base.session_id, "sess-001");
                match r.message.content {
                    MessageContent::Text(t) => assert_eq!(t, "Hello, Claude!"),
                    _ => panic!("Expected MessageContent::Text"),
                }
            }
            _ => panic!("Expected User variant"),
        }
    }

    /// Test: User record with block array content deserializes (MessageContent::Blocks with tool_result)
    #[test]
    fn test_user_record_block_content() {
        let json = r#"{
            "type": "user",
            "uuid": "abc-456",
            "timestamp": "2026-02-20T01:29:00.000Z",
            "sessionId": "sess-001",
            "version": "2.1.49",
            "cwd": "/home/user/project",
            "parentUuid": "abc-123",
            "isSidechain": false,
            "userType": "external",
            "gitBranch": "main",
            "sourceToolAssistantUUID": "assist-789",
            "message": {
                "role": "user",
                "content": [
                    {
                        "type": "tool_result",
                        "tool_use_id": "tool-001",
                        "content": "File written successfully.",
                        "is_error": false
                    },
                    {
                        "type": "text",
                        "text": "Here is the result."
                    }
                ]
            }
        }"#;
        let record: JSONLRecord = serde_json::from_str(json).expect("should deserialize user record with block content");
        match record {
            JSONLRecord::User(r) => {
                assert_eq!(r.base.parent_uuid, Some("abc-123".to_string()));
                assert_eq!(r.source_tool_assistant_uuid, Some("assist-789".to_string()));
                match &r.message.content {
                    MessageContent::Blocks(blocks) => {
                        assert_eq!(blocks.len(), 2);
                    }
                    _ => panic!("Expected MessageContent::Blocks"),
                }
            }
            _ => panic!("Expected User variant"),
        }
    }

    /// Test: Assistant record with text + thinking + tool_use blocks deserializes
    #[test]
    fn test_assistant_record_with_blocks() {
        let json = r#"{
            "type": "assistant",
            "uuid": "assist-001",
            "timestamp": "2026-02-20T01:30:00.000Z",
            "sessionId": "sess-001",
            "version": "2.1.49",
            "cwd": "/home/user/project",
            "parentUuid": "abc-456",
            "isSidechain": false,
            "userType": "external",
            "gitBranch": "main",
            "requestId": "req_011CYJ",
            "message": {
                "id": "msg_001",
                "type": "message",
                "role": "assistant",
                "model": "claude-opus-4-6",
                "content": [
                    {
                        "type": "thinking",
                        "thinking": "Let me think about this...",
                        "signature": "sig-abc"
                    },
                    {
                        "type": "text",
                        "text": "Here is my response."
                    },
                    {
                        "type": "tool_use",
                        "id": "tool-002",
                        "name": "Read",
                        "input": {"file_path": "/tmp/test.txt"}
                    }
                ],
                "stop_reason": "tool_use",
                "stop_sequence": null,
                "usage": {
                    "input_tokens": 1000,
                    "output_tokens": 500,
                    "cache_creation_input_tokens": 200,
                    "cache_read_input_tokens": 800
                }
            }
        }"#;
        let record: JSONLRecord = serde_json::from_str(json).expect("should deserialize assistant record with blocks");
        match record {
            JSONLRecord::Assistant(r) => {
                assert_eq!(r.base.uuid, "assist-001");
                assert_eq!(r.request_id, Some("req_011CYJ".to_string()));
                assert_eq!(r.message.model, "claude-opus-4-6");
                assert_eq!(r.message.content.len(), 3);
                assert_eq!(r.message.stop_reason, Some("tool_use".to_string()));
                let usage = r.message.usage.as_ref().expect("usage should be present");
                assert_eq!(usage.input_tokens, 1000);
                assert_eq!(usage.output_tokens, 500);
            }
            _ => panic!("Expected Assistant variant"),
        }
    }

    /// Test: UsageStats with unknown field "inference_geo" captures it in overflow
    #[test]
    fn test_usage_stats_overflow() {
        let json = r#"{
            "type": "assistant",
            "uuid": "assist-002",
            "timestamp": "2026-02-20T01:31:00.000Z",
            "sessionId": "sess-001",
            "version": "2.1.49",
            "cwd": "/home/user/project",
            "isSidechain": false,
            "userType": "external",
            "gitBranch": "main",
            "message": {
                "id": "msg_002",
                "type": "message",
                "role": "assistant",
                "model": "claude-sonnet-4-5-20250929",
                "content": [{"type": "text", "text": "Hi"}],
                "stop_reason": "end_turn",
                "usage": {
                    "input_tokens": 500,
                    "output_tokens": 100,
                    "inference_geo": "us-east-1",
                    "server_tool_use": {"web_search_requests": 2, "web_fetch_requests": 1}
                }
            }
        }"#;
        let record: JSONLRecord = serde_json::from_str(json).expect("should deserialize assistant with usage overflow");
        match record {
            JSONLRecord::Assistant(r) => {
                let usage = r.message.usage.as_ref().expect("usage should be present");
                assert_eq!(usage.input_tokens, 500);
                assert!(usage.overflow.contains_key("inference_geo"), "inference_geo should be in overflow");
                assert!(usage.overflow.contains_key("server_tool_use"), "server_tool_use should be in overflow");
                assert_eq!(usage.overflow.len(), 2, "exactly 2 unknown fields should be in overflow");
            }
            _ => panic!("Expected Assistant variant"),
        }
    }

    /// Test: Queue-operation record deserializes (no uuid, partial structure)
    #[test]
    fn test_queue_operation_record() {
        let json = r#"{
            "type": "queue-operation",
            "operation": "enqueue",
            "timestamp": "2026-02-20T01:32:00.000Z",
            "sessionId": "sess-002",
            "content": "Please fix the bug in main.rs"
        }"#;
        let record: JSONLRecord = serde_json::from_str(json).expect("should deserialize queue-operation record");
        match record {
            JSONLRecord::QueueOperation(r) => {
                assert_eq!(r.operation, "enqueue");
                assert_eq!(r.session_id, "sess-002");
                assert_eq!(r.content, Some("Please fix the bug in main.rs".to_string()));
            }
            _ => panic!("Expected QueueOperation variant"),
        }
    }

    /// Test: Summary record deserializes (lightweight, no uuid or sessionId)
    #[test]
    fn test_summary_record() {
        let json = r#"{
            "type": "summary",
            "summary": "The user asked to refactor the authentication module.",
            "leafUuid": "leaf-uuid-001"
        }"#;
        let record: JSONLRecord = serde_json::from_str(json).expect("should deserialize summary record");
        match record {
            JSONLRecord::Summary(r) => {
                assert_eq!(r.summary, "The user asked to refactor the authentication module.");
                assert_eq!(r.leaf_uuid, "leaf-uuid-001");
            }
            _ => panic!("Expected Summary variant"),
        }
    }

    /// Test: System record with subtype "stop_hook_summary" and extra fields captures unknowns in overflow
    #[test]
    fn test_system_record_overflow() {
        let json = r#"{
            "type": "system",
            "uuid": "sys-001",
            "timestamp": "2026-02-20T01:33:00.000Z",
            "sessionId": "sess-001",
            "version": "2.1.49",
            "cwd": "/home/user/project",
            "isSidechain": false,
            "userType": "external",
            "gitBranch": "main",
            "subtype": "stop_hook_summary",
            "level": "info",
            "hookCount": 3,
            "hookInfos": [{"name": "pre-commit", "output": "ok"}],
            "hookErrors": [],
            "preventedContinuation": false,
            "stopReason": "end_turn",
            "hasOutput": true
        }"#;
        let record: JSONLRecord = serde_json::from_str(json).expect("should deserialize system record with overflow");
        match record {
            JSONLRecord::System(r) => {
                assert_eq!(r.base.uuid, "sys-001");
                assert_eq!(r.subtype, "stop_hook_summary");
                assert_eq!(r.level, Some("info".to_string()));
                assert_eq!(r.hook_count, Some(3));
                // hookInfos, hookErrors, preventedContinuation, stopReason, hasOutput
                // should all be captured in overflow
                assert!(r.overflow.contains_key("hookInfos"), "hookInfos should be in overflow");
                assert!(r.overflow.contains_key("hookErrors"), "hookErrors should be in overflow");
                assert!(r.overflow.contains_key("preventedContinuation"), "preventedContinuation should be in overflow");
                assert!(r.overflow.contains_key("stopReason"), "stopReason should be in overflow");
                assert!(r.overflow.contains_key("hasOutput"), "hasOutput should be in overflow");
            }
            _ => panic!("Expected System variant"),
        }
    }

    /// Test: File-history-snapshot record deserializes
    #[test]
    fn test_file_history_snapshot_record() {
        let json = r#"{
            "type": "file-history-snapshot",
            "messageId": "msg-snap-001",
            "snapshot": {
                "messageId": "msg-snap-001",
                "trackedFileBackups": {
                    "src/main.rs": {"backupFileName": "main.rs.bak", "version": 1, "backupTime": "2026-02-20T01:00:00Z"}
                },
                "timestamp": "2026-02-20T01:34:00.000Z"
            },
            "isSnapshotUpdate": false
        }"#;
        let record: JSONLRecord = serde_json::from_str(json).expect("should deserialize file-history-snapshot record");
        match record {
            JSONLRecord::FileHistorySnapshot(r) => {
                assert_eq!(r.message_id, "msg-snap-001");
                assert!(!r.is_snapshot_update);
                assert!(r.snapshot.is_object());
            }
            _ => panic!("Expected FileHistorySnapshot variant"),
        }
    }

    /// Test: Queue-operation without content field (dequeue operation)
    #[test]
    fn test_queue_operation_no_content() {
        let json = r#"{
            "type": "queue-operation",
            "operation": "dequeue",
            "timestamp": "2026-02-20T01:35:00.000Z",
            "sessionId": "sess-002"
        }"#;
        let record: JSONLRecord = serde_json::from_str(json).expect("should deserialize dequeue queue-operation");
        match record {
            JSONLRecord::QueueOperation(r) => {
                assert_eq!(r.operation, "dequeue");
                assert!(r.content.is_none());
            }
            _ => panic!("Expected QueueOperation variant"),
        }
    }

    /// Test: User record with overflow fields (unknown fields captured, not discarded)
    #[test]
    fn test_user_record_overflow() {
        let json = r#"{
            "type": "user",
            "uuid": "abc-789",
            "timestamp": "2026-02-20T01:36:00.000Z",
            "sessionId": "sess-001",
            "version": "2.1.49",
            "cwd": "/home/user/project",
            "isSidechain": false,
            "userType": "external",
            "gitBranch": "main",
            "message": {
                "role": "user",
                "content": "test"
            },
            "isVisibleInTranscriptOnly": true,
            "isCompactSummary": true,
            "mcpMeta": {"server": "test-server"},
            "imagePasteIds": [1, 2]
        }"#;
        let record: JSONLRecord = serde_json::from_str(json).expect("should deserialize user record with overflow fields");
        match record {
            JSONLRecord::User(r) => {
                assert!(r.overflow.contains_key("isVisibleInTranscriptOnly"), "isVisibleInTranscriptOnly should be in overflow");
                assert!(r.overflow.contains_key("isCompactSummary"), "isCompactSummary should be in overflow");
                assert!(r.overflow.contains_key("mcpMeta"), "mcpMeta should be in overflow");
                assert!(r.overflow.contains_key("imagePasteIds"), "imagePasteIds should be in overflow");
            }
            _ => panic!("Expected User variant"),
        }
    }
}
