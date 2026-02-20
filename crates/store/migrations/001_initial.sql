-- Migration 001: Initial schema — all normalized tables for Claude Code JSONL ingestion.
-- This DDL creates 13 tables covering the 7 JSONL record types discovered in empirical
-- analysis of ~/.claude/projects/ session files.

-- sessions: one row per JSONL session file. session_id is the UUID from the filename.
CREATE TABLE sessions (
    session_id    TEXT PRIMARY KEY,
    project_path  TEXT,
    first_seen_at TEXT,
    last_seen_at  TEXT,
    version       TEXT,
    slug          TEXT,
    git_branch    TEXT
);

-- messages: normalized representation of user, assistant, progress, and system records.
-- All full-base record types share these columns. Type discriminator preserved.
CREATE TABLE messages (
    uuid          TEXT PRIMARY KEY,
    session_id    TEXT NOT NULL REFERENCES sessions(session_id),
    type          TEXT NOT NULL,
    timestamp     TEXT NOT NULL,
    parent_uuid   TEXT,
    is_sidechain  INTEGER NOT NULL DEFAULT 0,
    user_type     TEXT,
    cwd           TEXT,
    git_branch    TEXT,
    version       TEXT,
    slug          TEXT,
    agent_id      TEXT,
    is_meta       INTEGER,
    model         TEXT,
    stop_reason   TEXT,
    request_id    TEXT,
    subtype       TEXT
);

-- message_content: content blocks from user and assistant messages.
-- block_type is one of: text, thinking, tool_use, tool_result.
CREATE TABLE message_content (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    message_uuid  TEXT NOT NULL REFERENCES messages(uuid),
    block_index   INTEGER NOT NULL,
    block_type    TEXT NOT NULL,
    text_content  TEXT,
    tool_use_id   TEXT,
    tool_name     TEXT,
    tool_input    TEXT,
    is_error      INTEGER,
    thinking_signature TEXT,
    UNIQUE(message_uuid, block_index)
);

-- token_usage: per-assistant-message API usage statistics.
CREATE TABLE token_usage (
    message_uuid                  TEXT PRIMARY KEY REFERENCES messages(uuid),
    input_tokens                  INTEGER,
    output_tokens                 INTEGER,
    cache_creation_input_tokens   INTEGER,
    cache_read_input_tokens       INTEGER,
    service_tier                  TEXT,
    cache_creation_json           TEXT,
    extra_json                    TEXT
);

-- tool_executions: joined tool_use (from assistant) + tool_result (from user) records.
CREATE TABLE tool_executions (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    message_uuid  TEXT NOT NULL REFERENCES messages(uuid),
    tool_use_id   TEXT NOT NULL,
    tool_name     TEXT NOT NULL,
    input_json    TEXT,
    result_content TEXT,
    is_error      INTEGER,
    UNIQUE(message_uuid, tool_use_id)
);

-- agents: subagent tracking for multi-agent sessions.
CREATE TABLE agents (
    agent_id      TEXT PRIMARY KEY,
    session_id    TEXT REFERENCES sessions(session_id),
    first_seen_at TEXT,
    last_seen_at  TEXT
);

-- queue_operations: queue-operation records (enqueue, dequeue, remove, popAll).
-- These records lack a UUID, so we use an auto-incrementing primary key.
CREATE TABLE queue_operations (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id    TEXT NOT NULL,
    operation     TEXT NOT NULL,
    timestamp     TEXT NOT NULL,
    content       TEXT
);

-- progress_events: progress records with polymorphic data payloads.
-- data_json holds the full progress.data object as JSON.
CREATE TABLE progress_events (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    uuid          TEXT NOT NULL,
    session_id    TEXT NOT NULL,
    timestamp     TEXT NOT NULL,
    data_type     TEXT NOT NULL,
    data_json     TEXT NOT NULL
);

-- system_events: system records with subtype discrimination.
-- Subtype-specific fields stored in extra_json to handle the 6+ subtypes.
CREATE TABLE system_events (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    uuid          TEXT NOT NULL,
    session_id    TEXT NOT NULL,
    timestamp     TEXT NOT NULL,
    subtype       TEXT NOT NULL,
    level         TEXT,
    duration_ms   INTEGER,
    content       TEXT,
    extra_json    TEXT
);

-- summaries: summary records (lightweight — no uuid or sessionId in the record itself).
-- session_id is derived from the filename during ingestion.
CREATE TABLE summaries (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id    TEXT NOT NULL,
    summary       TEXT NOT NULL,
    leaf_uuid     TEXT NOT NULL
);

-- sync_metadata: byte-offset tracking for incremental sync.
-- Each JSONL file has one row tracking how far we have read.
CREATE TABLE sync_metadata (
    file_path         TEXT PRIMARY KEY,
    last_byte_offset  INTEGER NOT NULL DEFAULT 0,
    record_count      INTEGER NOT NULL DEFAULT 0,
    last_synced_at    TEXT NOT NULL
);

-- schema_drift_log: overflow fields captured from serde(flatten) HashMap.
-- Tracks unknown/new fields appearing in JSONL records across versions.
CREATE TABLE schema_drift_log (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    field_name      TEXT NOT NULL,
    record_type     TEXT NOT NULL,
    version         TEXT,
    sample_value    TEXT,
    first_seen_at   TEXT NOT NULL DEFAULT (datetime('now')),
    source_context  TEXT,
    UNIQUE(field_name, record_type, version)
);

-- Indexes for query performance on high-cardinality tables.
CREATE INDEX idx_messages_session_id ON messages(session_id);
CREATE INDEX idx_messages_timestamp ON messages(timestamp);
CREATE INDEX idx_message_content_message_uuid ON message_content(message_uuid);
CREATE INDEX idx_tool_executions_message_uuid ON tool_executions(message_uuid);
CREATE INDEX idx_tool_executions_tool_name ON tool_executions(tool_name);
CREATE INDEX idx_progress_events_session_id ON progress_events(session_id);
CREATE INDEX idx_progress_events_data_type ON progress_events(data_type);
CREATE INDEX idx_system_events_session_id ON system_events(session_id);
CREATE INDEX idx_system_events_subtype ON system_events(subtype);
CREATE INDEX idx_queue_operations_session_id ON queue_operations(session_id);
