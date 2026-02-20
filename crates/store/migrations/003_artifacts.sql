-- Migration 003: Artifact layer tables — file tracking, file operations, and git operations.
--
-- Creates three tables for the artifact decomposition layer (Phase 5):
--   files:           one row per unique file path per session
--   file_operations: every file touch operation extracted from tool_use blocks
--   git_operations:  git commands extracted from Bash tool_use blocks
--
-- Also creates an external-content FTS5 virtual table over file_operations
-- for full-text search of file content and diffs.
--
-- Requirement IDs: ART-01 (files table), ART-02 (file_operations table),
--                  ART-03 (git_operations table), FTS-02 (file_operations FTS index)

-- files: one row per unique file path per session.
-- Tracks first/last modification times and total operation count.
CREATE TABLE files (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id      TEXT NOT NULL REFERENCES sessions(session_id),
    file_path       TEXT NOT NULL,
    first_seen      TEXT NOT NULL,
    last_modified   TEXT NOT NULL,
    operation_count INTEGER NOT NULL DEFAULT 0,
    UNIQUE(session_id, file_path)
);

-- file_operations: every file touch operation.
-- operation_type is one of: write, edit, read, bash_cp, bash_mv, bash_rm, bash_mkdir, bash_touch.
-- content holds full content for write, new_string for edit, NULL for read.
-- old_content holds old_string for edit, NULL for write/read.
-- command holds the Bash command for bash_* operations.
CREATE TABLE file_operations (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id      TEXT NOT NULL REFERENCES sessions(session_id),
    file_path       TEXT NOT NULL,
    operation_type  TEXT NOT NULL,
    content         TEXT,
    old_content     TEXT,
    command         TEXT,
    tool_use_id     TEXT,
    message_uuid    TEXT REFERENCES messages(uuid),
    timestamp       TEXT NOT NULL,
    UNIQUE(tool_use_id)
);

-- git_operations: extracted from Bash git commands.
-- operation_type is one of: commit, push, checkout, branch, merge, rebase, stash, pull, status, log, diff, add.
-- commit_message extracted for commit operations.
-- branch extracted where detectable.
CREATE TABLE git_operations (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id      TEXT NOT NULL REFERENCES sessions(session_id),
    operation_type  TEXT NOT NULL,
    command         TEXT NOT NULL,
    commit_message  TEXT,
    branch          TEXT,
    tool_use_id     TEXT,
    message_uuid    TEXT REFERENCES messages(uuid),
    timestamp       TEXT NOT NULL,
    UNIQUE(tool_use_id, operation_type)
);

-- Indexes following PAT-032 naming convention: idx_{table}_{column}
CREATE INDEX idx_files_session_id ON files(session_id);
CREATE INDEX idx_files_file_path ON files(file_path);
CREATE INDEX idx_file_operations_session_id ON file_operations(session_id);
CREATE INDEX idx_file_operations_file_path ON file_operations(file_path);
CREATE INDEX idx_file_operations_timestamp ON file_operations(timestamp);
CREATE INDEX idx_file_operations_tool_use_id ON file_operations(tool_use_id);
CREATE INDEX idx_git_operations_session_id ON git_operations(session_id);
CREATE INDEX idx_git_operations_operation_type ON git_operations(operation_type);
CREATE INDEX idx_git_operations_timestamp ON git_operations(timestamp);

-- FTS5 external-content virtual table over file_operations for content search.
-- Follows the same external-content pattern as 002_fts5.sql (fts_message_content).
-- Indexes content, old_content, and command columns for full-text search.
-- Requires manual rebuild after sync operations, same as fts_message_content.
CREATE VIRTUAL TABLE fts_file_operations USING fts5(
    content,
    old_content,
    command,
    content='file_operations',
    content_rowid='id',
    tokenize='unicode61'
);
