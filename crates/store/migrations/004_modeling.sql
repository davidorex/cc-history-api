-- Migration 004: Modeling layer — projects table, artifact enrichment columns, and 7 analytical views.
--
-- This migration adds:
--   1. projects table:          one row per unique project_path, backfilled from sessions
--   2. result_summary/is_error: enrichment columns on file_operations and git_operations,
--                               backfilled from tool_executions via tool_use_id JOIN
--   3. Missing index:           idx_file_operations_operation_type
--   4. Seven analytical views:  cross-domain relationships queryable by any consumer
--
-- Requirement IDs: M2-01 (projects), M2-02 (artifact columns), M2-03 (views)

--------------------------------------------------------------------------------
-- 1. Projects table
--------------------------------------------------------------------------------

CREATE TABLE IF NOT EXISTS projects (
    project_path  TEXT PRIMARY KEY,
    display_name  TEXT,
    first_seen    TEXT NOT NULL,
    last_seen     TEXT NOT NULL,
    session_count INTEGER NOT NULL DEFAULT 0
);

-- Backfill projects from sessions
INSERT OR IGNORE INTO projects (project_path, display_name, first_seen, last_seen, session_count)
SELECT project_path,
       -- display_name: extract last path component after /Projects/
       REPLACE(SUBSTR(project_path, INSTR(project_path, '/Projects/') + 10), '/', ' / '),
       MIN(first_seen_at),
       MAX(COALESCE(last_seen_at, first_seen_at)),
       COUNT(*)
FROM sessions
WHERE project_path IS NOT NULL AND project_path != ''
GROUP BY project_path;

--------------------------------------------------------------------------------
-- 2. Enrichment columns on artifact tables
--------------------------------------------------------------------------------

ALTER TABLE file_operations ADD COLUMN result_summary TEXT;
ALTER TABLE file_operations ADD COLUMN is_error INTEGER DEFAULT 0;
ALTER TABLE git_operations ADD COLUMN result_summary TEXT;
ALTER TABLE git_operations ADD COLUMN is_error INTEGER DEFAULT 0;

-- Backfill result_summary and is_error from tool_executions
UPDATE file_operations SET
    result_summary = (SELECT SUBSTR(te.result_content, 1, 500) FROM tool_executions te WHERE te.tool_use_id = file_operations.tool_use_id),
    is_error = COALESCE((SELECT te.is_error FROM tool_executions te WHERE te.tool_use_id = file_operations.tool_use_id), 0)
WHERE tool_use_id IS NOT NULL;

UPDATE git_operations SET
    result_summary = (SELECT SUBSTR(te.result_content, 1, 500) FROM tool_executions te WHERE te.tool_use_id = git_operations.tool_use_id),
    is_error = COALESCE((SELECT te.is_error FROM tool_executions te WHERE te.tool_use_id = git_operations.tool_use_id), 0)
WHERE tool_use_id IS NOT NULL;

--------------------------------------------------------------------------------
-- 3. Missing index
--------------------------------------------------------------------------------

CREATE INDEX IF NOT EXISTS idx_file_operations_operation_type ON file_operations(operation_type);

--------------------------------------------------------------------------------
-- 4. Analytical views
--------------------------------------------------------------------------------

-- v_file_token_cost: Per-file token cost attribution across all sessions
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
JOIN messages m ON fo.message_uuid = m.uuid
JOIN sessions s ON m.session_id = s.session_id
LEFT JOIN token_usage tu ON m.uuid = tu.message_uuid
GROUP BY s.project_path, fo.file_path, fo.operation_type;

-- v_file_conversation_context: Conversation context around file mutations
-- Each row is an assistant text block adjacent to a file operation
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
JOIN messages m_op ON fo.message_uuid = m_op.uuid
JOIN messages m_asst ON m_asst.session_id = m_op.session_id
    AND m_asst.type = 'assistant'
    AND m_asst.timestamp <= m_op.timestamp
    AND m_asst.timestamp >= datetime(m_op.timestamp, '-60 seconds')
JOIN message_content mc ON mc.message_uuid = m_asst.uuid
    AND mc.block_type = 'text'
JOIN sessions s ON fo.session_id = s.session_id
WHERE fo.operation_type IN ('write', 'edit');

-- v_project_summary: Project activity summary
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
LEFT JOIN messages m ON m.session_id = s.session_id
LEFT JOIN token_usage tu ON tu.message_uuid = m.uuid
LEFT JOIN file_operations fo ON fo.session_id = s.session_id
LEFT JOIN git_operations go2 ON go2.session_id = s.session_id
GROUP BY s.project_path;

-- v_file_provenance: Complete file operation history across sessions
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
JOIN messages m ON fo.message_uuid = m.uuid
JOIN sessions s ON fo.session_id = s.session_id
ORDER BY fo.file_path, fo.timestamp;

-- v_git_commit_context: Git commit messages with surrounding assistant reasoning
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
JOIN messages m_commit ON go2.message_uuid = m_commit.uuid
JOIN messages m_asst ON m_asst.session_id = m_commit.session_id
    AND m_asst.type = 'assistant'
    AND m_asst.timestamp <= m_commit.timestamp
    AND m_asst.timestamp >= datetime(m_commit.timestamp, '-120 seconds')
JOIN message_content mc ON mc.message_uuid = m_asst.uuid
    AND mc.block_type = 'text'
JOIN sessions s ON go2.session_id = s.session_id
WHERE go2.operation_type = 'commit';

-- v_tool_errors: Tool error patterns with conversation context
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
JOIN messages m ON te.message_uuid = m.uuid
JOIN sessions s ON m.session_id = s.session_id
WHERE te.is_error = 1;

-- v_session_cost: Session cost breakdown
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
LEFT JOIN messages m ON m.session_id = s.session_id
LEFT JOIN token_usage tu ON tu.message_uuid = m.uuid
LEFT JOIN file_operations fo ON fo.session_id = s.session_id
LEFT JOIN git_operations go2 ON go2.session_id = s.session_id
GROUP BY s.session_id;
