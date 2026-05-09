-- Migration 009: FTS5 full-text search index over attachment textual content.
--
-- C1.3 wires a third FTS5 virtual table alongside fts_message_content
-- (migration 002) and fts_file_operations (migration 003). It indexes
-- textual payloads from four AttachmentBody subtypes whose content was
-- identified during the C1.1 corpus survey as carrying searchable prose
-- not otherwise indexed by the message-content FTS:
--
--   1. mcp_instructions_delta — `body_json -> $.addedBlocks` is a JSON array
--      of block strings concatenated to one document per row. The flatten
--      uses json_each() so multi-block deltas index every block, not just
--      the JSON array literal.
--
--   2. skill_listing — `body_json -> $.content` is a single string (the
--      newline-joined enumeration of available skills shown to the agent).
--
--   3. edited_text_file — `body_json -> $.snippet` is a single string (the
--      line-numbered snippet shown to the agent post-edit).
--
--   4. nested_memory — `body_json -> $.content.content` is a single string
--      (the inner triple's body, distinct from the outer envelope path).
--
-- Unlike fts_message_content (external-content over message_content) this
-- FTS table is contentless: attachments rows do not have a single text
-- column to externalize, and the same attachments row can produce 1..N
-- documents (one per block in mcp_instructions_delta.added_blocks). A
-- contentless FTS5 table stores its own copy of the indexed text, which is
-- intentional here — the alternative would be a synthetic projection table
-- mirroring messages/message_content, which is heavier than the per-rebuild
-- DELETE+INSERT pattern this migration is structured around.
--
-- Schema:
--   - attachment_uuid TEXT — the FK back to attachments.uuid (denormalized
--     for unjoined retrieval; FTS5 contentless tables carry no rowid->row
--     join shape unlike external-content tables). UNINDEXED so attachment_uuid
--     itself is not part of the FTS tokenization corpus.
--   - session_id TEXT — for project/session filtering downstream. UNINDEXED.
--   - inner_type TEXT — the AttachmentBody subtype discriminator (one of
--     the four indexed subtypes). UNINDEXED.
--   - text_content TEXT — the actual indexed payload (the content searched
--     by MATCH).
--
-- Maintenance shape:
--   - The watcher's 30s FTS rebuild loop (crates/server/src/watcher.rs)
--     calls rebuild_fts_attachment_text_content alongside rebuild_fts_index
--     and rebuild_fts_file_operations. The rebuild function DELETEs all
--     rows then re-INSERTs from a SELECT over attachments. This is the
--     contentless-table analog of the 'rebuild' command available on
--     external-content tables.
--   - Replay-safe: IF NOT EXISTS gates the DDL; the migration runner gates
--     application via schema_versions.
--
-- Tokenizer: unicode61 (matches the other two FTS tables in this codebase).

CREATE VIRTUAL TABLE IF NOT EXISTS fts_attachment_text_content USING fts5(
    attachment_uuid UNINDEXED,
    session_id UNINDEXED,
    inner_type UNINDEXED,
    text_content,
    tokenize='unicode61'
);
