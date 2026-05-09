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
/// The JSON `type` field selects the variant. Eight known discriminator values
/// dispatch to typed variants (the seven historical full/partial/lightweight
/// shapes plus `attachment`, modeled by C1.1 with a 12-subtype inner body
/// enum). Any other string-valued discriminator falls through to
/// [`JSONLRecord::Unknown`], which preserves the original discriminator name
/// and the entire raw JSON object so the data is not lost.
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
/// The seven historical typed variants must continue to deserialize
/// byte-identically to the previous `#[derive(Deserialize)] #[serde(tag = "type")]`
/// behavior; the existing test suite at the bottom of this file exercises each
/// variant and is the regression net for that invariant.
#[derive(Debug, Clone)]
pub enum JSONLRecord {
    User(UserRecord),
    Assistant(AssistantRecord),
    Progress(ProgressRecord),
    System(SystemRecord),
    QueueOperation(QueueOperationRecord),
    Summary(SummaryRecord),
    FileHistorySnapshot(FileHistorySnapshotRecord),
    /// Attachment record introduced as a typed variant in C1.1. Carries an
    /// `AttachmentBody` whose own discriminator (`attachment.type`) selects
    /// one of 12 modeled subtypes covering ~97% of attachment records by
    /// volume. Subtypes outside the modeled set fall through to
    /// [`AttachmentBody::Unknown`] (a Path-A pattern applied at the inner
    /// discriminator level) so the parent record still deserializes intact
    /// and the unmodeled subtype is recoverable downstream.
    Attachment(AttachmentRecord),
    /// Catch-all for JSONL lines whose `type` discriminator is a string but
    /// not one of the eight known values (e.g. `last-prompt`, `custom-title`,
    /// `permission-mode`, `agent-name`, `ai-title`, or any future record
    /// type). Preserves both the discriminator name and the full raw JSON
    /// object so the bytes are recoverable downstream.
    ///
    /// Variant placement: this is the LAST variant in the enum so adding it
    /// does not shift the position of any prior typed variant in the source
    /// — important because the manual `Deserialize` impl checks the known
    /// discriminators by string match and order is irrelevant there, but
    /// match-arm ordering elsewhere in the codebase remains stable.
    Unknown {
        type_name: String,
        raw: serde_json::Value,
    },
}

/// Discriminator value -> typed variant dispatch.
///
/// Returns `Some` if the input is one of the eight known JSONL record-type
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
        "attachment" => Some(KnownRecordType::Attachment),
        _ => None,
    }
}

/// Internal enum naming the eight known record types. Used only by the
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
    Attachment,
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
                KnownRecordType::Attachment => {
                    serde_json::from_value::<AttachmentRecord>(typed_value)
                        .map(JSONLRecord::Attachment)
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
            JSONLRecord::Attachment(r) => insert_type_and_serialize(r, "attachment", serializer),
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

// =============================================================================
// C1.1 — AttachmentRecord + AttachmentBody (12 modeled subtypes)
//
// `attachment` records carry a nested `attachment` object whose own `type`
// field acts as the inner discriminator. The outer envelope (uuid, sessionId,
// timestamp, cwd, version, gitBranch, slug, parentUuid, entrypoint) mirrors
// the shape of the historical full-base records but with no `message`/`subtype`
// payload — the payload lives entirely under `attachment`.
//
// AttachmentBody enumerates the 12 subtypes that the C1.1 corpus survey
// observed at >=10 records each (covering ~9,800 of 10,085 records, ~97% by
// volume). The remaining ~10 subtypes (~85 records, <1%) fall through to
// AttachmentBody::Unknown via the manual Deserialize impl, mirroring the
// Path-A two-pass dispatch from JSONLRecord above. C1.2 is responsible for
// routing modeled subtypes to typed columns in the `attachments` and
// `hook_executions` tables; unmodeled subtypes ride in `attachments.body_json`
// and additionally log to `record_type_drift_log` with
// `record_type = "attachment.<subtype>"` so future promotion is data-driven.
//
// Field selection per subtype tracks the plan's §C1.1 table exactly. Fields
// observed in real records but not enumerated by the plan (e.g. `removedNames`
// on `mcp_instructions_delta`, `skillCount`/`isInitial` on `skill_listing`,
// `addedLines`/`readdedNames` on `deferred_tools_delta`,
// `contentDiffersFromDisk` inside `nested_memory.content`) are captured by
// each subtype struct's #[serde(flatten)] HashMap so they remain queryable
// downstream without expanding the modeled schema in C1.1.
// =============================================================================

/// An `attachment`-discriminated JSONL record.
///
/// The envelope fields mirror those observed on real attachment records in the
/// corpus: `uuid`, `sessionId`, `timestamp`, `cwd`, `version`, `gitBranch`,
/// `slug`, `parentUuid`, `entrypoint`, `userType`, `isSidechain`, plus the
/// nested `attachment` body. Some fields are optional because not every
/// attachment subtype carries every envelope field (e.g. some `task_reminder`
/// records observed without `slug`); `#[serde(default)]` plus `Option` keeps
/// missing-field deserialization permissive without dropping data.
///
/// The catch-all `overflow` HashMap captures any envelope-level field that is
/// not yet enumerated here (e.g. `agentId` was observed on some
/// `deferred_tools_delta` records). This mirrors the overflow-capture pattern
/// used by `UserRecord`, `AssistantRecord`, etc.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AttachmentRecord {
    pub uuid: String,
    pub timestamp: String,
    pub session_id: String,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub parent_uuid: Option<String>,
    #[serde(default)]
    pub is_sidechain: bool,
    #[serde(default)]
    pub user_type: Option<String>,
    #[serde(default)]
    pub git_branch: Option<String>,
    #[serde(default)]
    pub slug: Option<String>,
    #[serde(default)]
    pub entrypoint: Option<String>,
    /// The inner discriminated body. Twelve subtypes are modeled; unknown
    /// subtypes deserialize to [`AttachmentBody::Unknown`].
    pub attachment: AttachmentBody,
    /// Catch-all for envelope-level fields not enumerated above.
    #[serde(flatten)]
    pub overflow: HashMap<String, serde_json::Value>,
}

/// Inner discriminated body of an `AttachmentRecord`.
///
/// The 12 modeled variants correspond exactly to the table in plan §C1.1.
/// The [`AttachmentBody::Unknown`] variant is the inner-discriminator
/// equivalent of [`JSONLRecord::Unknown`] — it preserves the unmodeled
/// subtype name and the full raw body so future promotion has a
/// representative sample. The dispatch is implemented by a hand-rolled
/// `Deserialize` impl below mirroring the two-pass pattern on `JSONLRecord`.
#[derive(Debug, Clone)]
pub enum AttachmentBody {
    HookSuccess(HookSuccessBody),
    HookPermissionDecision(HookPermissionDecisionBody),
    McpInstructionsDelta(McpInstructionsDeltaBody),
    SkillListing(SkillListingBody),
    EditedTextFile(EditedTextFileBody),
    TaskReminder(TaskReminderBody),
    TodoReminder(TodoReminderBody),
    DeferredToolsDelta(DeferredToolsDeltaBody),
    PlanMode(PlanModeBody),
    PlanModeExit(PlanModeExitBody),
    PlanModeReentry(PlanModeBody),
    NestedMemory(NestedMemoryBody),
    /// Inner-discriminator catch-all. Preserves the unmodeled subtype name
    /// and the raw body JSON so downstream forensic logging can record the
    /// drop into `record_type_drift_log` with `record_type =
    /// "attachment.<subtype>"`. This is the C1.1 application of the Path-A
    /// pattern at the inner level: an unmodeled inner subtype should not
    /// fail the parent record's deserialization.
    Unknown {
        subtype: String,
        raw: serde_json::Value,
    },
}

/// Discriminator value -> typed inner-body dispatch. Mirrors
/// [`known_record_type`] but for the 12 modeled subtypes.
fn known_attachment_subtype(subtype: &str) -> Option<KnownAttachmentSubtype> {
    match subtype {
        "hook_success" => Some(KnownAttachmentSubtype::HookSuccess),
        "hook_permission_decision" => Some(KnownAttachmentSubtype::HookPermissionDecision),
        "mcp_instructions_delta" => Some(KnownAttachmentSubtype::McpInstructionsDelta),
        "skill_listing" => Some(KnownAttachmentSubtype::SkillListing),
        "edited_text_file" => Some(KnownAttachmentSubtype::EditedTextFile),
        "task_reminder" => Some(KnownAttachmentSubtype::TaskReminder),
        "todo_reminder" => Some(KnownAttachmentSubtype::TodoReminder),
        "deferred_tools_delta" => Some(KnownAttachmentSubtype::DeferredToolsDelta),
        "plan_mode" => Some(KnownAttachmentSubtype::PlanMode),
        "plan_mode_exit" => Some(KnownAttachmentSubtype::PlanModeExit),
        "plan_mode_reentry" => Some(KnownAttachmentSubtype::PlanModeReentry),
        "nested_memory" => Some(KnownAttachmentSubtype::NestedMemory),
        _ => None,
    }
}

#[derive(Debug, Clone, Copy)]
enum KnownAttachmentSubtype {
    HookSuccess,
    HookPermissionDecision,
    McpInstructionsDelta,
    SkillListing,
    EditedTextFile,
    TaskReminder,
    TodoReminder,
    DeferredToolsDelta,
    PlanMode,
    PlanModeExit,
    PlanModeReentry,
    NestedMemory,
}

impl<'de> Deserialize<'de> for AttachmentBody {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        // Two-pass: capture the body as a raw Value, inspect `type`, dispatch
        // into the matching subtype struct via `from_value`. An unmodeled
        // subtype falls through to `AttachmentBody::Unknown` capturing both
        // the subtype name and the full raw body. A missing or non-string
        // `type` field is an error — same contract as JSONLRecord above.
        let value = serde_json::Value::deserialize(deserializer)?;
        let type_field = value
            .get("type")
            .ok_or_else(|| de::Error::missing_field("type"))?;
        let subtype = type_field.as_str().ok_or_else(|| {
            de::Error::invalid_type(
                de::Unexpected::Other(&format!("non-string attachment type: {type_field}")),
                &"a string-valued attachment `type` discriminator",
            )
        })?;

        if let Some(kind) = known_attachment_subtype(subtype) {
            // Strip `type` before dispatch so it does not leak into a
            // subtype struct's `#[serde(flatten)] overflow`. Mirrors the
            // strip-before-flatten guard in JSONLRecord's manual Deserialize.
            let mut typed_value = value;
            if let Some(map) = typed_value.as_object_mut() {
                map.remove("type");
            }
            return match kind {
                KnownAttachmentSubtype::HookSuccess => {
                    serde_json::from_value::<HookSuccessBody>(typed_value)
                        .map(AttachmentBody::HookSuccess)
                        .map_err(de::Error::custom)
                }
                KnownAttachmentSubtype::HookPermissionDecision => {
                    serde_json::from_value::<HookPermissionDecisionBody>(typed_value)
                        .map(AttachmentBody::HookPermissionDecision)
                        .map_err(de::Error::custom)
                }
                KnownAttachmentSubtype::McpInstructionsDelta => {
                    serde_json::from_value::<McpInstructionsDeltaBody>(typed_value)
                        .map(AttachmentBody::McpInstructionsDelta)
                        .map_err(de::Error::custom)
                }
                KnownAttachmentSubtype::SkillListing => {
                    serde_json::from_value::<SkillListingBody>(typed_value)
                        .map(AttachmentBody::SkillListing)
                        .map_err(de::Error::custom)
                }
                KnownAttachmentSubtype::EditedTextFile => {
                    serde_json::from_value::<EditedTextFileBody>(typed_value)
                        .map(AttachmentBody::EditedTextFile)
                        .map_err(de::Error::custom)
                }
                KnownAttachmentSubtype::TaskReminder => {
                    serde_json::from_value::<TaskReminderBody>(typed_value)
                        .map(AttachmentBody::TaskReminder)
                        .map_err(de::Error::custom)
                }
                KnownAttachmentSubtype::TodoReminder => {
                    serde_json::from_value::<TodoReminderBody>(typed_value)
                        .map(AttachmentBody::TodoReminder)
                        .map_err(de::Error::custom)
                }
                KnownAttachmentSubtype::DeferredToolsDelta => {
                    serde_json::from_value::<DeferredToolsDeltaBody>(typed_value)
                        .map(AttachmentBody::DeferredToolsDelta)
                        .map_err(de::Error::custom)
                }
                KnownAttachmentSubtype::PlanMode => {
                    serde_json::from_value::<PlanModeBody>(typed_value)
                        .map(AttachmentBody::PlanMode)
                        .map_err(de::Error::custom)
                }
                KnownAttachmentSubtype::PlanModeExit => {
                    serde_json::from_value::<PlanModeExitBody>(typed_value)
                        .map(AttachmentBody::PlanModeExit)
                        .map_err(de::Error::custom)
                }
                KnownAttachmentSubtype::PlanModeReentry => {
                    serde_json::from_value::<PlanModeBody>(typed_value)
                        .map(AttachmentBody::PlanModeReentry)
                        .map_err(de::Error::custom)
                }
                KnownAttachmentSubtype::NestedMemory => {
                    serde_json::from_value::<NestedMemoryBody>(typed_value)
                        .map(AttachmentBody::NestedMemory)
                        .map_err(de::Error::custom)
                }
            };
        }

        Ok(AttachmentBody::Unknown {
            subtype: subtype.to_string(),
            raw: value,
        })
    }
}

impl Serialize for AttachmentBody {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        // For typed variants, build a JSON object from the inner struct and
        // insert the `type` discriminator. For Unknown, serialize the raw
        // Value directly — it already includes the original `type` field.
        match self {
            AttachmentBody::HookSuccess(b) => insert_type_and_serialize(b, "hook_success", serializer),
            AttachmentBody::HookPermissionDecision(b) => {
                insert_type_and_serialize(b, "hook_permission_decision", serializer)
            }
            AttachmentBody::McpInstructionsDelta(b) => {
                insert_type_and_serialize(b, "mcp_instructions_delta", serializer)
            }
            AttachmentBody::SkillListing(b) => insert_type_and_serialize(b, "skill_listing", serializer),
            AttachmentBody::EditedTextFile(b) => {
                insert_type_and_serialize(b, "edited_text_file", serializer)
            }
            AttachmentBody::TaskReminder(b) => insert_type_and_serialize(b, "task_reminder", serializer),
            AttachmentBody::TodoReminder(b) => insert_type_and_serialize(b, "todo_reminder", serializer),
            AttachmentBody::DeferredToolsDelta(b) => {
                insert_type_and_serialize(b, "deferred_tools_delta", serializer)
            }
            AttachmentBody::PlanMode(b) => insert_type_and_serialize(b, "plan_mode", serializer),
            AttachmentBody::PlanModeExit(b) => insert_type_and_serialize(b, "plan_mode_exit", serializer),
            AttachmentBody::PlanModeReentry(b) => {
                insert_type_and_serialize(b, "plan_mode_reentry", serializer)
            }
            AttachmentBody::NestedMemory(b) => insert_type_and_serialize(b, "nested_memory", serializer),
            AttachmentBody::Unknown { raw, .. } => raw.serialize(serializer),
        }
    }
}

/// `hook_success` body — populated for every hook execution that the daemon
/// captures. Joinable to `tool_executions.tool_use_id` via `tool_use_id`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HookSuccessBody {
    pub hook_name: String,
    #[serde(rename = "toolUseID")]
    pub tool_use_id: String,
    pub hook_event: String,
    #[serde(default)]
    pub content: Option<String>,
    #[serde(default)]
    pub stdout: Option<String>,
    #[serde(default)]
    pub stderr: Option<String>,
    #[serde(default)]
    pub exit_code: Option<i64>,
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub duration_ms: Option<i64>,
    #[serde(flatten)]
    pub overflow: HashMap<String, serde_json::Value>,
}

/// `hook_permission_decision` body — emitted for `PermissionRequest`-style
/// hook events. `decision` is typically `"allow"` or `"deny"`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HookPermissionDecisionBody {
    pub decision: String,
    #[serde(rename = "toolUseID")]
    pub tool_use_id: String,
    pub hook_event: String,
    #[serde(flatten)]
    pub overflow: HashMap<String, serde_json::Value>,
}

/// `mcp_instructions_delta` body. Fields beyond the modeled set
/// (`removedNames` observed in the corpus) ride in overflow for forensic
/// recoverability without expanding the modeled schema.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpInstructionsDeltaBody {
    #[serde(default)]
    pub added_names: Vec<String>,
    #[serde(default)]
    pub added_blocks: Vec<String>,
    #[serde(flatten)]
    pub overflow: HashMap<String, serde_json::Value>,
}

/// `skill_listing` body — newline-joined enumeration of the agent's available
/// skills. Additional fields (`skillCount`, `isInitial`) ride in overflow.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillListingBody {
    pub content: String,
    #[serde(flatten)]
    pub overflow: HashMap<String, serde_json::Value>,
}

/// `edited_text_file` body — captures the line-numbered snippet shown to the
/// agent after a file edit operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EditedTextFileBody {
    pub filename: String,
    pub snippet: String,
    #[serde(flatten)]
    pub overflow: HashMap<String, serde_json::Value>,
}

/// `task_reminder` body. `content` is `serde_json::Value` because the field
/// is observed empty (`[]`) on most records but is shaped as a list of task
/// objects when populated. Plan-table-driven typing.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskReminderBody {
    pub content: serde_json::Value,
    pub item_count: i64,
    #[serde(flatten)]
    pub overflow: HashMap<String, serde_json::Value>,
}

/// `todo_reminder` body — same shape as `task_reminder`. Modeled as a
/// distinct struct because future schema changes may differentiate them and
/// rusqlite-row binding becomes simpler with concrete types.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TodoReminderBody {
    pub content: serde_json::Value,
    pub item_count: i64,
    #[serde(flatten)]
    pub overflow: HashMap<String, serde_json::Value>,
}

/// `deferred_tools_delta` body. Like `mcp_instructions_delta`, additional
/// fields observed in the corpus (`addedLines`, `removedNames`,
/// `readdedNames`) ride in overflow.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeferredToolsDeltaBody {
    #[serde(default)]
    pub added_names: Vec<String>,
    #[serde(flatten)]
    pub overflow: HashMap<String, serde_json::Value>,
}

/// `plan_mode` body, also reused for `plan_mode_reentry` per plan §C1.1
/// ("same shape as plan_mode"). Both `reminderType` and `isSubAgent` are
/// optional because some corpus samples carry them and some do not.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanModeBody {
    pub plan_file_path: String,
    #[serde(default)]
    pub plan_exists: Option<bool>,
    #[serde(default)]
    pub reminder_type: Option<String>,
    #[serde(default)]
    pub is_sub_agent: Option<bool>,
    #[serde(flatten)]
    pub overflow: HashMap<String, serde_json::Value>,
}

/// `plan_mode_exit` body. Both fields are optional per the plan table; in
/// the corpus survey they are typically present but the spec marks them so.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanModeExitBody {
    #[serde(default)]
    pub plan_file_path: Option<String>,
    #[serde(default)]
    pub plan_exists: Option<bool>,
    #[serde(flatten)]
    pub overflow: HashMap<String, serde_json::Value>,
}

/// Inner content object inside a `nested_memory.content` payload.
/// Captures the path/type/content triple plus any unmodeled fields
/// (`contentDiffersFromDisk` was observed) into overflow.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NestedMemoryContent {
    pub path: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub content: String,
    #[serde(flatten)]
    pub overflow: HashMap<String, serde_json::Value>,
}

/// `nested_memory` body. Wraps `NestedMemoryContent` so the inner triple
/// (path/type/content) is queryable directly.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NestedMemoryBody {
    pub path: String,
    pub content: NestedMemoryContent,
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

    /// Test B-B-2: the five session-metadata unknown discriminators from the
    /// corpus survey (last-prompt, custom-title, permission-mode, agent-name,
    /// ai-title) continue to deserialize to JSONLRecord::Unknown after C1.1.
    /// `attachment` was the sixth member in the original B1.1 survey but is
    /// now promoted to a typed variant via C1.1 — see
    /// test_attachment_discriminator_no_longer_unknown above.
    #[test]
    fn test_unknown_variant_known_corpus_discriminators() {
        let cases: &[(&str, &str)] = &[
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

    // -----------------------------------------------------------------------
    // C1.1 — AttachmentRecord + AttachmentBody tests
    //
    // One test per modeled subtype (12 total) exercises real-corpus JSON
    // bodies sampled from /tmp/c1.1-samples-<subtype>.json (kept inline so
    // the regression net does not depend on any out-of-tree fixture). Plus:
    // - Unknown subtype falls through to AttachmentBody::Unknown
    // - Attachment Serialize round-trip
    // - Missing required field on a modeled subtype falls through to Unknown
    // - "attachment" as JSONLRecord discriminator dispatches to
    //   JSONLRecord::Attachment, NOT JSONLRecord::Unknown (regression check
    //   that the dispatch table now includes attachment).
    // -----------------------------------------------------------------------

    /// Helper: assert that a JSONL line parses to JSONLRecord::Attachment and
    /// returns the body for further inspection.
    fn parse_attachment(json: &str) -> AttachmentBody {
        let record: JSONLRecord = serde_json::from_str(json)
            .unwrap_or_else(|e| panic!("attachment record should parse: {e}\nJSON: {json}"));
        match record {
            JSONLRecord::Attachment(r) => r.attachment,
            other => panic!("expected JSONLRecord::Attachment, got {other:?}"),
        }
    }

    #[test]
    fn test_attachment_hook_success() {
        let json = r#"{
            "type": "attachment",
            "uuid": "att-hs-001",
            "timestamp": "2026-05-04T01:39:46.095Z",
            "sessionId": "sess-c11",
            "version": "2.1.126",
            "cwd": "/Users/david/Projects/cc-history-api",
            "gitBranch": "main",
            "slug": "curious-napping-koala",
            "entrypoint": "cli",
            "parentUuid": "parent-001",
            "userType": "external",
            "isSidechain": false,
            "attachment": {
                "type": "hook_success",
                "hookName": "PostToolUse:Bash",
                "toolUseID": "toolu_01abc",
                "hookEvent": "PostToolUse",
                "content": "",
                "stdout": "{}\n",
                "stderr": "",
                "exitCode": 0,
                "command": "python3 hook.py",
                "durationMs": 27
            }
        }"#;
        match parse_attachment(json) {
            AttachmentBody::HookSuccess(b) => {
                assert_eq!(b.hook_name, "PostToolUse:Bash");
                assert_eq!(b.tool_use_id, "toolu_01abc");
                assert_eq!(b.hook_event, "PostToolUse");
                assert_eq!(b.exit_code, Some(0));
                assert_eq!(b.duration_ms, Some(27));
                assert_eq!(b.command.as_deref(), Some("python3 hook.py"));
            }
            other => panic!("expected HookSuccess, got {other:?}"),
        }
    }

    #[test]
    fn test_attachment_hook_permission_decision() {
        let json = r#"{
            "type": "attachment",
            "uuid": "att-hpd-001",
            "timestamp": "2026-05-04T01:39:51.066Z",
            "sessionId": "sess-c11",
            "version": "2.1.126",
            "cwd": "/Users/david/Projects/cc-history-api",
            "gitBranch": "main",
            "attachment": {
                "type": "hook_permission_decision",
                "decision": "allow",
                "toolUseID": "toolu_01G58KeXWvV1cvo1BENqqX9b",
                "hookEvent": "PermissionRequest"
            }
        }"#;
        match parse_attachment(json) {
            AttachmentBody::HookPermissionDecision(b) => {
                assert_eq!(b.decision, "allow");
                assert_eq!(b.tool_use_id, "toolu_01G58KeXWvV1cvo1BENqqX9b");
                assert_eq!(b.hook_event, "PermissionRequest");
            }
            other => panic!("expected HookPermissionDecision, got {other:?}"),
        }
    }

    #[test]
    fn test_attachment_mcp_instructions_delta() {
        // `removedNames` is observed in real corpus records — it lands in
        // overflow, since the plan's modeled set is just (addedNames, addedBlocks).
        let json = r###"{
            "type": "attachment",
            "uuid": "att-mid-001",
            "timestamp": "2026-05-04T01:00:00.000Z",
            "sessionId": "sess-c11",
            "version": "2.1.126",
            "attachment": {
                "type": "mcp_instructions_delta",
                "addedNames": ["claude-history"],
                "addedBlocks": ["## claude-history\n..."],
                "removedNames": []
            }
        }"###;
        match parse_attachment(json) {
            AttachmentBody::McpInstructionsDelta(b) => {
                assert_eq!(b.added_names, vec!["claude-history".to_string()]);
                assert_eq!(b.added_blocks.len(), 1);
                assert!(
                    b.overflow.contains_key("removedNames"),
                    "removedNames must ride in overflow"
                );
            }
            other => panic!("expected McpInstructionsDelta, got {other:?}"),
        }
    }

    #[test]
    fn test_attachment_skill_listing() {
        let json = r#"{
            "type": "attachment",
            "uuid": "att-sl-001",
            "timestamp": "2026-05-04T01:00:00.000Z",
            "sessionId": "sess-c11",
            "version": "2.1.126",
            "attachment": {
                "type": "skill_listing",
                "content": "- update-config: ...",
                "skillCount": 171,
                "isInitial": true
            }
        }"#;
        match parse_attachment(json) {
            AttachmentBody::SkillListing(b) => {
                assert!(b.content.starts_with("- update-config"));
                assert!(b.overflow.contains_key("skillCount"));
                assert!(b.overflow.contains_key("isInitial"));
            }
            other => panic!("expected SkillListing, got {other:?}"),
        }
    }

    #[test]
    fn test_attachment_edited_text_file() {
        let json = r#"{
            "type": "attachment",
            "uuid": "att-etf-001",
            "timestamp": "2026-05-04T01:00:00.000Z",
            "sessionId": "sess-c11",
            "version": "2.1.126",
            "attachment": {
                "type": "edited_text_file",
                "filename": "/path/to/file.ts",
                "snippet": "1\tline one\n2\tline two\n"
            }
        }"#;
        match parse_attachment(json) {
            AttachmentBody::EditedTextFile(b) => {
                assert_eq!(b.filename, "/path/to/file.ts");
                assert!(b.snippet.contains("line one"));
            }
            other => panic!("expected EditedTextFile, got {other:?}"),
        }
    }

    #[test]
    fn test_attachment_task_reminder() {
        let json = r#"{
            "type": "attachment",
            "uuid": "att-tr-001",
            "timestamp": "2026-05-04T01:39:46.095Z",
            "sessionId": "sess-c11",
            "version": "2.1.126",
            "attachment": {
                "type": "task_reminder",
                "content": [],
                "itemCount": 0
            }
        }"#;
        match parse_attachment(json) {
            AttachmentBody::TaskReminder(b) => {
                assert!(b.content.is_array());
                assert_eq!(b.item_count, 0);
            }
            other => panic!("expected TaskReminder, got {other:?}"),
        }
    }

    #[test]
    fn test_attachment_todo_reminder() {
        let json = r#"{
            "type": "attachment",
            "uuid": "att-todor-001",
            "timestamp": "2026-04-24T23:31:24.984Z",
            "sessionId": "sess-c11",
            "version": "2.1.119",
            "attachment": {
                "type": "todo_reminder",
                "content": [],
                "itemCount": 0
            }
        }"#;
        match parse_attachment(json) {
            AttachmentBody::TodoReminder(b) => {
                assert!(b.content.is_array());
                assert_eq!(b.item_count, 0);
            }
            other => panic!("expected TodoReminder, got {other:?}"),
        }
    }

    #[test]
    fn test_attachment_deferred_tools_delta() {
        // Real corpus records carry `addedLines`, `removedNames`,
        // `readdedNames` in addition to the modeled `addedNames` — they
        // ride in overflow.
        let json = r#"{
            "type": "attachment",
            "uuid": "att-dtd-001",
            "timestamp": "2026-05-04T01:00:00.000Z",
            "sessionId": "sess-c11",
            "version": "2.1.126",
            "attachment": {
                "type": "deferred_tools_delta",
                "addedNames": ["CronCreate", "CronDelete"],
                "addedLines": ["CronCreate", "CronDelete"],
                "removedNames": [],
                "readdedNames": []
            }
        }"#;
        match parse_attachment(json) {
            AttachmentBody::DeferredToolsDelta(b) => {
                assert_eq!(b.added_names.len(), 2);
                assert!(b.overflow.contains_key("addedLines"));
                assert!(b.overflow.contains_key("removedNames"));
                assert!(b.overflow.contains_key("readdedNames"));
            }
            other => panic!("expected DeferredToolsDelta, got {other:?}"),
        }
    }

    #[test]
    fn test_attachment_plan_mode() {
        let json = r#"{
            "type": "attachment",
            "uuid": "att-pm-001",
            "timestamp": "2026-04-25T03:42:34.373Z",
            "sessionId": "sess-c11",
            "version": "2.1.119",
            "attachment": {
                "type": "plan_mode",
                "reminderType": "full",
                "isSubAgent": false,
                "planFilePath": "/Users/david/.claude/plans/x.md",
                "planExists": false
            }
        }"#;
        match parse_attachment(json) {
            AttachmentBody::PlanMode(b) => {
                assert_eq!(b.plan_file_path, "/Users/david/.claude/plans/x.md");
                assert_eq!(b.plan_exists, Some(false));
                assert_eq!(b.reminder_type.as_deref(), Some("full"));
                assert_eq!(b.is_sub_agent, Some(false));
            }
            other => panic!("expected PlanMode, got {other:?}"),
        }
    }

    #[test]
    fn test_attachment_plan_mode_exit() {
        let json = r#"{
            "type": "attachment",
            "uuid": "att-pme-001",
            "timestamp": "2026-05-07T21:43:31.576Z",
            "sessionId": "sess-c11",
            "version": "2.1.126",
            "attachment": {
                "type": "plan_mode_exit",
                "planFilePath": "/Users/david/.claude/plans/y.md",
                "planExists": true
            }
        }"#;
        match parse_attachment(json) {
            AttachmentBody::PlanModeExit(b) => {
                assert_eq!(b.plan_file_path.as_deref(), Some("/Users/david/.claude/plans/y.md"));
                assert_eq!(b.plan_exists, Some(true));
            }
            other => panic!("expected PlanModeExit, got {other:?}"),
        }
    }

    #[test]
    fn test_attachment_plan_mode_reentry() {
        // plan_mode_reentry shares the PlanModeBody shape per plan §C1.1.
        let json = r#"{
            "type": "attachment",
            "uuid": "att-pmr-001",
            "timestamp": "2026-05-08T11:09:20.870Z",
            "sessionId": "sess-c11",
            "version": "2.1.126",
            "attachment": {
                "type": "plan_mode_reentry",
                "planFilePath": "/Users/david/.claude/plans/z.md"
            }
        }"#;
        match parse_attachment(json) {
            AttachmentBody::PlanModeReentry(b) => {
                assert_eq!(b.plan_file_path, "/Users/david/.claude/plans/z.md");
                assert_eq!(b.plan_exists, None);
            }
            other => panic!("expected PlanModeReentry, got {other:?}"),
        }
    }

    #[test]
    fn test_attachment_nested_memory() {
        let json = r##"{
            "type": "attachment",
            "uuid": "att-nm-001",
            "timestamp": "2026-05-04T01:00:00.000Z",
            "sessionId": "sess-c11",
            "version": "2.1.126",
            "attachment": {
                "type": "nested_memory",
                "path": "/tmp/CLAUDE.md",
                "content": {
                    "path": "/tmp/CLAUDE.md",
                    "type": "Project",
                    "content": "# memo\n",
                    "contentDiffersFromDisk": false
                }
            }
        }"##;
        match parse_attachment(json) {
            AttachmentBody::NestedMemory(b) => {
                assert_eq!(b.path, "/tmp/CLAUDE.md");
                assert_eq!(b.content.path, "/tmp/CLAUDE.md");
                assert_eq!(b.content.kind, "Project");
                assert_eq!(b.content.content, "# memo\n");
                assert!(b.content.overflow.contains_key("contentDiffersFromDisk"));
            }
            other => panic!("expected NestedMemory, got {other:?}"),
        }
    }

    /// An unknown inner subtype must fall through to AttachmentBody::Unknown
    /// rather than failing the parent record's deserialization. This is the
    /// Path-A pattern applied at the inner discriminator level.
    #[test]
    fn test_attachment_unknown_subtype_falls_through() {
        let json = r#"{
            "type": "attachment",
            "uuid": "att-unk-001",
            "timestamp": "2026-05-04T01:00:00.000Z",
            "sessionId": "sess-c11",
            "version": "2.1.126",
            "attachment": {
                "type": "date_change",
                "previous": "2026-05-03",
                "current": "2026-05-04"
            }
        }"#;
        match parse_attachment(json) {
            AttachmentBody::Unknown { subtype, raw } => {
                assert_eq!(subtype, "date_change");
                assert_eq!(raw.get("previous").and_then(|v| v.as_str()), Some("2026-05-03"));
                assert_eq!(raw.get("current").and_then(|v| v.as_str()), Some("2026-05-04"));
                assert_eq!(
                    raw.get("type").and_then(|v| v.as_str()),
                    Some("date_change"),
                    "raw must retain inner discriminator"
                );
            }
            other => panic!("expected AttachmentBody::Unknown, got {other:?}"),
        }
    }

    /// Serialize round-trip: an Attachment that parses then re-serializes
    /// must re-parse to the same variant. We don't byte-compare the JSON
    /// (HashMap iteration is non-deterministic) but the structural identity
    /// is what downstream consumers rely on.
    #[test]
    fn test_attachment_serialize_roundtrip() {
        let json = r#"{
            "type": "attachment",
            "uuid": "att-rt-001",
            "timestamp": "2026-05-04T01:00:00.000Z",
            "sessionId": "sess-rt",
            "version": "2.1.126",
            "attachment": {
                "type": "hook_success",
                "hookName": "Stop",
                "toolUseID": "tu-rt",
                "hookEvent": "Stop",
                "exitCode": 0,
                "durationMs": 5
            }
        }"#;
        let parsed: JSONLRecord = serde_json::from_str(json).unwrap();
        let serialized = serde_json::to_string(&parsed).unwrap();
        let reparsed: JSONLRecord = serde_json::from_str(&serialized).unwrap();
        match reparsed {
            JSONLRecord::Attachment(r) => {
                assert_eq!(r.uuid, "att-rt-001");
                assert_eq!(r.session_id, "sess-rt");
                match r.attachment {
                    AttachmentBody::HookSuccess(b) => {
                        assert_eq!(b.hook_name, "Stop");
                        assert_eq!(b.exit_code, Some(0));
                    }
                    other => panic!("expected HookSuccess after roundtrip, got {other:?}"),
                }
            }
            other => panic!("expected Attachment after roundtrip, got {other:?}"),
        }
        // Outer discriminator must survive the roundtrip.
        let value: serde_json::Value = serde_json::from_str(&serialized).unwrap();
        assert_eq!(value["type"], "attachment");
        assert_eq!(value["attachment"]["type"], "hook_success");
    }

    /// A modeled subtype with a missing required field (e.g. `hook_success`
    /// without `hookName`) should fall through to AttachmentBody::Unknown
    /// rather than failing the whole parent record. The plan §C1.1 framing:
    /// "getting any single subtype's struct shape wrong on first encounter
    /// will silently dump the record into the unknown-subtype catch-all" —
    /// this test locks in that behavior.
    #[test]
    fn test_attachment_modeled_subtype_missing_field_falls_through() {
        let json = r#"{
            "type": "attachment",
            "uuid": "att-bad-001",
            "timestamp": "2026-05-04T01:00:00.000Z",
            "sessionId": "sess-bad",
            "version": "2.1.126",
            "attachment": {
                "type": "hook_success",
                "toolUseID": "tu-bad",
                "hookEvent": "Stop"
            }
        }"#;
        let result: Result<JSONLRecord, _> = serde_json::from_str(json);
        // The behavior we want for C1.1: a missing required field on a
        // modeled subtype currently surfaces as a parse error from the typed
        // dispatch (because hook_name has no #[serde(default)]). The Path-A
        // fall-through happens at the *outer* level for unknown discriminators.
        // For modeled-but-malformed inner bodies the contract is "parse error
        // bubbles up" — distinct from the inner-discriminator catch-all.
        // This test documents that asymmetry so future review knows the
        // failure mode is intentional.
        assert!(
            result.is_err(),
            "modeled subtype missing required field should error, not silently fall through"
        );
    }

    /// Regression check: the JSONLRecord-level dispatch for "attachment"
    /// must now route to JSONLRecord::Attachment, NOT JSONLRecord::Unknown.
    /// Pre-C1.1, B1.1's test_unknown_variant_known_corpus_discriminators
    /// asserted "attachment" hit Unknown. C1.1 promotes it; that prior
    /// behavior must change here.
    #[test]
    fn test_attachment_discriminator_no_longer_unknown() {
        let json = r#"{
            "type": "attachment",
            "uuid": "att-promo-001",
            "timestamp": "2026-05-04T01:00:00.000Z",
            "sessionId": "sess-promo",
            "version": "2.1.126",
            "attachment": {"type": "hook_success", "hookName": "h", "toolUseID": "tu", "hookEvent": "Stop"}
        }"#;
        let record: JSONLRecord = serde_json::from_str(json).unwrap();
        assert!(
            matches!(record, JSONLRecord::Attachment(_)),
            "attachment discriminator must now route to Attachment, not Unknown"
        );
    }

    /// Regression check: non-attachment unknown discriminators (e.g.
    /// `last-prompt`) must still fall through to JSONLRecord::Unknown after
    /// C1.1, since C1.1 only promoted `attachment`.
    #[test]
    fn test_non_attachment_unknown_still_falls_through_post_c11() {
        let cases: &[&str] = &["last-prompt", "custom-title", "permission-mode", "agent-name", "ai-title"];
        for type_name in cases {
            let json = format!(r#"{{"type":"{type_name}","sessionId":"s","foo":"bar"}}"#);
            let record: JSONLRecord = serde_json::from_str(&json)
                .unwrap_or_else(|e| panic!("{type_name} should parse: {e}"));
            match record {
                JSONLRecord::Unknown { type_name: tn, .. } => assert_eq!(tn, *type_name),
                other => panic!("expected Unknown for {type_name}, got {other:?}"),
            }
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
