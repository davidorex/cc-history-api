## Yes, several modeling adjustments are warranted

Treating the DB as a semantic-signal store, the unpromoted overflow falls into three buckets. Here is what's worth modeling, what isn't, and a few structural gaps that are bigger than just promotion.

### Already correctly excluded by design

Migration 005 deliberately **dropped `progress_events` and `queue_operations`** with the rationale that progress is redundant snapshots of state already captured by the final `tool_result`, and queue content duplicates the user prompt already in `messages`. The decomposer at `crates/store/src/decompose.rs:551-565` no-ops on progress records. So the 337K `progress.parentToolUseID` / `progress.toolUseID` overflow entries reflect drift surveillance only — there is no progress payload being lost. **No change needed here.** This decision is sound under the semantic-signal lens.

### Pure noise — keep in `extra_json`, never promote

These have effectively zero variance or zero semantic content:

| Field                                                                                                           | Reason                                                               |
| --------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------- |
| `*.entrypoint`                                                                                                  | Always `"cli"` across 157K occurrences                               |
| `assistant.message.type`                                                                                        | Always `"message"` across 184K occurrences                           |
| `assistant.message.container`                                                                                   | Almost always null                                                   |
| `assistant.message.context_management`                                                                          | Almost always `{applied_edits:[]}`                                   |
| `system.messageCount`                                                                                           | Derivable from `COUNT(*) FROM messages`                              |
| `system.hookInfos` / `hookErrors` / `hasOutput` / `preventedContinuation` / `stopReason` / `toolUseID` (system) | Hook plumbing for `stop_hook_summary` subtype — plumbing, not signal |
| `system.error` / `cause` / `maxRetries` / `retryAttempt` / `retryInMs`                                          | Retry telemetry — ops, not semantics                                 |
| `system.verb` / `writtenPaths`                                                                                  | Hook output metadata                                                 |
| `system.compactMetadata` / `microcompactMetadata`                                                               | Compaction ops telemetry                                             |
| `assistant.message.usage.iterations` / `speed` / `inference_geo`                                                | Usage perf/region — keep in `token_usage.extra_json`                 |

### Tier 1 — high semantic signal, modeling gap visible in queries

These are the recommended adjustments. Each maps to a query pattern that is currently impossible or expensive without `json_extract` round-trips:

**(1) Agent / plugin / skill attribution — biggest gap.**
`assistant.attributionAgent` (2,585 occ across 2.1.121+), `attributionPlugin` (492), `attributionSkill` (19) tag each assistant turn with which subagent / plugin / skill produced it. The existing `agents` table only stores `(agent_id, session_id, first_seen_at)` — agent_id is a run UUID, not a name. Without these columns:
- Cannot answer "show all output produced by `general-purpose` subagent across the project"
- Cannot answer "which skills/plugins are actually exercised in production conversations"
- The new `agents` view of work has no human-readable surface
Suggested model: `messages.attribution_agent`, `messages.attribution_plugin`, `messages.attribution_skill` (TEXT, nullable, all three populated only on assistant rows). Existing `agents` table could optionally gain a `name` column populated from `attributionAgent` for joins.

**(2) Fork lineage between sessions.**
`assistant.forkedFrom` (188), `user.forkedFrom` (166), `system.forkedFrom` (58) carry `{messageUuid, sessionId}` — the parent session and message a forked session descended from. Currently:
- `messages.parent_uuid` only handles intra-session links
- Sessions table has no fork pointer
- Reconstructing "all sessions descended from session X" requires `json_extract(extra_json, '$.forkedFrom.sessionId')` scanning every message
Suggested model: `sessions.forked_from_session_id` and `sessions.forked_from_message_uuid` columns, populated when any record in the session carries `forkedFrom` (likely the first user/assistant record). This is session-level metadata that surfaces a graph the data already captures.

**(3) Turn grouping via `promptId`.**
`user.promptId` (56,331 occ since 2.1.100) is a stable identifier for one user turn. A single user prompt often produces a chain of (user → assistant → tool_use → user(tool_result) → assistant) messages. Currently you can only group via `parent_uuid` walks. With a real `messages.prompt_id` column populated on user records (and propagated to descendants if Claude Code emits it on assistant/system records too — needs verification), turn-grouped queries (`GROUP BY prompt_id`) become trivial.
Suggested model: `messages.prompt_id TEXT` populated on user rows where present. Potential later step: backfill descendant `prompt_id` via `parent_uuid` walk if Claude Code only stamps it on the originating user row.

**(4) Synthetic-message filter consistency.**
`user.isVisibleInTranscriptOnly` (291 occ across 50 versions) is the direct sibling of `isCompactSummary` — a boolean filter distinguishing synthetic from real conversational messages. `is_compact_summary` was promoted; this one wasn't. The analytical views (`v_project_summary`, `v_session_cost`, etc.) already filter by `is_compact_summary = 0` per the Phase 06 design. Adding the same filter for synthetic transcript-only messages would be a one-column change.
Suggested model: `messages.is_visible_in_transcript_only INTEGER DEFAULT 0`, mirroring the existing pattern for `is_compact_summary`. Update analytical views to filter where appropriate.

### Tier 2 — moderate signal, narrower utility

**(5) Plan-mode content.**
`user.planContent` (85 occ) is the user's articulated plan markdown when plan mode produces a structured plan. This is high-value text content but very rare. Since `messages.extra_json` already carries it via the user-record overflow path, it is reachable; promotion would mainly enable cleaner FTS5 indexing on plan content.
Suggested model: optional. Could be promoted to `messages.plan_content TEXT`, or left in `extra_json`. Lower priority unless plan analysis becomes a target.

**(6) Logical parent across compaction boundaries.**
`system.logicalParentUuid` (287 occ) points to the logical parent of a compaction event in the conversation graph. Without it, the conversation graph fractures at every compaction. Already lives in `system_events.extra_json`.
Suggested model: `system_events.logical_parent_uuid TEXT`. Smaller change because system_events already exists with its own extra_json.

**(7) MCP structured results.**
`user.mcpMeta` (7 occ) carries structured MCP tool results in a format distinct from the regular tool_result content blocks. Very low volume but semantically rich. Probably not worth promoting now; the low count means most MCP tool calls are flowing through normal `tool_result` content blocks.

### Tier 3 — small additions to existing structure

**(8) `assistant.message.diagnostics`** (6,352 occ since 2.1.119) carries `{cache_miss_reason: {...}}`. Useful for cost/cache analysis but operational, not semantic. Keep in `extra_json`.

**(9) `assistant.message.stop_details`** (41,877 occ since 2.1.100) is a structured envelope around stop reason. Existing `messages.stop_reason` carries the simple string. Could augment but not a high-leverage change.

**(10) `assistant.apiError` / `apiErrorStatus` / `errorDetails`.** Total: 6 occurrences. Leave in extra_json.

### Cross-cutting structural observations

**A. The `agents` table is underspecified relative to the data we now have.**
With `attributionAgent` / `attributionPlugin` / `attributionSkill` available per turn, the agents table could become a richer dimension table: `(agent_id, session_id, first_seen_at, agent_name, plugin_name, skill_name)`. Currently it only records that an agent run existed, not what it was.

**B. Sessions table lacks any fork pointer.**
This is the structural gap behind (2) above. Without it, the session graph is implicitly a forest of disconnected trees rather than the actual DAG that compaction-fork produces.

**C. The analytical views do not consult `extra_json`.**
The seven existing views (`v_project_summary`, `v_session_cost`, etc.) treat `extra_json` as opaque. Any analytical capability built on attribution, forking, or turn-grouping requires either (a) promoting the fields per Tier 1, or (b) augmenting the views with `json_extract` calls. Per the Phase 06 design preference (real columns over JSON extraction for hot-path analytics), Tier 1 promotions are the lower-friction path.

**D. Drift surveillance is doing its job.**
The fact that you can see all of these unpromoted-but-known fields with version correlation, occurrence counts, and sample values is exactly what the drift-tracking architecture is for. The decision to promote any of them is a deliberate semantic-value decision — the underlying data has been preserved either way.

### Suggested promotion order, by leverage

1. `messages.attribution_agent` / `attribution_plugin` / `attribution_skill` — biggest analytical leverage, recently arrived, no current way to query
2. `sessions.forked_from_session_id` + `sessions.forked_from_message_uuid` — unlocks session-graph queries
3. `messages.prompt_id` — unlocks turn-grouping
4. `messages.is_visible_in_transcript_only` — completes the synthetic-message filter pair started by `is_compact_summary`
5. `system_events.logical_parent_uuid` — patches conversation graph across compactions
6. (optional) `messages.plan_content` — promote only if plan analysis becomes a target

Each of (1)–(5) is a small migration in the established 006 pattern: ALTER TABLE ADD COLUMN + decomposer extraction + (where useful) view updates. None require new tables.