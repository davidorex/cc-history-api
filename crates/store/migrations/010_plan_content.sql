-- Migration 010: messages.plan_content TEXT column + idempotent backfill
-- from messages.extra_json.
--
-- C2.1 promotes the user.planContent field (plan-mode markdown body) from the
-- catch-all messages.extra_json into a typed messages.plan_content column.
-- Per the schema-drift snapshot at the time of investigation, planContent
-- accumulated as an unmodeled overflow field with corpus-wide volume on the
-- order of ~80 plan documents ranging single-digit-KB to low-double-digit-KB
-- each. extra_json is not covered by the FTS5 index, so plan-content prose
-- has been ingested-but-invisible to the existing search surfaces. Promotion
-- to a real column is the structural prerequisite for C2.3 (FTS5 wiring via
-- synthetic message_content rows), C2.4 (CLI plans subcommand), C2.5 (MCP
-- list_plans / get_plan tools), and C2.6 (REST /v1/plans endpoints).
--
-- This migration's intent:
--
--   1. ADD COLUMN messages.plan_content TEXT — nullable; the vast majority
--      of message rows have no plan content. SQLite's ALTER TABLE ADD COLUMN
--      is non-idempotent at the DDL level (re-running raises "duplicate
--      column name"), so the migration runner at crates/store/src/schema.rs
--      gates application via schema_versions; the runner-level gate is the
--      authoritative idempotency mechanism. The two UPDATE statements below
--      are independently idempotent (their WHERE clauses no longer match
--      after the first run), so even if a partial replay occurred the data
--      side would remain stable.
--
--   2. Backfill UPDATE — copies json_extract(extra_json, '$.planContent')
--      into the new plan_content column for every existing row that has the
--      key. This recovers the historical plan-content corpus ingested before
--      C2.2's decomposer change took effect.
--
--   3. Cleanup UPDATE — removes the planContent key from extra_json once
--      it has been promoted to the typed column. This avoids a state where
--      the same data lives in two places, which would otherwise complicate
--      future query-authoring (every plan-content reader would need to know
--      whether to consult plan_content or extra_json or both). After C2.2's
--      decomposer change, future ingestion writes plan_content directly and
--      strips planContent from the extra_json HashMap before serialization,
--      so the cleanup state remains stable.
--
--   4. Partial index idx_messages_plan_content_present — keys by session_id
--      where plan_content IS NOT NULL. This serves the "sessions with plans"
--      query shape used by the upcoming C2.4 CLI plans-list and C2.6 REST
--      /v1/plans endpoints. A partial index is dramatically smaller than a
--      full index here because plan-bearing rows are a small fraction of the
--      total messages corpus.
--
-- Replay safety:
--   - The migration runner's schema_versions guard prevents re-application,
--     making the non-idempotent ADD COLUMN safe in normal operation.
--   - Both UPDATE statements are individually idempotent: the backfill's
--     WHERE clause excludes rows that no longer carry $.planContent in
--     extra_json (because the cleanup UPDATE removed it), and the cleanup's
--     WHERE clause similarly no-ops once cleanup has run.
--   - CREATE INDEX uses IF NOT EXISTS for belt-and-suspenders idempotency.

ALTER TABLE messages ADD COLUMN plan_content TEXT;

UPDATE messages
SET plan_content = json_extract(extra_json, '$.planContent')
WHERE json_extract(extra_json, '$.planContent') IS NOT NULL;

UPDATE messages
SET extra_json = json_remove(extra_json, '$.planContent')
WHERE json_extract(extra_json, '$.planContent') IS NOT NULL;

CREATE INDEX IF NOT EXISTS idx_messages_plan_content_present
    ON messages(session_id)
    WHERE plan_content IS NOT NULL;
