-- Migration 002: FTS5 full-text search index over message content.
--
-- Creates an external-content FTS5 virtual table that indexes
-- message_content.text_content without duplicating storage.
-- The FTS5 index stores only token positions and metadata;
-- actual text remains in the message_content table.
--
-- External content mode requires manual index maintenance:
-- after sync operations, call the 'rebuild' command to re-index
-- from the current content of message_content.
--
-- Requirement IDs: FTS-01 (message content FTS index)
--
-- Note: FTS-02 (file_operations content index) is deferred to Phase 5.
-- The file_operations table does not exist until Phase 5 (Artifact Layer).
-- The FTS index for file_operations will be created alongside that table.
-- This is dependency-respecting sequencing, not scope reduction.

CREATE VIRTUAL TABLE fts_message_content USING fts5(
    text_content,
    content='message_content',
    content_rowid='id',
    tokenize='unicode61'
);
