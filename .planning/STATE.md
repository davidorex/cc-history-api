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

- [x] **A1** Author log rotation LaunchAgent + helper script *(no predecessors)*  *— landed 2026-05-09: plist sha256 7e6808…1c9b239 at ~/Library/LaunchAgents/com.davidrex.claude-history-logrotate.plist; helper sha256 e1eb2a…f76b3c2 at ~/.local/bin/claude-history-logrotate; outside repo, no commit; synthetic-load test confirmed fd continuity (inode 142611871 preserved post-truncate); LaunchAgent scheduled daily 03:00*
  - [x] A1-Audit  *— completed 2026-05-09: 29-row deviation catalog + 5 meta-plan defects flagged (6 divergence, 11 addition, 1 dependency, 9 verification, 1 documentation, 1 test-coverage per triage recount). No severity language in output. Notable: catalog row #13 flags STATE.md edit as documentation deviation (parent-agent action against plan §A1 "no in-repo changes" framing); meta-plan #3 surfaces internal plan inconsistency (24-hour soak gate has no commit to gate)*
  - [x] A1-Triage  *— completed 2026-05-09: 34-row triage (29 dev + 5 meta-plan). 5 plan-defect, 16 impl-defect, 8 mutual, 5 informational. 0 build-blocking, 4 test-blocking, 18 runtime-only, 12 static-only. 27 deterministic, 7 nondeterministic. Triage flagged template-grep collision: "blocking" forbidden-word matches required categorical axis labels "build-blocking" and "test-blocking". Real template bug, fixed in this batch.*
  - [x] A1-Review  *— completed 2026-05-09. Decisions per deviation:*
    *— IGNORE (engineering choices within reasonable scope; project-pattern compliant subagent judgment): #5-8, #11, #12, #15-18, #20, #21, #25, #27-29*
    *— IGNORE (informational, requirement satisfied per audit's own confirmation): #14, #22, #23, #24, #26*
    *— RECORD (passive observations; no immediate action): #1 (shellcheck install at user's discretion), #4 (inflate method semantically equivalent — different bytes-on-disk via base64 framing, same architectural result), #9 (out.log handling — unverified until out.log gets non-zero bytes), #10 (logrotate own log files don't exist yet because schedule hasn't fired), #19 (.tmp atomic-write idiom — safer than direct overwrite)*
    *— RECORD (real but minor plan-text issues; parent-agent fix applied below): meta-plan #1 (threshold not numeric — impl chose 50 MB which matches the inflate target), meta-plan #2 (cadence ambiguity), meta-plan #5 (retention count not specified — impl chose 7)*
    *— RESOLVED via parent-agent direct fix to plan: meta-plan #4 (STATE.md ledger protocol now documented as parent-agent bookkeeping not impl-scope; clarified in plan §A1)*
    *— RESOLVED via parent-agent direct fix to plan: meta-plan #3 (24-hour soak now described as passive verification, not commit-authorization gate, since A1 has no commit by design)*
    *— RESOLVED via parent-agent direct fix to template: template-grep collision (deviation-triage-template.md forbidden-word list updated to remove the bare "blocking" / "non-blocking" tokens that collide with required categorical axis labels)*
    *Net: A1 implementation accepted as-is. Plan refinements applied. No issue-resolution subagent needed.*
- [x] **A2** MCPB rebundle (manifest 0.1.0 → 0.1.1) *(no predecessors)*  *— landed in 658b345: mcpb/manifest.json bumped; cargo build clean; bundle repackaged at /Users/david/Projects/cc-history-api/mcpb/mcpb.mcpb (5.4 MB, 2 files); binary sha256 20b74d…ae8011; awaiting user UI install via Claude Desktop → Settings → Extensions → drag-drop → restart*
  - [x] A2-Audit  *— completed 2026-05-09: 11-row deviation catalog + 2 meta-plan defects (2 omission, 4 verification, 1 divergence, 2 commit-message, 1 addition, 1 verification). Constraint observed: feature-dev:code-reviewer subagent type had no shell access — many verification rows are "NOT EXECUTED, substitute evidence." Future audits use general-purpose with read-only tools instead. Notable: row #7 flags manifest user_config + --db-path args ship to Desktop differing from installed Feb 23 manifest (predated 658b345 in 802a7a4); meta-plan #2 flags ambiguous "8ef5…" commit prefix in plan*
  - [x] A2-Triage  *— completed 2026-05-09: 13-row triage (11 dev + 2 meta-plan). 2 plan-defect, 8 impl-defect, 3 mutual. 0 build-blocking, 0 test-blocking, 3 runtime-only, 10 static-only. 8 deterministic, 0 nondeterministic, 5 unverified (audit tooling constraint). Plan refs not referenced by any deviation: R1, R3, R5, R7. Same template grep collision flagged.*
  - [x] A2-Review  *— completed 2026-05-09. Decisions per deviation:*
    *— IGNORE (audit tooling constraints — feature-dev:code-reviewer had no shell, claims provable via re-run with general-purpose subagent if needed): #5, #6, #8, #11*
    *— IGNORE (defensible choices per project conventions): #3 (5aa0375 docs-only commit fairly excluded from binary-impact list per plan's own framing), #9 ("returns empty" describes prior state, not new commit's claim — outside the no-unjustified-definitives prohibition's spirit), #10 (SHA values in commit message ARE forensic detail per project commit guidelines)*
    *— RECORD (pending user UI install): #1 (Claude Desktop reinstall remains user's UI step), #2 (post-install live verification will run when user installs)*
    *— RECORD (operational quirk noted; commits should be immutable so not amending 658b345): #4 (mcpb pack output-location quirk worth documenting in CLAUDE.md plugin-release section in a future commit, not by amending this one)*
    *— RECORD (significant finding: bundle ships behavioral additions beyond the version bump — manifest's user_config block + mcp_config.args --db-path were already in mcpb/manifest.json since 802a7a4 but absent from currently-installed Feb 23 manifest; on user install, Claude Desktop will receive these. The --db-path arg defaults to ${user_config.db_path} which itself defaults to ~/.claude/.claude-history.db, matching the binary's own default — so net runtime behavior unchanged. The user_config exposes db_path to Claude Desktop's UI for the first time. NOT a bug; ARE undocumented additions in commit 658b345's framing): #7*
    *— RESOLVED via parent-agent direct fix to plan: PD2 (plan §A2 line 49 SHA prefix "8ef5…" corrected to "5aa0375")*
    *— RECORD (PD1: plan ambiguity on subagent-vs-user-UI step delineation within A2 — plan revision deferred to broader plan-quality pass; flagged for future improvement)*
    *Net: A2 implementation accepted as-is. Bundle ready for user UI install. Plan typo fixed. PD1 deferred.*

**User-pending action**: drag `/Users/david/Projects/cc-history-api/mcpb/mcpb.mcpb` into Claude Desktop → Settings → Extensions; restart Desktop. Currently-installed extension shows version 0.1.0 from Feb 23; new bundle is 0.1.1 with commits 6143fbf + 5d8f934 + the user_config/--db-path manifest additions per row #7. Once installed, the post-install verification commands in plan §A2 (pgrep fresh PID, file_history substring test through Claude Desktop's MCP surface) will close out A2's verification debt.

### Tier 2 — Structural floor (B1.1 is start of B/C chain; must precede C tier)

- [x] **B1.1** JSONLRecord::Unknown variant + migration 007 *(no predecessors; B/C chain starts here)*  *— landed in 99ee138: manual Deserialize impl on JSONLRecord with two-pass dispatch; new Unknown { type_name, raw } variant; new decompose_unknown fn; new drift::log_record_type_drift fn; migration 007_record_type_drift.sql with UNIQUE(type_name, version); MIGRATIONS array registration; 5 new tests in record.rs + 5 new tests in drift.rs + 4 new tests in schema.rs; cargo test passes 44/44 core, 138/138 store. db.rs idempotency assertion bumped 6→7 (single-line coupling fix).*
  - [x] B1.1-Audit  *— completed 2026-05-09: 50-row deviation catalog + 5 plan-defect notes. Notable: R8 empty-discriminator-string test missing per plan §65; R16 commit-message uses definitive language "Closes…" / "to prevent…" against aims-vs-certainties rule; R18 record_type_drift_log absent from live DB (expected — daemon on prior binary, no kickstart); R36 commit subject 95 chars exceeds 72-char convention; R44 plan §17 record.rs:177-524 line range now stale post-edit; R48 Unknown-variant Serialize round-trip untested; R19/R20/R21/R22/R23/R24 several mechanical-coupling additions beyond plan §77 file list (db.rs, schema.rs assertion bump, manual Serialize impl, KnownRecordType helper, Unknown arm in log_record_overflow). cargo build clean, 44/44 + 138/138 tests pass, sqlite3 schema apply OK, idempotent replay OK, UNIQUE constraint error 19 confirmed, /v1/health 200, daemon undisturbed.*
  - [x] B1.1-Triage  *— completed 2026-05-09: 22 deviation rows + 5 meta-plan = 27 triaged. Categories: 2 omission, 11 divergence, 2 addition, 5 verification, 2 commit-message, 1 test-coverage, 5 meta-plan. Plan-spec reach balanced 9/9/9 (plan-defect/impl-defect/mutual). Build/test impact: 3 build-blocking, 6 test-blocking, 6 runtime-only, 12 static-only. 18 deterministic, 9 unverified (mostly pending daemon kickstart for migration 007 to apply to live DB). No severity language; grep self-check clean.*
  - [x] B1.1-Review  *— completed 2026-05-09. Decisions per deviation:*
    *— RESOLVED via cross-check: #25 (RECORD_TYPE_SAMPLE_MAX_LEN comment claim verified — drift.rs:23 has `MAX_SAMPLE_VALUE_LEN: usize = 500;` and decompose.rs:730 has `RECORD_TYPE_SAMPLE_MAX_LEN: usize = 500;` — comment is accurate)*
    *— IGNORE (mechanically required for compilation/test-pass; collectively resolve to the operational reality that migration registration touches schema_versions row count): #19 (db.rs assertion bump), #20 (schema.rs migration_006 test bump), #21 (manual Serialize impl required to keep Serialize working after switching from derive), #22 (rename attrs removed mechanically given manual impl), #23 (drift.rs Unknown arm for compiler exhaustiveness)*
    *— IGNORE (defensible engineering choices within reasonable scope): #24 (KnownRecordType helper enum is a clean factoring), #43 (serde_json::Value as pass-1 intermediate is reasonable for JSONL parser context)*
    *— RECORD (real but committed; cannot amend; future commits adopt the pattern): #16 (commit-message "Closes…" / "to prevent…" definitive language; future commits speak to aims and intentions per CLAUDE.md), #36 (commit subject 95 chars exceeds 72-char convention; future commits aim for ≤72 chars)*
    *— RECORD (verification gaps pending daemon kickstart — separately authorized step per CLAUDE.md post-build protocol): #18 (record_type_drift_log absent from live DB), #32 (no-new-WARN-categories check), #33 (regression baselines not captured pre-commit), #41 (corpus spot-check not performed), #45 (live-ingestion smoke not performed)*
    *— RECORD (test gaps to address in follow-up commit, not blocking B1.1 review closure): #8 (empty-discriminator-string test missing per plan §65; the impl path silently treats `""` as Unknown which is a defensible default but not asserted; add test in B1.2 or a separate small commit), #48 (Unknown-variant Serialize round-trip not tested; same handling)*
    *— RECORD (out-of-scope per plan §62 for B1.1; warrants separate future work): #28 (ContentBlock-level drift remains unaddressed; same architectural blind spot at inner discriminator level; flagged in audit report addendum and acknowledged in commit body line 13-14)*
    *— RESOLVED via parent-agent direct fix to plan: #44 + meta-plan-1 (plan §17 line range `record.rs:177-524` updated to test-name list); meta-plan-2 (plan §65 ambiguity for empty-discriminator-string clarified); meta-plan-3 (plan §77 file list overreach for B1.1 corrected to distinguish B1.1 vs B1.2 surfaces); meta-plan-5 (plan acknowledges mechanical-coupling edits like db.rs and schema.rs assertion bumps as in-scope by default for any migration-registering commit); #26 + #37 + meta-plan (plan §65 updated to acknowledge multi-file test distribution as project-pattern compliant)*
    *— DEFERRED (real but lower-leverage; flag for future plan revision): meta-plan-4 (regression baselines protocol absent; the plan calls for `/tmp/regress.*.before` files but doesn't say how to capture them. Pre-commit baseline capture would be a useful CLI subcommand or hook. Out of scope for B1.1's review.)*
    *Net: B1.1 implementation accepted as-is. Plan refinements applied. 5 verification gaps will resolve after daemon kickstart (next user-authorized step). 2 test gaps and 1 out-of-scope item recorded for future work. B1.1-Review unblocks B1.2 once user authorizes.*
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

- [x] **D1** Move version_history backfill into sync_all *(no predecessors)*  *— landed in 6a91894: 38-line INSERT OR IGNORE block at end of sync_all (between "Sync complete" tracing and FTS rebuild); regression test test_sync_all_backfills_version_history with 3-version fixture asserting per-version session_count from sessions table. cargo test 129/129 store.*
  - [x] D1-Audit  *— completed 2026-05-09: 22-row catalog. Notable: D1 production block ~36 LOC vs plan estimate "~15 LOC"; regression test (~110 LOC) lives in D1 but plan §167 assigns regression-test commit ownership to D4 (internal plan inconsistency); plan §164 wording "Move" vs impl additive interpretation (watcher's startup backfill kept; impl defers removal to D2); D1 runs backfill unconditionally on no-op syncs (defensible per impl: INSERT OR IGNORE makes it cheap); 5 verification gaps pending daemon kickstart; commit message conforms to all project conventions (61-char subject, no Co-Authored-By, no definitive language). Categories used: divergence, addition, sequence, verification, test-coverage, commit-message, documentation, meta-plan.*
  - [x] D1-Triage  *— completed 2026-05-09: 14 dev rows + 5 meta-plan = 19. 0 omission, 6 divergence, 1 addition, 2 verification, 2 test-coverage, 8 meta-plan. 0 build-blocking, 0 test-blocking, 3 runtime-only, 16 static-only — minimal runtime impact, mostly observational. 6 plan-defect / 8 impl-defect / 5 mutual.*
  - [x] D1-Review  *— completed 2026-05-09. Decisions per deviation:*
    *— RESOLVED via parent-agent direct fix to plan: mp-1 (test ownership ambiguity D1 vs D4 — plan §164 updated to acknowledge D1 owns the version_history-specific regression test, D4 owns the integration test exercising D1+D3 together); mp-2 (Move vs Add — plan §164 updated from "Move" to "Add" with explicit deferral of watcher's backfill removal to D2); mp-3 (no-op sync behavior — plan §164 updated to specify backfill runs unconditionally on every sync_all invocation, INSERT OR IGNORE makes it cheap)*
    *— RESOLVED via prior commit a6edb61 (operational policy): mp-4 (procedure for verifying independent commit when workspace uncompilable — the parallel-spawn batching policy in MEMORY.md and subagent-prompt-template.md addresses exactly this case)*
    *— DEFERRED (lower-leverage; future plan-text revision): 20 + mp-5 (audit doc cited as D1 input has no D1-specific content; could update plan §221 input list to remove the audit-doc reference for D1 since the plan section itself is sufficient)*
    *— IGNORE (cosmetic, behaviorally equivalent): #3 (closure-binding identifier `\|c\|` vs `\|conn\|`), #4 (log strings differ from watcher's — site-distinguishable and appropriate)*
    *— IGNORE (test is correct, conforms to plan): #7 (placement matches commit msg), #13 (test fixture invariant of session_count=1 correctly asserted)*
    *— IGNORE (correctly scoped out per plan): #14 (OR-IGNORE-pre-existing-row case is D3 territory)*
    *— RECORD (LOC estimate vs actual; minor): #1 (plan ~15 LOC vs actual 36 LOC for production block — plan estimates updated above to ~30-40 LOC)*
    *— RECORD (verification gaps pending daemon kickstart, same pattern as B1.1): #6 (live-verify procedure), #8 (regression baselines)*
    *— RECORD (intentional engineering choice; plan now reflects): #2 (regression test in D1 — resolved by mp-1 plan update), #5 (additive interpretation — resolved by mp-2 plan update), #9 (no-op sync backfill — resolved by mp-3 plan update)*
    *— IGNORE (non-actionable observation): #19 (workspace-broken-during-verification — resolved by policy commit a6edb61), #22 (test count discrepancy 129 vs 138 — explained by other commits between D1 and audit time)*
    *Net: D1 implementation accepted as-is. Plan refinements applied to §164. 2 verification gaps pending daemon kickstart (separate user-authorized step). D1-Review unblocks D4 once D3 also reaches review.*
- [x] **D2** Sequence the daemon-startup race *(no predecessors)*  *— landed in b7e18d1: tokio::sync::oneshot signaling between serve.rs's sync_all task and watcher_loop's startup backfill query. Sender fires after sync_all returns regardless of Ok/Err. Receiver awaited as first action in watcher_loop before issuing the INSERT OR IGNORE backfill SQL. Live filesystem-event arm and cancellation arm intentionally NOT gated on the signal — only the one-shot startup backfill query waits. API listener binds and serves concurrently. Regression test watcher::tests::watcher_startup_backfill_waits_for_sync_all_signal asserts no race: spawns loop, inserts sessions row AFTER spawn but BEFORE firing oneshot, sleeps, asserts version_history still empty (no race), fires oneshot, polls until non-empty with 1s deadline, asserts observation timestamp at-or-after signal-fire timestamp. Two files changed: serve.rs (+42), watcher.rs (+144). Build clean, regression test passes. Daemon kickstart pending — current PID 2344 still on pre-99ee138 binary.*
  - [x] D2-Audit  *— completed 2026-05-09: 25-row catalog. Notable: D2 production ~30 LOC matches plan estimate; +117 lines is regression test (plan §167 assigns regression-test ownership to D4 only — same scope-blur as D1 absorbing its own test); plan §D2 verification line 314 enumerates boot-log strings that do not match actual code on the cold-boot path (plan: "Filesystem watcher established", "Starting sync", "Sync complete" vs actual: "File watcher started for projects directory" at serve.rs:134, "Startup sync completed" at serve.rs:174); functional gate works correctly via tokio::sync::oneshot; race-detector test load-bearing assertion is the pre-signal assert_eq!(0) at watcher.rs:809-812; subject 73 chars (one over conventional 72); 5 verification gaps pending daemon kickstart; initial_version query at watcher.rs:487-497 has the same cold-boot race shape as the now-gated backfill but is NOT gated (out of plan scope but observable). Linear history confirmed (parent 636aa9b). Categories used: divergence, addition, sequence, verification, test-coverage, commit-message, documentation, dependency.*
  - [x] D2-Triage  *— completed 2026-05-09: 24 triaged (20 dev + 4 meta-plan). 0 omission, 5 divergence, 1 addition, 3 sequence, 1 dependency, 1 verification, 1 regression, 1 commit-message, 1 documentation, 6 test-coverage, 4 meta-plan. 1 build-blocking, 6 test-blocking, 8 runtime-only, 9 static-only. 16 deterministic, 4 nondeterministic, 4 unverified. 5 plan-defect, 12 impl-defect, 7 mutual.*
  - [x] D2-Review  *— completed 2026-05-09. Decisions per deviation:*
    *— RESOLVED via parent-agent direct fix to plan: mp-1 + mp-2 + #3 + #4 (plan §314 boot-log strings updated to match actual code: "File watcher started for projects directory" replaces "Filesystem watcher established"; "Startup sync completed" added between "Sync complete" and "version_history backfill completed on startup" to reflect the gate's release point); mp-3 (plan §244 D2 line clarified — each D-tier item may include its own focused regression test in the file it modifies; D4 owns the integration test); mp-4 (plan §244 D2 LOC clarification — ~30 LOC is production-code only; regression tests additive)*
    *— IGNORE (mechanically required for the gate; only internal callsite): #22 (public-fn signature widening on watcher_loop)*
    *— IGNORE (defensible engineering choices, audit confirmed semantics match commit-message claims): #13 (expect panic message; load-bearing invariant assertion that cannot fire under normal control flow), #21 (warn-on-Err falls through to backfill; matches message claim), #14 (commit-message describes conceptual order, runtime is the same just with additional intermediate log strings)*
    *— IGNORE (test design observations; non-blocking): #8 (PID-based test DB path — cargo serializes tests in same process), #10 + #11 (race-detector reasoning relies on init_db sequence — the LOAD-BEARING assertion is correctly the post-signal poll, pre-signal is corroborating not load-bearing), #23 + #24 (mpsc shutdown paths and wall-clock assertion semantics)*
    *— RECORD (real but minor): #9 (1s wall-clock deadline in test could flake under heavy CI load; acceptable for now), #16 (subject 73 chars, one over conventional 72; same pattern as B1.1's 95-char subject — future commits aim for ≤72)*
    *— RECORD (out-of-scope per plan §165 but worth flagging): #12 (initial_version query at watcher.rs:487-497 has the same cold-boot race shape as the now-gated backfill; not gated by D2 because plan §165 only requires gating the backfill query; recorded as future-work analogous to ContentBlock-level drift recorded for B1.1)*
    *— RECORD (verification gaps pending daemon kickstart, same pattern as B1.1 + D1): #5 (live boot-log verification), #17 (regression baselines)*
    *— IGNORE (informational, not deviations): #19 (D2 committed alone per operational policy — confirms compliance), #20 (no documentation changes — plan §D2 doesn't require any), #2 (regression test addition — resolved by mp-3 plan fix)*
    *— RECORD (LOC estimate vs actual; same pattern as D1): #1 (plan ~30 LOC vs 185 insertions — resolved by mp-4 plan fix clarifying production-code-only estimate)*
    *Net: D2 implementation accepted as-is. Plan refinements applied to §314 (boot-log strings) and §244 (LOC + regression test scope). 2 verification gaps pending daemon kickstart. 1 future-work item recorded (#12 ungated initial_version query). D2-Review unblocks D3 once D3 reaches review and D4 once both D1+D3 reviews close.*
- [ ] **D3** Unify session_count semantics *(no predecessors)*  *— spawned 2026-05-09 in parallel with B1.1+D1+D2 batch; same race-induced stop; WIP discarded; reverted to pending*
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
