## What it means

Reset `sync_metadata.last_byte_offset = 0` for chosen files → next `sync` re-decodes them from byte 0 through the **current** decomposer code path. Idempotent: UUID-PK `INSERT OR IGNORE` prevents duplicates.

## Why 241 files

That's the count of JSONL files containing any of the 6 unknown discriminators (`attachment` + 5 session-metadata). Pre-computed by B1.2 at `/tmp/b1.2-affected-files.txt`. B1.2 ran them through `JSONLRecord::Unknown` (typed Attachment didn't exist yet). Re-running them now routes through `decompose_attachment` → typed tables.

## Process

| #   | Step        | Action                                                                                                                                        |
| --- | ----------- | --------------------------------------------------------------------------------------------------------------------------------------------- |
| 1   | Snapshot    | `launchctl unload …plist` → `cp DB → /tmp/…bak` → `PRAGMA integrity_check`                                                                    |
| 2   | Pre-counts  | Capture row counts for `attachments`, `hook_executions`, `fts_attachment_text_content`, `record_type_drift_log` (filtered)                    |
| 3   | Reset scope | `UPDATE sync_metadata SET last_byte_offset = 0 WHERE file_path IN (<241 paths>)` — assert affected = 241                                      |
| 4   | Re-ingest   | `claude-history sync` (single-process, ~10–20 min). Tee output to `/tmp/…sync.log`                                                            |
| 5   | Verify      | post >= pre on every table; `attachments` ≈13K, `hook_executions` ≈9K, integrity_check ok. **Fail → restore backup, do not re-enable daemon** |
| 6   | Resume      | `launchctl load …plist` → `/v1/health` 200 → watcher 30s rebuild populates `fts_attachment_text_content`                                      |

## Then

Write `.planning/audit/c1-attachments-backfill-execution-…md`, commit, flip `attachment-table-backfill-gap.md` from active to resolved, update STATE.md.