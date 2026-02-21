-- Migration 005: Drop zero-value tables
--
-- progress_events held agent_progress, bash_progress, hook_progress, and
-- mcp_progress blobs — redundant snapshots of subagent accumulated state
-- where the final tool_result already captures everything. These records
-- accounted for ~70% of database size (~4.25GB of a 6GB database).
--
-- queue_operations held enqueue/dequeue/remove/popAll records — enqueue
-- content duplicates the raw user prompt already in messages, and the
-- scheduling operations carry no project intelligence.
--
-- The decompose pipeline no longer inserts into either table (as of the
-- companion code change in decompose.rs). This migration drops the tables
-- and their indexes to reclaim space and remove schema noise.

DROP INDEX IF EXISTS idx_progress_events_session_id;
DROP INDEX IF EXISTS idx_progress_events_data_type;
DROP INDEX IF EXISTS idx_queue_operations_session_id;

DROP TABLE IF EXISTS progress_events;
DROP TABLE IF EXISTS queue_operations;
