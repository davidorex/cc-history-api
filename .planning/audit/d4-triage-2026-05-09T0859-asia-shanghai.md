# D4 Triage — commit `21ec5b6`

**Triage timestamp:** 2026-05-09T08:59 Asia/Shanghai (UTC+0800)
**Audit input:** `/Users/david/Projects/cc-history-api/.planning/audit/d4-audit-2026-05-09T0853-asia-shanghai.md` (38 observations)
**Plan reference:** `/Users/david/.claude/plans/curious-napping-koala.md` (§163–§167 Tier 4, §244 file table, §307–§323 verification, §192–§196 spawn-order DAG)
**Mode:** Structural classification along three axes — plan-spec reach, build/test impact, determinism. No severity language. No accept/reject decisions.

---

## Axis totals

**Plan-spec reach:**
- plan-defect: 4 (#1, #27, #28, #36)
- impl-defect: 0
- mutual: 0
- informational: 34

**Build/test impact:**
- build-blocking: 0
- test-blocking: 0
- runtime-only: 0
- static-only: 5 (#2, #10, #19, #20, #21)
- none: 33

**Determinism:**
- deterministic: 36
- nondeterministic: 0
- unverified: 2 (#37, #38)

**Meta-plan rows:** 3 (#27, #28, #36)
**Unverified rows pending daemon kickstart:** 2 (#37, #38)

---

## Triage table

| # | Audit category | Plan-spec reach | Build/test impact | Determinism | Rationale |
|---|---|---|---|---|---|
| 1 | divergence | plan-defect | none | deterministic | Plan §167 estimate "~50 LOC" understates the comment-heavy regression-test pattern that prior D-tier reviews accepted; no impl wrongness, the spec text is stale. |
| 2 | addition | informational | static-only | deterministic | ~50-line doc-comment block above the test function; sits in source but does not affect compile or runtime — purely textual. |
| 3 | test-coverage | informational | none | deterministic | Discriminating-assertion math traced and shown to distinguish D3's COUNT(*) re-derive from a hypothetical pre-D3 +1 path; an observation about test fitness, not a defect. |
| 4 | test-coverage | informational | none | deterministic | Comparison of D4-test pre-call shape vs D3-test stale-plant shape; observation of test design, no contract on either. |
| 5 | addition | informational | none | deterministic | Best-effort cleanup pattern matches D2/D3 precedent — pattern-compliance observation. |
| 6 | divergence | informational | none | deterministic | D4 inlines JSONL strings rather than reusing D1's store-crate-private helpers; cross-crate visibility forces inlining, not a contract miss. |
| 7 | test-coverage | informational | none | deterministic | Phase-2 raw-SQL inserts bypass the decompose path by design and acknowledged in code comment. |
| 8 | test-coverage | informational | none | deterministic | Carries forward D3-Audit row #16 assertion-specificity gap (last_seen_at, session_id preservation, total-row-count post-call); already a recorded deferred follow-up. |
| 9 | addition | informational | none | deterministic | Test name length and convention match watcher.rs precedent. |
| 10 | addition | informational | static-only | deterministic | ~39% comment density in source file; affects file size only, not compile or runtime. |
| 11 | addition | informational | none | deterministic | CancellationToken intentionally absent — test does not spawn a task; pattern matches D3-test. |
| 12 | addition | informational | none | deterministic | Doc-comment justification of "Option A" placement; consistent with Cargo DAG (server depends on store, not vice versa). |
| 13 | test-coverage | informational | none | deterministic | Test is partially end-to-end (Phase 1) and partially unit-level (Phase 2); plan §167 wording does not mandate a stricter shape. |
| 14 | test-coverage | informational | none | deterministic | PID-scoped temp paths plus remove-then-create at test start prevent collision; pattern matches D2/D3. |
| 15 | verification | informational | none | deterministic | Plan §322 explicitly defines D4's live-verify as the test-suite gate; cargo test 3/3 satisfies it. |
| 16 | verification | informational | none | deterministic | Daemon at PID 2344 runs a pre-D4 binary; D4 is test-only so daemon-state inconsistency is not on D4's path; gap inherited from prior D-tier audits. |
| 17 | test-coverage | informational | none | deterministic | Phase-1 COUNT(*)==3 plus per-version session_count==1 indirectly pins the three rows; an observation about assertion shape directness. |
| 18 | commit-message | informational | none | deterministic | Subject 57 chars, conventional-commits prefix, item-tag suffix; no forbidden definitive language. |
| 19 | commit-message | informational | static-only | deterministic | Body sections largely duplicate the in-code doc-comment content; no impact on build or runtime, only on text artifacts. |
| 20 | documentation | informational | none | deterministic | Single-file working set (`crates/server/src/watcher.rs`); plan does not mandate D4 doc edits. |
| 21 | sequence | informational | static-only | deterministic | Linear history B1.1→D1→D2→D3→D4 honors the spawn-order DAG; structural fact about commit graph. |
| 22 | dependency | informational | none | deterministic | `claude_history_store::sync::sync_all` already public via prior usage at serve.rs / main.rs; no Cargo.toml change required. |
| 23 | test-coverage | informational | none | deterministic | Fixture JSONL shape matches `JSONLRecord::User` minimal-required-fields; cargo test 3/3 confirms deserialization. |
| 24 | test-coverage | informational | none | deterministic | Test isolated from production DB and daemon via PID-scoped temp paths. |
| 25 | verification | informational | none | deterministic | `PRAGMA integrity_check` and `foreign_key_check` return ok; D4 did not touch production DB. |
| 26 | verification | informational | none | deterministic | §435 sync no-op safeguard is mechanically inapplicable to a test-only commit; recorded as such. |
| 27 | meta-plan | plan-defect | none | deterministic | Same LOC-estimate-vs-actual pattern flagged in D1/D2/D3 audits; plan §167 estimate text not refined despite three prior surfacings. |
| 28 | meta-plan | plan-defect | none | deterministic | Cross-section coherence across §163, §164–§167, §244, §320–§322 holds; only §167 LOC budget is stale per #27. |
| 29 | addition | informational | none | deterministic | WatcherState construction with manual broadcast::channel matches D3-test pattern byte-identically. |
| 30 | test-coverage | informational | none | deterministic | Untouched-row invariant for 2.1.100 / 2.1.101 is one-sided; no Phase-2 post-call total-row-count assertion (related to #17). |
| 31 | verification | informational | none | deterministic | D2 gate code byte-identical pre/post D4 per V7 empty diff. |
| 32 | verification | informational | none | deterministic | D3 ON CONFLICT SQL byte-identical pre/post D4 per V7 empty diff. |
| 33 | verification | informational | none | deterministic | D1 sync_all backfill byte-identical pre/post D4 per V7 empty diff. |
| 34 | test-coverage | informational | none | deterministic | FTS5 rebuild side-effect not asserted; D1's own test similarly does not assert it; pattern consistent. |
| 35 | test-coverage | informational | none | deterministic | Phase-2 `pre_check_vh_count == 1` precondition is load-bearing for the discriminating-math chain; observation about test robustness. |
| 36 | meta-plan | plan-defect | none | deterministic | D4-Review unblock criterion is leaf-node by structural fact (no downstream node in §192–§196); plan does not need to enumerate it. Classified plan-defect because the gating record lives in STATE.md rather than plan, but coherent given plan's DAG section. |
| 37 | verification | informational | none | unverified | §328 live-ingestion latency check not exercised; daemon on pre-99ee138 binary makes it inapplicable until kickstart. |
| 38 | verification | informational | none | unverified | §329 no-new-WARN check not exercised; same daemon-state precondition as #37. |

---

## Meta-plan subsection

The audit's three `meta-plan` rows describe plan-internal coherence rather than commit-vs-plan deviations:

- **#27** — Plan §167 LOC estimate "~50 LOC" vs actual +245 LOC. Same understatement pattern recurred across D1 (15 vs 36+110), D2 (30 vs 30+117), D3 (5 vs 1+89). The plan text has not been refined despite three prior surfacings.
- **#28** — Cross-section coherence across §163 (Tier 4 intro), §164–§167 (per-item), §244 (file table), §320–§322 (verification). The audit finds internal coherence holds; only §167's LOC budget is stale (per #27).
- **#36** — D4-Review unblock criterion. Plan §192–§196 places `[D4]` as a leaf node with no downstream arrow; STATE.md row #194 records D4 as the terminal node. No contradiction; the gating fact lives in STATE.md rather than the plan body.

All three classify as `plan-defect` along plan-spec reach (per the template's definition: "the plan was wrong"). #28 and #36 surface coherence rather than wrongness; they are flagged plan-defect because remediation, if any, would be a plan edit.

---

## Notes

- Row #1 (divergence: LOC budget overshoot) duplicates #27's underlying observation but is filed as `divergence` by the audit because it compares the commit against the plan estimate rather than commenting on the plan's internal coherence. Triaged identically to #27 along plan-spec reach.
- Rows #37 and #38 are the only `unverified` entries; both depend on a daemon kickstart that has not occurred. The deterministic verification commands V1–V14 all returned exit 0 in the audit.
- No row classifies as `build-blocking`, `test-blocking`, or `runtime-only`. D4 is a test-only commit per plan §322 and audit V7 confirms no production code outside `mod tests` was modified; cargo test 3/3 passed (V9, V10).
