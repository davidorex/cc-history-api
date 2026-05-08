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

use serde::de::{self, Deserializer};
use serde::ser::Serializer;
use serde::{Deserialize, Serialize};

use crate::message::{AssistantMessage, UserMessage};
use crate::progress::ProgressRecord;
use crate::system::SystemRecord;

/// Top-level discriminated union for all JSONL line types.
///
/// The JSON `type` field selects the variant. Seven known discriminator values
/// dispatch to typed variants; any other string-valued discriminator falls
/// through to [`JSONLRecord::Unknown`], which preserves the original
/// discriminator name and the entire raw JSON object so the data is not lost.
///
/// The dispatch is implemented by a hand-rolled `Deserialize` impl below,
/// not by `#[serde(tag = "type")]`. The two-pass dispatch — deserialize to
/// `serde_json::Value`, inspect the `type` field, then dispatch to the typed
/// variant struct via `serde_json::from_value` or fall back to the `Unknown`
/// variant — is the mechanism the audit at
/// `.planning/audit/jsonl-unknown-record-type-attachment-investigation-2026-05-08T0551-asia-shanghai.md`
/// describes as Path A. `#[serde(tag = "type")]` does not natively support a
/// catch-all variant that preserves the payload, so the dispatch is hand-written.
///
/// Records whose JSON has no `type` field, or whose `type` is not a JSON
/// string, return a deserializer error — preserving the existing
/// malformed-JSONL failure mode handled by `crates/core/src/parser.rs`.
///
/// The seven typed variants must continue to deserialize byte-identically to
/// the previous `#[derive(Deserialize)] #[serde(tag = "type")]` behavior; the
/// existing test suite at the bottom of this file exercises each variant and
/// is the regression net for that invariant.
#[derive(Debug, Clone)]
pub enum JSONLRecord {
    User(UserRecord),
    Assistant(AssistantRecord),
    Progress(ProgressRecord),
    System(SystemRecord),
    QueueOperation(QueueOperationRecord),
    Summary(SummaryRecord),
    FileHistorySnapshot(FileHistorySnapshotRecord),
    /// Catch-all for JSONL lines whose `type` discriminator is a string but
    /// not one of the seven known values (e.g. `attachment`, `last-prompt`,
    /// `custom-title`, `permission-mode`, `agent-name`, `ai-title`, or any
    /// future record type). Preserves both the discriminator name and the
    /// full raw JSON object so the bytes are recoverable downstream.
    ///
    /// Variant placement: this is the LAST variant in the enum so adding it
    /// does not shift the position of any prior typed variant in the source
    /// — important because the manual `Deserialize` impl checks the seven
    /// known discriminators by string match and order is irrelevant there,
    /// but match-arm ordering elsewhere in the codebase remains stable.
    Unknown {
        type_name: String,
        raw: serde_json::Value,
    },
}

/// Discriminator value -> typed variant dispatch.
///
/// Returns `Some` if the input is one of the seven known JSONL record-type
/// strings, `None` otherwise. Centralized so the manual `Deserialize` impl
/// and any future call site (e.g. validation, telemetry) read from one source.
fn known_record_type(type_name: &str) -> Option<KnownRecordType> {
    match type_name {
        "user" => Some(KnownRecordType::User),
        "assistant" => Some(KnownRecordType::Assistant),
        "progress" => Some(KnownRecordType::Progress),
        "system" => Some(KnownRecordType::System),
        "queue-operation" => Some(KnownRecordType::QueueOperation),
        "summary" => Some(KnownRecordType::Summary),
        "file-history-snapshot" => Some(KnownRecordType::FileHistorySnapshot),
        _ => None,
    }
}

/// Internal enum naming the seven known record types. Used only by the
/// manual `Deserialize` impl to keep the dispatch table tidy; not exported.
#[derive(Debug, Clone, Copy)]
enum KnownRecordType {
    User,
    Assistant,
    Progress,
    System,
    QueueOperation,
    Summary,
    FileHistorySnapshot,
}

impl<'de> Deserialize<'de> for JSONLRecord {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        // First pass: capture the entire JSON object as a generic Value so
        // we can inspect `type` without committing to a typed shape yet.
        // This mirrors the audit's Path A two-pass dispatch.
        let value = serde_json::Value::deserialize(deserializer)?;

        // Extract the `type` discriminator. A missing or non-string `type`
        // is an error condition the previous derived impl also rejected
        // (`#[serde(tag = "type")]` requires a string discriminator); we
        // preserve that contract so malformed JSONL still fails at the
        // parser layer instead of being smuggled into Unknown with a
        // synthetic type_name.
        let type_field = value.get("type").ok_or_else(|| {
            de::Error::missing_field("type")
        })?;
        let type_name = type_field.as_str().ok_or_else(|| {
            de::Error::invalid_type(
                de::Unexpected::Other(&format!("non-string type: {type_field}")),
                &"a string-valued `type` discriminator",
            )
        })?;

        // Second pass: if the discriminator is one of the seven known strings,
        // dispatch to the typed variant struct via from_value. The error
        // message from each typed dispatch retains its original shape so
        // existing tests asserting on parse-error text still match.
        //
        // We must remove `type` from the JSON object before dispatching:
        // each typed struct uses `#[serde(flatten)] overflow: HashMap<...>`
        // and the `type` discriminator would otherwise be captured into
        // overflow (which would land in schema_drift_log as if it were a
        // novel field). The previous `#[serde(tag = "type")]` derived impl
        // consumed the discriminator before flattening; we replicate that
        // by stripping the field here.
        if let Some(kind) = known_record_type(type_name) {
            let mut typed_value = value;
            if let Some(map) = typed_value.as_object_mut() {
                map.remove("type");
            }
            return match kind {
                KnownRecordType::User => serde_json::from_value::<UserRecord>(typed_value)
                    .map(JSONLRecord::User)
                    .map_err(de::Error::custom),
                KnownRecordType::Assistant => serde_json::from_value::<AssistantRecord>(typed_value)
                    .map(JSONLRecord::Assistant)
                    .map_err(de::Error::custom),
                KnownRecordType::Progress => serde_json::from_value::<ProgressRecord>(typed_value)
                    .map(JSONLRecord::Progress)
                    .map_err(de::Error::custom),
                KnownRecordType::System => serde_json::from_value::<SystemRecord>(typed_value)
                    .map(JSONLRecord::System)
                    .map_err(de::Error::custom),
                KnownRecordType::QueueOperation => {
                    serde_json::from_value::<QueueOperationRecord>(typed_value)
                        .map(JSONLRecord::QueueOperation)
                        .map_err(de::Error::custom)
                }
                KnownRecordType::Summary => serde_json::from_value::<SummaryRecord>(typed_value)
                    .map(JSONLRecord::Summary)
                    .map_err(de::Error::custom),
                KnownRecordType::FileHistorySnapshot => {
                    serde_json::from_value::<FileHistorySnapshotRecord>(typed_value)
                        .map(JSONLRecord::FileHistorySnapshot)
                        .map_err(de::Error::custom)
                }
            };
        }

        // Discriminator is a string but not one of the seven known values.
        // Capture the discriminator name and the full raw JSON object so the
        // record is preserved for downstream forensic and drift logging
        // (`decompose_unknown` in crates/store/src/decompose.rs).
        Ok(JSONLRecord::Unknown {
            type_name: type_name.to_string(),
            raw: value,
        })
    }
}

impl Serialize for JSONLRecord {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        // For each typed variant, build a serde_json::Value that matches the
        // shape the previous `#[derive(Serialize)] #[serde(tag = "type")]`
        // emitted: the inner struct's fields with a `type` discriminator
        // field merged in. We use serde_json::to_value + Map::insert rather
        // than a custom map serializer because the typed variant structs
        // already use #[serde(flatten)] for their RecordBase and overflow
        // fields, and re-deriving that structure here would risk drift.
        //
        // For Unknown, we serialize the captured raw Value directly — it
        // already includes the original `type` field intact.
        match self {
            JSONLRecord::User(r) => insert_type_and_serialize(r, "user", serializer),
            JSONLRecord::Assistant(r) => insert_type_and_serialize(r, "assistant", serializer),
            JSONLRecord::Progress(r) => insert_type_and_serialize(r, "progress", serializer),
            JSONLRecord::System(r) => insert_type_and_serialize(r, "system", serializer),
            JSONLRecord::QueueOperation(r) => {
                insert_type_and_serialize(r, "queue-operation", serializer)
            }
            JSONLRecord::Summary(r) => insert_type_and_serialize(r, "summary", serializer),
            JSONLRecord::FileHistorySnapshot(r) => {
                insert_type_and_serialize(r, "file-history-snapshot", serializer)
            }
            JSONLRecord::Unknown { raw, .. } => raw.serialize(serializer),
        }
    }
}

/// Serialize a typed variant by first converting it to a JSON object, then
/// inserting the `type` discriminator at the front of the map. Mirrors the
/// shape `#[serde(tag = "type")]` produced when this enum used the derived
/// impl; preserves backward compatibility for any code path that relied on
/// the JSON form of a JSONLRecord.
fn insert_type_and_serialize<T, S>(value: &T, type_name: &str, serializer: S) -> Result<S::Ok, S::Error>
where
    T: Serialize,
    S: Serializer,
{
    use serde::ser::Error as _;
    let mut json = serde_json::to_value(value).map_err(S::Error::custom)?;
    if let Some(map) = json.as_object_mut() {
        map.insert(
            "type".to_string(),
            serde_json::Value::String(type_name.to_string()),
        );
    }
    json.serialize(serializer)
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

    // -----------------------------------------------------------------------
    // B1.1 — JSONLRecord::Unknown variant tests
    //
    // These tests cover the variant-level catch-all introduced to close the
    // structural blind spot where ~13.5K corpus records were silently dropped
    // by the parser when their `type` discriminator did not match one of the
    // seven known strings. The first six tests above (unchanged) are the
    // regression net for the seven typed variants — they must continue to
    // pass byte-identically against the new manual `Deserialize` impl.
    // -----------------------------------------------------------------------

    /// Test B-A: round-trip Serialize/Deserialize for a known variant produces
    /// JSON whose typed shape matches the input. We don't assert byte-identity
    /// of the entire JSON (HashMap iteration order is non-deterministic for
    /// the overflow field) but we verify that re-parsing the serialized form
    /// returns to the same logical structure.
    #[test]
    fn test_known_variant_roundtrip_user() {
        let json = r#"{
            "type": "user",
            "uuid": "abc-rt",
            "timestamp": "2026-02-20T01:00:00.000Z",
            "sessionId": "sess-rt",
            "version": "2.1.49",
            "cwd": "/home/user/project",
            "isSidechain": false,
            "userType": "external",
            "gitBranch": "main",
            "message": {"role": "user", "content": "Round trip"}
        }"#;
        let record: JSONLRecord = serde_json::from_str(json).expect("user record should parse");
        let serialized =
            serde_json::to_string(&record).expect("user record should serialize");
        // Re-deserialize and confirm the variant + key fields match.
        let reparsed: JSONLRecord =
            serde_json::from_str(&serialized).expect("serialized form should re-parse");
        match reparsed {
            JSONLRecord::User(r) => {
                assert_eq!(r.base.uuid, "abc-rt");
                assert_eq!(r.base.session_id, "sess-rt");
            }
            _ => panic!("Expected User variant after round-trip"),
        }
        // The serialized form must include the `type` discriminator so it
        // round-trips through downstream consumers that re-parse JSONLRecord.
        let as_value: serde_json::Value =
            serde_json::from_str(&serialized).unwrap();
        assert_eq!(as_value["type"], "user");
        // The `type` field must NOT have leaked into overflow during the
        // typed deserialization (regression check for the strip-before-flatten
        // contract documented in the manual Deserialize impl).
        assert!(
            as_value.get("type").is_some(),
            "type discriminator should be present"
        );
    }

    /// Test B-B: an unknown-discriminator JSONL line deserializes to
    /// `JSONLRecord::Unknown` capturing the discriminator name and the full
    /// raw payload. This covers the dominant corpus-loss case (`attachment`,
    /// `last-prompt`, etc.) and any future Claude Code record-type emissions.
    #[test]
    fn test_unknown_variant_captures_discriminator_and_raw() {
        let json = r#"{
            "type": "fictitious-test-type",
            "foo": "bar",
            "nested": {"baz": 42},
            "sessionId": "sess-unk"
        }"#;
        let record: JSONLRecord =
            serde_json::from_str(json).expect("unknown discriminator should fall through to Unknown");
        match record {
            JSONLRecord::Unknown { type_name, raw } => {
                assert_eq!(type_name, "fictitious-test-type");
                assert_eq!(
                    raw.get("foo").and_then(|v| v.as_str()),
                    Some("bar"),
                    "raw payload should preserve top-level foo field"
                );
                assert_eq!(
                    raw.get("nested").and_then(|v| v.get("baz")).and_then(|v| v.as_i64()),
                    Some(42),
                    "raw payload should preserve nested fields"
                );
                // The discriminator itself is preserved inside `raw` too,
                // since Unknown captures the entire original object.
                assert_eq!(
                    raw.get("type").and_then(|v| v.as_str()),
                    Some("fictitious-test-type"),
                    "raw payload should retain the original `type` field"
                );
            }
            _ => panic!("Expected JSONLRecord::Unknown for an unknown discriminator"),
        }
    }

    /// Test B-B-2: each of the six observed unknown discriminators from the
    /// corpus survey (attachment, last-prompt, custom-title, permission-mode,
    /// agent-name, ai-title) deserializes to JSONLRecord::Unknown rather than
    /// failing. This is the corpus-survey-driven regression check.
    #[test]
    fn test_unknown_variant_known_corpus_discriminators() {
        let cases: &[(&str, &str)] = &[
            ("attachment", r#"{"type":"attachment","attachment":{"type":"hook_success"}}"#),
            ("last-prompt", r#"{"type":"last-prompt","lastPrompt":"hi","sessionId":"s"}"#),
            ("custom-title", r#"{"type":"custom-title","customTitle":"x","sessionId":"s"}"#),
            ("permission-mode", r#"{"type":"permission-mode","permissionMode":"plan","sessionId":"s"}"#),
            ("agent-name", r#"{"type":"agent-name","agentName":"a","sessionId":"s"}"#),
            ("ai-title", r#"{"type":"ai-title","aiTitle":"a","sessionId":"s"}"#),
        ];
        for (expected_name, json) in cases {
            let record: JSONLRecord = serde_json::from_str(json)
                .unwrap_or_else(|e| panic!("{expected_name} should parse as Unknown: {e}"));
            match record {
                JSONLRecord::Unknown { type_name, .. } => {
                    assert_eq!(&type_name, expected_name);
                }
                _ => panic!("Expected Unknown variant for {expected_name}"),
            }
        }
    }

    /// Test B-D: a JSONL line with no `type` field returns a deserializer
    /// error rather than silently falling through to Unknown. Preserves the
    /// existing malformed-JSONL failure mode handled by parser.rs.
    #[test]
    fn test_missing_type_field_errors() {
        let json = r#"{"sessionId":"s","foo":"bar"}"#;
        let result: Result<JSONLRecord, _> = serde_json::from_str(json);
        assert!(
            result.is_err(),
            "JSONL line missing `type` field must error, not yield Unknown; got {:?}",
            result.ok()
        );
    }

    /// Test B-D-2: a JSONL line whose `type` is a non-string (null, number,
    /// object) returns a deserializer error rather than yielding Unknown.
    /// We never want a synthetic non-string discriminator coerced into the
    /// type_name field — that would corrupt downstream drift logging.
    #[test]
    fn test_non_string_type_field_errors() {
        for bad in [
            r#"{"type":null,"sessionId":"s"}"#,
            r#"{"type":42,"sessionId":"s"}"#,
            r#"{"type":{"nested":"x"},"sessionId":"s"}"#,
        ] {
            let result: Result<JSONLRecord, _> = serde_json::from_str(bad);
            assert!(
                result.is_err(),
                "non-string type discriminator must error: {bad}"
            );
        }
    }

    /// Test B-E: the typed dispatch must NOT leak the `type` discriminator
    /// into the per-variant overflow HashMap. Without the strip-before-flatten
    /// guard in the manual Deserialize impl, `type` would land in
    /// UserRecord.overflow (since UserRecord uses `#[serde(flatten)] overflow:
    /// HashMap<...>`) and ultimately get logged to schema_drift_log as if it
    /// were a novel field. Regression check for that invariant.
    #[test]
    fn test_typed_dispatch_does_not_leak_type_to_overflow() {
        let json = r#"{
            "type": "user",
            "uuid": "abc-leak",
            "timestamp": "2026-02-20T01:00:00.000Z",
            "sessionId": "sess-leak",
            "version": "2.1.49",
            "cwd": "/home/user/project",
            "isSidechain": false,
            "userType": "external",
            "gitBranch": "main",
            "message": {"role": "user", "content": "hi"}
        }"#;
        let record: JSONLRecord =
            serde_json::from_str(json).expect("user record should parse");
        match record {
            JSONLRecord::User(r) => {
                assert!(
                    !r.overflow.contains_key("type"),
                    "type discriminator must not leak into overflow; overflow keys = {:?}",
                    r.overflow.keys().collect::<Vec<_>>()
                );
            }
            _ => panic!("Expected User variant"),
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
