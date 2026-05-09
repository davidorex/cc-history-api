-- Migration 008: attachments + hook_executions tables
--
-- Structural foundation for typed AttachmentRecord ingestion. Migration 007
-- (record_type_drift_log) and B1.1 (JSONLRecord::Unknown variant) closed the
-- silent-drop blind spot at the parent-record level. C1.1 closes it at the
-- inner-discriminator level for the `attachment` record type:
--
--   - JSONLRecord::Attachment is now a typed variant (crates/core/src/record.rs)
--   - AttachmentBody enumerates 12 modeled subtypes covering ~97% of attachment
--     records by volume (~9,800 of 10,085 observed at investigation time)
--   - Inner subtypes outside the modeled set fall through to
--     AttachmentBody::Unknown and log to record_type_drift_log with
--     record_type = "attachment.<subtype>" via existing drift infrastructure
--
-- C1.1 only creates the destination tables. The decomposer routing that
-- populates them lands in C1.2 — see decompose_attachment in
-- crates/store/src/decompose.rs (currently a stub that logs to drift only).
--
-- Two tables:
--
--   1. `attachments` — uuid-keyed envelope mirroring the messages table shape.
--      `inner_type` is the AttachmentBody discriminator string. `body_json`
--      holds the raw inner body for unmodeled subtypes (or, optionally, for
--      modeled subtypes' overflow fields — a C1.2 design decision).
--
--   2. `hook_executions` — flat row-per-hook-execution serving the two hook
--      subtypes (`hook_success` + `hook_permission_decision`). One table
--      because the two share the (toolUseID, hookEvent) join shape; subtype
--      is recoverable via attachment_uuid -> attachments.inner_type. The
--      `tool_use_id` column is indexed to enable joins to
--      `tool_executions.tool_use_id`. For UserPromptSubmit/Stop hooks
--      (corpus open question 3 in the audit doc), `tool_use_id` is a
--      synthetic correlation ID — not a tool_executions row — so the join
--      is naturally a LEFT JOIN.
--
-- All DDL is IF NOT EXISTS so the migration is replay-safe per safeguard 6.
-- The runner at crates/store/src/schema.rs gates application via
-- schema_versions, so the IF NOT EXISTS guards are belt-and-suspenders.
--
-- Foreign keys:
--   - attachments.session_id -> sessions.session_id (existing pattern)
--   - hook_executions.attachment_uuid -> attachments.uuid (intra-migration FK)
-- Both rely on PRAGMA foreign_keys = ON which the daemon sets on connection
-- open (see crates/store/src/db.rs init_db).

CREATE TABLE IF NOT EXISTS attachments (
    uuid          TEXT PRIMARY KEY,
    session_id    TEXT NOT NULL,
    parent_uuid   TEXT,
    timestamp     TEXT NOT NULL,
    cwd           TEXT,
    version       TEXT,
    git_branch    TEXT,
    slug          TEXT,
    entrypoint    TEXT,
    inner_type    TEXT NOT NULL,
    body_json     TEXT,
    FOREIGN KEY (session_id) REFERENCES sessions(session_id)
);

CREATE INDEX IF NOT EXISTS idx_attachments_session_id
    ON attachments(session_id);

CREATE INDEX IF NOT EXISTS idx_attachments_inner_type
    ON attachments(inner_type);

CREATE INDEX IF NOT EXISTS idx_attachments_timestamp
    ON attachments(timestamp);

CREATE TABLE IF NOT EXISTS hook_executions (
    id               INTEGER PRIMARY KEY AUTOINCREMENT,
    attachment_uuid  TEXT NOT NULL,
    hook_name        TEXT,
    hook_event       TEXT,
    tool_use_id      TEXT,
    exit_code        INTEGER,
    duration_ms      INTEGER,
    stdout           TEXT,
    stderr           TEXT,
    command          TEXT,
    decision         TEXT,
    FOREIGN KEY (attachment_uuid) REFERENCES attachments(uuid)
);

CREATE INDEX IF NOT EXISTS idx_hook_executions_tool_use_id
    ON hook_executions(tool_use_id);

CREATE INDEX IF NOT EXISTS idx_hook_executions_attachment_uuid
    ON hook_executions(attachment_uuid);

CREATE INDEX IF NOT EXISTS idx_hook_executions_hook_event
    ON hook_executions(hook_event);
