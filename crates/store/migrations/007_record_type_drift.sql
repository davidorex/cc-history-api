-- Migration 007: record_type_drift_log
--
-- Closes the variant-level architectural blind spot in JSONL ingestion.
--
-- The JSONLRecord enum at crates/core/src/record.rs has, prior to commit B1.1,
-- seven known variants discriminated by the JSON `type` field. Records whose
-- `type` value is not one of those seven (observed in the corpus:
-- `attachment`, `last-prompt`, `custom-title`, `permission-mode`, `agent-name`,
-- `ai-title`) caused serde to reject the entire record. The parser caught the
-- error as a tracing::warn!, advanced past the line, and wrote nothing — so
-- the record was silently dropped.
--
-- B1.1 introduces a JSONLRecord::Unknown { type_name, raw } variant via a
-- manual Deserialize impl with two-pass dispatch. Unknown discriminators
-- now deserialize successfully and route through decompose_unknown, which
-- records each (type_name, version) pair to this new table.
--
-- The shape mirrors schema_drift_log (migration 006): a UNIQUE constraint
-- on (type_name, version) so re-observations idempotently increment
-- occurrence_count via INSERT ... ON CONFLICT DO UPDATE.
--
-- Backfill of the historical ~13.5K records dropped corpus-wide before B1.1
-- shipped is intentionally deferred to B1.2's bytewise re-ingestion path
-- (UPDATE sync_metadata SET last_byte_offset = 0 WHERE file_path IN (...)
--  + claude-history sync). This migration is purely structural; it does
-- not write any rows on application.
--
-- All DDL is IF NOT EXISTS so this migration is safe to replay; the runner
-- at crates/store/src/schema.rs already gates application via schema_versions.

CREATE TABLE IF NOT EXISTS record_type_drift_log (
    id               INTEGER PRIMARY KEY AUTOINCREMENT,
    type_name        TEXT NOT NULL,
    version          TEXT,
    sample_value     TEXT,
    first_seen_at    TEXT NOT NULL DEFAULT (datetime('now')),
    last_seen_at     TEXT NOT NULL DEFAULT (datetime('now')),
    occurrence_count INTEGER NOT NULL DEFAULT 1,
    UNIQUE(type_name, version)
);

CREATE INDEX IF NOT EXISTS idx_record_type_drift_type_name
    ON record_type_drift_log(type_name);
