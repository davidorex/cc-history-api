-- Migration 006: Version monitoring foundation
--
-- This migration addresses three concerns:
--   1. Version tracking:   version_history table for persistent tracking of Claude Code
--                          versions observed across sessions, backfilled from sessions
--                          and correlated with schema_drift_log for new_fields_count.
--   2. Message enrichment: Promotes compact summary detection and tool-use provenance
--                          to real columns on messages (is_compact_summary,
--                          source_tool_use_id, extra_json) for future decomposer use.
--   3. Drift occurrence:   Adds occurrence_count and last_seen_at to schema_drift_log
--                          for tracking how often and how recently drift fields appear.
--
-- All 7 analytical views are recreated with is_compact_summary = 0 filtering so that
-- compact summary messages (which contain aggregated/synthetic content) do not skew
-- token counts, file provenance, or other analytical queries.
--
-- Requirement IDs: VER-02

--------------------------------------------------------------------------------
-- 1. version_history table
--------------------------------------------------------------------------------

CREATE TABLE IF NOT EXISTS version_history (
    version          TEXT PRIMARY KEY,
    first_seen_at    TEXT NOT NULL,
    last_seen_at     TEXT NOT NULL,
    session_id       TEXT,
    session_count    INTEGER NOT NULL DEFAULT 0,
    new_fields_count INTEGER NOT NULL DEFAULT 0
);

--------------------------------------------------------------------------------
-- 2. Backfill version_history from sessions
--------------------------------------------------------------------------------

INSERT OR IGNORE INTO version_history (version, first_seen_at, last_seen_at, session_id, session_count)
SELECT
    version,
    MIN(first_seen_at),
    MAX(COALESCE(last_seen_at, first_seen_at)),
    (SELECT s2.session_id FROM sessions s2 WHERE s2.version = sessions.version
     ORDER BY s2.first_seen_at ASC LIMIT 1),
    COUNT(*)
FROM sessions
WHERE version IS NOT NULL AND version != ''
GROUP BY version;

--------------------------------------------------------------------------------
-- 3. Backfill new_fields_count from schema_drift_log
--------------------------------------------------------------------------------

-- Count drift fields whose first appearance (by first_seen_at) is attributed to
-- this version — meaning no row with the same (field_name, record_type) exists
-- in schema_drift_log with an earlier first_seen_at tied to a different version.
UPDATE version_history
SET new_fields_count = (
    SELECT COUNT(*)
    FROM schema_drift_log sd
    WHERE sd.version = version_history.version
      AND NOT EXISTS (
          SELECT 1
          FROM schema_drift_log sd2
          WHERE sd2.field_name = sd.field_name
            AND sd2.record_type = sd.record_type
            AND sd2.first_seen_at < sd.first_seen_at
            AND sd2.version != sd.version
      )
);

--------------------------------------------------------------------------------
-- 4. Messages table enrichment columns
--------------------------------------------------------------------------------

ALTER TABLE messages ADD COLUMN is_compact_summary INTEGER DEFAULT 0;
ALTER TABLE messages ADD COLUMN source_tool_use_id TEXT;
ALTER TABLE messages ADD COLUMN extra_json TEXT;

--------------------------------------------------------------------------------
-- 5. schema_drift_log enhancement
--------------------------------------------------------------------------------

ALTER TABLE schema_drift_log ADD COLUMN occurrence_count INTEGER DEFAULT 1;
ALTER TABLE schema_drift_log ADD COLUMN last_seen_at TEXT;

-- Backfill last_seen_at from first_seen_at for existing rows
UPDATE schema_drift_log SET last_seen_at = first_seen_at WHERE last_seen_at IS NULL;

--------------------------------------------------------------------------------
-- 6. Recreate all 7 analytical views with is_compact_summary filtering
--------------------------------------------------------------------------------

-- v_file_token_cost: Per-file token cost attribution across all sessions
DROP VIEW IF EXISTS v_file_token_cost;
CREATE VIEW v_file_token_cost AS
SELECT
    s.project_path,
    fo.file_path,
    fo.operation_type,
    COUNT(*) as operation_count,
    COALESCE(SUM(tu.input_tokens), 0) as input_tokens,
    COALESCE(SUM(tu.output_tokens), 0) as output_tokens,
    COALESCE(SUM(tu.input_tokens + tu.output_tokens), 0) as total_tokens
FROM file_operations fo
JOIN messages m ON fo.message_uuid = m.uuid AND m.is_compact_summary = 0
JOIN sessions s ON m.session_id = s.session_id
LEFT JOIN token_usage tu ON m.uuid = tu.message_uuid
GROUP BY s.project_path, fo.file_path, fo.operation_type;

-- v_file_conversation_context: Conversation context around file mutations
DROP VIEW IF EXISTS v_file_conversation_context;
CREATE VIEW v_file_conversation_context AS
SELECT
    fo.file_path,
    fo.operation_type,
    fo.timestamp as operation_timestamp,
    fo.session_id,
    s.project_path,
    m_asst.uuid as assistant_message_uuid,
    mc.text_content as assistant_reasoning,
    m_asst.timestamp as reasoning_timestamp,
    m_asst.model
FROM file_operations fo
JOIN messages m_op ON fo.message_uuid = m_op.uuid AND m_op.is_compact_summary = 0
JOIN messages m_asst ON m_asst.session_id = m_op.session_id
    AND m_asst.type = 'assistant'
    AND m_asst.timestamp <= m_op.timestamp
    AND m_asst.timestamp >= datetime(m_op.timestamp, '-60 seconds')
JOIN message_content mc ON mc.message_uuid = m_asst.uuid
    AND mc.block_type = 'text'
JOIN sessions s ON fo.session_id = s.session_id
WHERE fo.operation_type IN ('write', 'edit');

-- v_project_summary: Project activity summary
DROP VIEW IF EXISTS v_project_summary;
CREATE VIEW v_project_summary AS
SELECT
    s.project_path,
    COUNT(DISTINCT s.session_id) as session_count,
    COUNT(DISTINCT m.uuid) as message_count,
    COALESCE(SUM(tu.input_tokens + tu.output_tokens), 0) as total_tokens,
    COUNT(DISTINCT fo.id) as file_operations,
    COUNT(DISTINCT go2.id) as git_operations,
    MIN(s.first_seen_at) as first_activity,
    MAX(m.timestamp) as last_activity
FROM sessions s
LEFT JOIN messages m ON m.session_id = s.session_id AND m.is_compact_summary = 0
LEFT JOIN token_usage tu ON tu.message_uuid = m.uuid
LEFT JOIN file_operations fo ON fo.session_id = s.session_id
LEFT JOIN git_operations go2 ON go2.session_id = s.session_id
GROUP BY s.project_path;

-- v_file_provenance: Complete file operation history across sessions
DROP VIEW IF EXISTS v_file_provenance;
CREATE VIEW v_file_provenance AS
SELECT
    fo.file_path,
    fo.operation_type,
    fo.timestamp,
    fo.session_id,
    s.project_path,
    fo.content,
    fo.old_content,
    fo.command,
    fo.result_summary,
    fo.is_error,
    fo.message_uuid,
    m.type as message_type
FROM file_operations fo
JOIN messages m ON fo.message_uuid = m.uuid AND m.is_compact_summary = 0
JOIN sessions s ON fo.session_id = s.session_id
ORDER BY fo.file_path, fo.timestamp;

-- v_git_commit_context: Git commit messages with surrounding assistant reasoning
DROP VIEW IF EXISTS v_git_commit_context;
CREATE VIEW v_git_commit_context AS
SELECT
    go2.commit_message,
    go2.branch,
    go2.timestamp as commit_timestamp,
    go2.session_id,
    s.project_path,
    mc.text_content as assistant_reasoning,
    m_asst.timestamp as reasoning_timestamp
FROM git_operations go2
JOIN messages m_commit ON go2.message_uuid = m_commit.uuid AND m_commit.is_compact_summary = 0
JOIN messages m_asst ON m_asst.session_id = m_commit.session_id
    AND m_asst.type = 'assistant'
    AND m_asst.timestamp <= m_commit.timestamp
    AND m_asst.timestamp >= datetime(m_commit.timestamp, '-120 seconds')
JOIN message_content mc ON mc.message_uuid = m_asst.uuid
    AND mc.block_type = 'text'
JOIN sessions s ON go2.session_id = s.session_id
WHERE go2.operation_type = 'commit';

-- v_tool_errors: Tool error patterns with conversation context
DROP VIEW IF EXISTS v_tool_errors;
CREATE VIEW v_tool_errors AS
SELECT
    te.tool_name,
    te.is_error,
    SUBSTR(te.result_content, 1, 500) as error_content,
    te.message_uuid,
    m.session_id,
    s.project_path,
    m.timestamp,
    te.input_json
FROM tool_executions te
JOIN messages m ON te.message_uuid = m.uuid AND m.is_compact_summary = 0
JOIN sessions s ON m.session_id = s.session_id
WHERE te.is_error = 1;

-- v_session_cost: Session cost breakdown
DROP VIEW IF EXISTS v_session_cost;
CREATE VIEW v_session_cost AS
SELECT
    s.session_id,
    s.project_path,
    s.first_seen_at,
    COUNT(DISTINCT m.uuid) as message_count,
    COALESCE(SUM(tu.input_tokens), 0) as input_tokens,
    COALESCE(SUM(tu.output_tokens), 0) as output_tokens,
    COALESCE(SUM(tu.cache_read_input_tokens), 0) as cache_read_tokens,
    COUNT(DISTINCT fo.id) as file_ops,
    COUNT(DISTINCT go2.id) as git_ops
FROM sessions s
LEFT JOIN messages m ON m.session_id = s.session_id AND m.is_compact_summary = 0
LEFT JOIN token_usage tu ON tu.message_uuid = m.uuid
LEFT JOIN file_operations fo ON fo.session_id = s.session_id
LEFT JOIN git_operations go2 ON go2.session_id = s.session_id
GROUP BY s.session_id;
