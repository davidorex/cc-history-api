# Phase 6: Version Monitoring - Context

**Gathered:** 2026-02-21
**Status:** Ready for planning

<domain>
## Phase Boundary

Active Claude Code version detection and schema drift analysis across versions. The watcher already detects version changes from the sessions table `version` field and fires `SseEvent::VersionChanged` (Phase 4). Phase 6 persists version history to a new `schema_versions` table, enriches drift presentation with version correlation and promotion status, and absorbs the compact summary / extra_json work from the milestone-2-spec to address the synthetic message class that inflates analytical views.

</domain>

<decisions>
## Implementation Decisions

### Version detection
- Source is the sessions table `version` field, already parsed from JSONL during ingestion (existing Phase 4 code)
- Detection is ingestion-only — no separate periodic polling loop; next session reveals any version change
- On version change: write to `schema_versions` table immediately (real-time persistence) AND fire existing `SseEvent::VersionChanged`
- Backfill on startup: scan sessions table for distinct versions and populate `schema_versions` on first run

### schema_versions table
- Columns: version, first_seen_at, last_seen_at, session_id (first session with this version), session_count, new_fields_count (overflow fields introduced with this version)
- Session count per version tracked (how many sessions ran on each Claude Code version)

### Drift presentation
- Primary grouping by Claude Code version, sub-grouped by record type within each version
- Each drift field includes truncated sample value (first ~200 chars from actual JSONL data)
- Each drift field shows promotion status: 'promoted' (has a real column), 'extra_json' (captured but not indexed), 'unhandled' (overflow only)
- Drift logging enhanced to record the Claude Code version that produced the overflow field (currently captures record context but not session version)

### /v1/schema/versions endpoint
- Default: timeline list — version string, first/last seen timestamps, session count, new_fields_count
- `?diff=true` query param adds per-version field diffs: new overflow fields introduced, fields that disappeared

### /v1/schema/drift endpoint
- Grouped by version (primary) then record type (sub-group)
- Each field: name, sample value (truncated), occurrence count, promotion status
- Version correlation shows which upgrade introduced each drift field

### Compact summary absorption (from milestone-2-spec)
- Add `is_compact_summary INTEGER DEFAULT 0` and `source_tool_use_id TEXT` as real columns on messages table
- Add `extra_json TEXT` on messages table for residual overflow fields (container, context_management, future unknowns)
- Wire `decompose_user` and `decompose_assistant` to populate both real columns (from known overflow keys) and extra_json (remaining overflow)
- Backfill existing rows from schema_drift_log data

### Analytical view updates
- All views that count messages or sum tokens (v_project_summary, v_session_cost, etc.) updated with `WHERE m.is_compact_summary = 0`
- `is_compact_summary` column exposed in view output so consumers can filter explicitly if needed
- Views are accurate by default; consumers query raw tables for full picture including synthetic messages

### CLI enhancement
- `version-check` and `schema-drift` CLI subcommands enhanced to match API richness
- Version history timeline with session counts and drift impact
- Grouped drift output with promotion status and sample values

### Claude's Discretion
- None — all decisions locked

</decisions>

<specifics>
## Specific Ideas

- The milestone-2-spec (`.planning/audit/milestone-2-spec.md`) documents the compact summary discovery in detail, including the specific drift fields (`isCompactSummary`, `isVisibleInTranscriptOnly`, `sourceToolUseID`, `compactMetadata`, `logicalParentUuid`) and their record types
- The `extra_json` pattern follows the approach already proven on `system_events` and `token_usage` tables — promoting semantically important fields to real columns while catching residual overflow in a JSON text column
- The existing `check_version_change` function in `crates/server/src/watcher.rs:375` is the integration point for persisting to schema_versions

</specifics>

<deferred>
## Deferred Ideas

None — discussion stayed within phase scope

</deferred>

---

*Phase: 06-version-monitoring*
*Context gathered: 2026-02-21*
