# Project State

## Project Reference

See: .planning/PROJECT.md (updated 2026-02-21)

**Core value:** Universal, language-agnostic, queryable access to Claude Code's complete session history through a single binary that never discards data and actively detects schema evolution.
**Current focus:** v1.0 MVP -- SHIPPED

## Current Position

Milestone: v1.0 MVP -- SHIPPED 2026-02-21
Status: All 6 phases complete. 27 plans executed. 102 requirements delivered. Milestone archived.
Last activity: 2026-02-22 -- Completed quick task 001 (queries CLI subcommand)

### Quick Tasks

| ID  | Name                              | Status   | Duration | Commit  |
|-----|-----------------------------------|----------|----------|---------|
| 001 | Add queries CLI subcommand (list/show/run) | Complete | 6 min | 16a252b |

## Performance Metrics

**Velocity:**
- Total plans completed: 27
- Average duration: ~4.3 min
- Total execution time: ~1.9 hours

**By Phase:**

| Phase | Plans | Total | Avg/Plan |
|-------|-------|-------|----------|
| 01 | 4/4 | 28 min | 7 min |
| 02 | 3/3 | 22 min | 7.3 min |
| 03 | 6/6 | ~30 min | ~5 min |
| 04 | 2/2 | 8 min | 4 min |
| 05 | 8/8 | 25 min | 3.1 min |
| 06 | 4/4 | ~14 min | ~3.5 min |

## Decisions

| Decision | Context | Date |
|----------|---------|------|
| Queries list/show routed without DB connection | Only run needs ConnectionMode; list/show are filesystem-only | 2026-02-22 |
| Query run output always JSON | Consistent with sql_passthrough behavior | 2026-02-22 |
| .sql+.toml sidecar pattern for query metadata | Auto-discovers params from SQL when no sidecar present | 2026-02-22 |

## Session Continuity

Last session: 2026-02-22
Stopped at: Quick task 001 complete.
Resume file: N/A

---

## Post-MVP Architectural Roadmap

**Encoded 2026-05-09 Asia/Shanghai. Plan source: `/Users/david/.claude/plans/curious-napping-koala.md`. Templates: `.planning/templates/{subagent-prompt,adversarial-audit,deviation-triage}-template.md`.**

This section mirrors the in-session TaskList DAG so a fresh session can rebuild the in-session list without re-reading the plan from scratch. Each implementation node carries a 4-task chain: Implementation → Adversarial audit → Deviation triage → User review (which may spawn issue-resolution subagents). Cross-node DAG dependencies route through the predecessor's Review checkpoint, not just the impl, so deviations must be reviewed before the next node may begin.

Status legend: `[ ]` pending · `[~]` in progress · `[x]` complete · `[!]` blocked-with-issue · `[-]` deleted/superseded

### Tier 1 — Ship as soon as practical (independent, no DAG predecessors)

- [ ] **A1** Author log rotation LaunchAgent + helper script *(no predecessors)*
  - [ ] A1-Audit
  - [ ] A1-Triage
  - [ ] A1-Review
- [ ] **A2** MCPB rebundle (manifest 0.1.0 → 0.1.1) *(no predecessors)*
  - [ ] A2-Audit
  - [ ] A2-Triage
  - [ ] A2-Review

### Tier 2 — Structural floor (B1.1 is start of B/C chain; must precede C tier)

- [ ] **B1.1** JSONLRecord::Unknown variant + migration 007 *(no predecessors; B/C chain starts here)*
  - [ ] B1.1-Audit
  - [ ] B1.1-Triage
  - [ ] B1.1-Review *(gate for B1.2)*
- [ ] **B1.2** Drift logging + CLI + REST + bytewise re-ingestion backfill *(← B1.1-Review)*
  - [ ] B1.2-Audit
  - [ ] B1.2-Triage
  - [ ] B1.2-Review *(gate for C1.1)*

### Tier 3 — Semantic recovery (commits serialize at migration-numbering)

- [ ] **C1.1** AttachmentRecord + AttachmentBody (12 subtypes) + migration 008 *(← B1.2-Review)*
  - [ ] C1.1-Audit
  - [ ] C1.1-Triage
  - [ ] C1.1-Review *(gate for C1.2 AND C2.1)*
- [ ] **C1.2** Decomposer routing for attachments + hook_executions *(← C1.1-Review)*
  - [ ] C1.2-Audit
  - [ ] C1.2-Triage
  - [ ] C1.2-Review *(gate for C1.3 AND C1.4)*
- [ ] **C1.3** FTS5 fts_attachment_text_content + watcher rebuild integration *(← C1.2-Review)*
  - [ ] C1.3-Audit
  - [ ] C1.3-Triage
  - [ ] C1.3-Review
- [ ] **C1.4** CLI / MCP / REST surfacing for attachments + hook_executions *(← C1.2-Review)*
  - [ ] C1.4-Audit
  - [ ] C1.4-Triage
  - [ ] C1.4-Review
- [ ] **C2.1** planContent migration 009 + decomposer extraction *(← C1.1-Review for migration numbering)*
  - [ ] C2.1-Audit
  - [ ] C2.1-Triage
  - [ ] C2.1-Review *(gate for C2.3, C2.4, C2.5, C2.6)*
- [ ] **C2.3** FTS5 coverage for plan_content via synthetic message_content rows *(← C2.1-Review)*
  - [ ] C2.3-Audit
  - [ ] C2.3-Triage
  - [ ] C2.3-Review
- [ ] **C2.4** CLI plans subcommand *(← C2.1-Review)*
  - [ ] C2.4-Audit
  - [ ] C2.4-Triage
  - [ ] C2.4-Review
- [ ] **C2.5** MCP list_plans / get_plan tools + query_messages extension *(← C2.1-Review)*
  - [ ] C2.5-Audit
  - [ ] C2.5-Triage
  - [ ] C2.5-Review
- [ ] **C2.6** REST /v1/plans endpoints *(← C2.1-Review)*
  - [ ] C2.6-Audit
  - [ ] C2.6-Triage
  - [ ] C2.6-Review

### Tier 4 — Cleanup (technical debt, non-blocking)

- [ ] **D1** Move version_history backfill into sync_all *(no predecessors)*
  - [ ] D1-Audit
  - [ ] D1-Triage
  - [ ] D1-Review *(gate for D4)*
- [ ] **D2** Sequence the daemon-startup race *(no predecessors)*
  - [ ] D2-Audit
  - [ ] D2-Triage
  - [ ] D2-Review
- [ ] **D3** Unify session_count semantics *(no predecessors)*
  - [ ] D3-Audit
  - [ ] D3-Triage
  - [ ] D3-Review *(gate for D4)*
- [ ] **D4** Regression test for D1 / D3 *(← D1-Review AND D3-Review)*
  - [ ] D4-Audit
  - [ ] D4-Triage
  - [ ] D4-Review

### Initial unblocked starting points (six)

```
A1, A2, B1.1, D1, D2, D3
```

Any of these may be authorized first; none block any other. Authorization for each spawn is the user's per the plan file's Execution model section.

### Cross-session rebuild procedure

If a fresh Claude Code session needs to rebuild the in-session TaskList from this section:

1. Read this section's checkbox tree.
2. For each `[ ]` line, call `TaskCreate` with the matching subject and description from the plan file's Tier sections.
3. After all 68 tasks are created, call `TaskUpdate` with `addBlockedBy` to encode dependencies per the gate annotations above (each `*(← XYZ-Review)*` becomes `addBlockedBy=[XYZ-Review-task-id]`).
4. The within-node chain (Audit ← Impl, Triage ← Audit, Review ← Triage) is implicit and must be encoded for every node.
5. For any task marked `[x]` in this section, after creation call `TaskUpdate` with `status: "completed"` to skip it.
6. For any task marked `[!]`, leave it pending; the prior session left it blocked-with-issue and the user owns the unblock decision.

The plan file at `/Users/david/.claude/plans/curious-napping-koala.md` is the canonical source of per-item subject/description content; this section is the durable status mirror.

### Status update protocol

When a task transitions in the in-session TaskList, mirror the change here within the same commit so STATE.md stays synchronized:

- Task moves to `in_progress` → flip checkbox to `[~]`
- Task completes → flip to `[x]` and add a one-line note with commit SHA below the task: `  - landed in <sha>: <one-line description>`
- Task surfaces an issue that blocks progress → flip to `[!]` and add a one-line note pointing at the audit-output or triage-output file
- Task is superseded or deleted → flip to `[-]` and add a one-line note explaining

Without this protocol, STATE.md drifts from the in-session view and the cross-session rebuild produces a stale picture.
