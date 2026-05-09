# D4 Adversarial Audit — commit `21ec5b6`

**Audit timestamp:** 2026-05-09T08:53 Asia/Shanghai (UTC+0800)
**Commit audited:** `21ec5b6` — `test(watcher): add D1+D3 integration regression test (D4)`
**Plan reference:** `/Users/david/.claude/plans/curious-napping-koala.md` (Tier 4 D4, §167; verification §320–§323; safeguards §16–§21)
**Mode:** Coverage-of-finding catalog. No triage, no prioritization, no remediation.

---

## Verification command results

| # | Command | Exit | Summary |
|---|---|---|---|
| V1 | `git show 21ec5b6 --stat` | 0 | 1 file changed: `crates/server/src/watcher.rs` +245 insertions, 0 deletions |
| V2 | `git show 21ec5b6 --format=fuller \| head -30` | 0 | Author + Commit `David Ryan <davidryan@gmail.com>`; AuthorDate == CommitDate `Sat May 9 08:46:13 2026 +0800`; subject `test(watcher): add D1+D3 integration regression test (D4)` |
| V3 | `git log --oneline 21ec5b6 -10` | 0 | Linear history confirmed; parent is `8e87e56 docs(planning): close D3 review checkpoint`; predecessors include 2e44080 (D3), b7e18d1 (D2), 6a91894 (D1), 99ee138 (B1.1) |
| V4 | `git diff 8e87e56..21ec5b6 -- crates/server/src/watcher.rs \| head -300` | 0 | All hunks land in the existing `#[cfg(test)] mod tests` block at the bottom of `watcher.rs`; new function `d4_integration_sync_all_then_live_check_version_change` |
| V5 | `git diff 8e87e56..21ec5b6 -- crates/server/src/watcher.rs \| wc -l` | 0 | 254 diff lines (245 + diff envelope) |
| V6 | `git diff 8e87e56..21ec5b6 --stat` | 0 | Single-file change matching V1 |
| V7 | `git diff 8e87e56..21ec5b6 -- 'crates/store/**' 'crates/core/**' 'crates/server/src/serve.rs' 'crates/server/src/main.rs' 'crates/server/src/api/**' \| head -50` | 0 | Empty output — no production code outside `watcher.rs` modified; D1's `sync.rs` backfill, D2's `serve.rs` gate, and D3's `check_version_change` ON CONFLICT are byte-identical to their post-commit states |
| V8 | `cargo build --release 2>&1 \| tail -5` | 0 | `Finished release profile [optimized] target(s) in 0.05s` (already-built artifact reused) |
| V9 | `cargo test -p claude-history 2>&1 \| tail -15` | 0 | 3/3 tests pass: D2 gate, D3 re-derive, D4 integration |
| V10 | `cargo test --package claude-history watcher::tests 2>&1 \| tail -20` | 0 | Same 3/3 pass result; total runtime 0.10s |
| V11 | `git log -n 1 21ec5b6 --format=%s \| awk '{print "subject_chars=" length}'` | 0 | `subject_chars=57` (under conventional 72 limit) |
| V12 | `curl -s http://127.0.0.1:7424/v1/health \| head -3` | 0 | `{"status":"ok","db_size":3316375552,"record_count":733285,"version":"0.1.0"}` |
| V13 | `launchctl list \| grep claude-history` | 0 | PID 2344 with last exit-status -9 (SIGKILL); throttle-recovered. Logrotate sibling agent also listed |
| V14 | `sqlite3 ~/.claude/.claude-history.db "PRAGMA integrity_check; PRAGMA foreign_key_check;"` | 0 | `ok` (no foreign-key violation rows printed) |

All 14 verification commands succeeded. None failed.

---

## Deviation catalog

### #1. [divergence] LOC budget overshoot vs plan §167's "~50 LOC" estimate
- **Plan-spec ref:** §167 — "D4. Regression test locking in correctness for D1/D3 (~50 LOC)"
- **Actual state:** `+245 LOC` in a single test function (`git show 21ec5b6 --stat` confirms 245 insertions, 0 deletions; `git diff … wc -l` shows 254 envelope lines)
- **Observation:** The implementation is approximately 4.9× the plan's stated budget. The expansion is comment-heavy (multi-paragraph doc-comment block on the function plus inline section markers like `// ----- Phase 1 — D1 surface -----`, plus inline rationale strings inside `assert_eq!` macros). A roughly comparable per-test in-file precedent is D3's test at watcher.rs:872–958 (~87 LOC) and D2's test at watcher.rs:763–869 (~107 LOC). D4's test exceeds both by 2.3–2.8×.
- **Determinism:** deterministic (LOC counts measured against fixed commit SHAs)
- **Verification evidence:** V1, V5, V6 confirm the count; comparison to D2/D3 made by reading watcher.rs:763–958.

### #2. [addition] Multi-paragraph doc-comment block above the test function (~50 lines of comments)
- **Plan-spec ref:** N/A
- **Actual state:** Lines 960–991 in the new test consist of 50+ comment lines articulating "Why this file (Option A)", "Setup (load-bearing math)" with an indented Phase 1 / Phase 2 explanation, and "Discriminating assertion" math expansion (commit `21ec5b6` watcher.rs hunk — doc-comment precedes `#[tokio::test]`)
- **Observation:** The plan does not require nor preclude such a comment block. D2's test has a ~12-line doc comment; D3's test has a ~13-line doc comment; D4's is roughly 4× longer. The comment block duplicates content that already appears in the commit message body of `21ec5b6`.
- **Determinism:** deterministic
- **Verification evidence:** Diff at V4 lines 956–1002 (mod tests block); commit-message body inspected via V2 longer form.

### #3. [test-coverage] Discriminating assertion verified to distinguish D3 from pre-D3 path
- **Plan-spec ref:** §167 + the D4-specific framing ("locking in correctness for D1/D3")
- **Actual state:** Test computes COUNT(*) FROM sessions WHERE version='2.1.99' = 3 after Phase 2 inserts. Calls `check_version_change` with `last_known_version = "2.1.0-prior"` and `session_id = "d4-session-99-extra-a"` (which has version `2.1.99` in sessions, so the changed branch fires). Asserts `session_count == 3`.
- **Observation:** Math traced against D3's actual ON CONFLICT SQL at watcher.rs:434–438:
  - INSERT side: `VALUES (?1, datetime('now'), datetime('now'), ?2, 1)` — would attempt insert with session_count=1 on conflict, so the ON CONFLICT clause is taken.
  - ON CONFLICT clause: `session_count = (SELECT COUNT(*) FROM sessions WHERE version = ?1)` where `?1 = '2.1.99'`. Sessions table holds 3 rows (1 from sync_all + 2 directly inserted), so COUNT=3. Asserted value = 3. ✓
  - Hypothetical pre-D3 path (`session_count = version_history.session_count + 1`) would yield 1 (D1 backfilled value) + 1 = 2. Assertion `==3` would fail. ✓
  The assertion is correctly load-bearing for D3 and would not false-pass under a pre-D3 +1 increment. The phase-2 setup (insert TWO additional sessions, not one) is specifically chosen to avoid the pre-D3 +1 = 2 false-pass that a single-session phase-2 would produce.
- **Determinism:** deterministic
- **Verification evidence:** V4 hunk lines (Phase 2 setup + ON CONFLICT SQL); cross-read of watcher.rs:432–441; D3 commit `2e44080` SQL byte-identical per V7.

### #4. [test-coverage] D4 vs D3-test discriminating differences
- **Plan-spec ref:** N/A (project-pattern compliance per §167's framing)
- **Actual state:** D3-test at watcher.rs:885–958 plants a STALE version_history row with `session_count = 0` for version 2.1.99, then asserts post-call value = 2 (sessions truth). D4 test does NOT plant a stale row; it relies on D1's sync_all backfill to write the row at session_count=1, then re-fires check_version_change to verify the COUNT(*) re-derive.
- **Observation:** D4's pre-call state is `session_count=1` (D1 truth-from-COUNT at Phase 1) plus 2 added sessions = 3 actual; D3's pre-call state is `session_count=0` (manual stale plant) plus 2 sessions = 2 actual. The shapes are deliberately different (D4 exercises D1's path then D3's; D3-test exercises only D3). No accidental cross-cutting could allow D4 to pass under pre-D3 conditions: a pre-D3 `+1` path would yield `1+1=2 ≠ 3` and fail.
- **Determinism:** deterministic
- **Verification evidence:** Direct read of D3 test at watcher.rs:911–921 vs D4 test at watcher.rs:1018–1041 in commit-tree state.

### #5. [addition] Best-effort cleanup pattern matches D2/D3 precedent
- **Plan-spec ref:** N/A
- **Actual state:** Test ends with `let _ = std::fs::remove_file(&db_path); let _ = std::fs::remove_dir_all(&projects_dir);` (no panic on cleanup failure; PID-scoped paths). D2/D3 tests have similar best-effort cleanup of their respective paths.
- **Observation:** Cleanup behavior matches established project pattern. Test does not use `tempfile::tempdir` (used by D1's sync.rs test); D4 uses raw `std::env::temp_dir()` with PID-scoped subdir, matching D2/D3 watcher.rs convention.
- **Determinism:** deterministic
- **Verification evidence:** V4 hunk last 3 lines; comparison to watcher.rs:773–795 (D2) and watcher.rs:888–891 (D3).

### #6. [divergence] D1 test fixture pattern not mirrored — D4 inlines JSONL strings
- **Plan-spec ref:** N/A (project-pattern compliance considered)
- **Actual state:** D1's regression test in `crates/store/src/sync.rs` uses `tempfile::tempdir`, the helper `valid_user_line_with_version(uuid, session_id, version)` and `write_jsonl_file(dir, filename, content)` (sync.rs:459, 482). D4 uses raw `std::fs::write` with inline `format!(r#"..."#)` strings, no helper extraction.
- **Observation:** D4 lives in the server crate which does not have access to those store-crate-private test helpers. The inlined JSONL string at watcher.rs:1005–1014 hardcodes the same JSONLRecord-deserializable shape as `valid_user_line_with_version` but with a different timestamp (2026-02-20 vs 2026-02-20 — actually the same; both use `2026-02-20T01:00:00Z`). Both shapes deserialize as `JSONLRecord::User`.
- **Determinism:** deterministic
- **Verification evidence:** V4 vs sync.rs:482–488 read directly.

### #7. [test-coverage] Phase 2 inserts use non-D1-backfill path (raw SQL)
- **Plan-spec ref:** N/A
- **Actual state:** Phase 2 inserts 2 sessions at version 2.1.99 via `INSERT INTO sessions (...) VALUES ('d4-session-99-extra-a', ...);` directly, NOT through `sync_file` or `sync_all`. The comment at watcher.rs:1056–1059 acknowledges this: "phase-2 setup invariant: version_history row for 2.1.99 still says 1 (D1 wrote it; raw sessions inserts did not update it)".
- **Observation:** Direct SQL insert into `sessions` bypasses the decompose path. This is the intended design — it makes the version_history-vs-sessions divergence observable without re-running sync_all. A reader unfamiliar with the architecture could mistakenly believe Phase 2 represents real "live" ingestion; the comment block addresses this explicitly. Since `check_version_change` only reads `sessions.version` to dispatch the upsert, the bypass is functionally equivalent for the purposes of this test.
- **Determinism:** deterministic
- **Verification evidence:** V4 hunk lines for Phase 2 INSERT (watcher.rs:1042–1053) + check_version_change SQL at watcher.rs:387–394.

### #8. [test-coverage] Assertion specificity gap (carried forward from D3-Audit row #16)
- **Plan-spec ref:** N/A
- **Actual state:** D4 asserts:
  - `version_history` row count post-Phase-1 == 3
  - per-version session_count post-Phase-1 == 1
  - sessions table COUNT(*) at 2.1.99 post-Phase-2 == 3 (sanity check)
  - version_history row at 2.1.99 still == 1 pre-call (sanity check)
  - version_history row at 2.1.99 post-call == 3 (load-bearing)
  - version_history rows at 2.1.100 / 2.1.101 still == 1 post-call (untouched-row invariant)
  Does NOT assert: `last_seen_at` advancement post-call; `session_id` value preservation post-call; `first_seen_at` not advanced; `version_history` total row count unchanged at 3 post-call (i.e., no spurious row creation).
- **Observation:** The same gap was flagged in D3-Audit row #16 against D3's own test. D4 does not surface those additional ON CONFLICT semantic effects. STATE.md D3-Review records this as a deferred follow-up.
- **Determinism:** deterministic
- **Verification evidence:** V4 hunk full read; STATE.md:185 D3-Audit row #16 reference.

### #9. [addition] Test name length: 56 chars vs project test-name conventions
- **Plan-spec ref:** N/A
- **Actual state:** `d4_integration_sync_all_then_live_check_version_change` (56 chars).
- **Observation:** Test names in watcher.rs include `check_version_change_session_count_re_derives_from_sessions_truth` (D3, 64 chars) and `watcher_startup_backfill_waits_for_sync_all_signal` (D2, 51 chars). D4's name length and convention (descriptive, snake-case, item-tag-prefixed `d4_…`) match the precedent.
- **Determinism:** deterministic
- **Verification evidence:** V9 test enumeration.

### #10. [addition] Comment-density ratio in D4 test
- **Plan-spec ref:** N/A
- **Actual state:** Of 245 inserted lines: comment lines (including `///` and `//`) approximately 95; blank lines approximately 25; code/assertion lines approximately 125 (rough count from V4 hunk).
- **Observation:** Roughly 39% comment density. D2's test has roughly 28% comment density; D3's test roughly 22%. D4 is the comment-heaviest of the three D-tier in-file regression tests.
- **Determinism:** deterministic (counts approximate; rounding ±5%)
- **Verification evidence:** V4 hunk read line-by-line; D2/D3 comparison via watcher.rs:763–958.

### #11. [addition] CancellationToken absent from D4 (no task spawn)
- **Plan-spec ref:** N/A (template item g)
- **Actual state:** Test does not call `tokio::spawn` and does not construct or use `tokio_util::sync::CancellationToken`. The test's doc comment at watcher.rs:990–992 acknowledges this: "The test does not exercise watcher_loop or any spawned task — it calls sync_all and check_version_change directly. No CancellationToken is required because no task is spawned."
- **Observation:** No leaked-task risk; pattern matches D3 test (which also calls check_version_change directly without spawn). D2 test does spawn watcher_loop and does use CancellationToken at watcher.rs:864.
- **Determinism:** deterministic
- **Verification evidence:** V4 hunk grep for `tokio::spawn` and `CancellationToken` (both absent).

### #12. [addition] Test-placement justification stated in code, not just commit message
- **Plan-spec ref:** N/A
- **Actual state:** Doc comment at watcher.rs:967–977 explains the placement choice ("Option A"): hosting the test inside `watcher::tests` lets it call `check_version_change` directly without widening that function's visibility; reverse placement in store would invert the workspace dependency direction.
- **Observation:** Justification is internally consistent. server/Cargo.toml:12 confirms `claude-history-store = { path = "../store" }` — store does NOT depend on server, so reverse placement is indeed not viable per Cargo's DAG. `claude_history_store::sync::sync_all` is publicly accessible from server (already used at serve.rs:166 and main.rs:845).
- **Determinism:** deterministic
- **Verification evidence:** server/Cargo.toml read; existing usage of `claude_history_store::sync::sync_all` per V7-adjacent grep.

### #13. [test-coverage] sync_all integration vs unit-stitched test
- **Plan-spec ref:** §167 ("exercises D1+D3 together")
- **Actual state:** Phase 1 exercises sync_all end-to-end (real JSONL fixtures → sync_all → version_history backfill). Phase 2 calls check_version_change directly (not via watcher_loop's filesystem-event path). The test is partially end-to-end (Phase 1) and partially unit-level (Phase 2).
- **Observation:** The plan's "exercises D1+D3 together" language is satisfied by exercising both code paths in sequence against one shared DB; the plan does not mandate that Phase 2 also be end-to-end via the filesystem-event path. A stricter integration shape would spawn `watcher_loop`, write a JSONL update with new sessions, and assert version_history convergence — that's substantially closer to D2's test pattern but heavier than what D4 implements.
- **Determinism:** deterministic
- **Verification evidence:** V4 hunk + watcher_loop construction at watcher.rs:487–507.

### #14. [test-coverage] Concurrent execution between two `d4_…` test runs in CI
- **Plan-spec ref:** N/A
- **Actual state:** Test uses PID-scoped paths (`d4-projects-{pid}` and `d4-test-{pid}.db`). Cleanup at end is best-effort.
- **Observation:** PID scoping prevents two parallel cargo-test invocations from colliding on the same machine in the same parent process. However, two separate `cargo test` invocations launched in parallel (different parent PIDs) would still get distinct PID-scoped paths. Within a single cargo-test process, `cargo test` runs tests in parallel by default — but D4 is the only `d4_…` test, so it cannot collide with itself. Path collision risk: if a previous run crashed without cleanup, the leftover dir is `remove_dir_all`-ed at test start (watcher.rs:996); same for the DB file. The pattern is robust.
- **Determinism:** deterministic
- **Verification evidence:** V4 hunk lines 994–1000 (path scoping + remove-then-create); D2/D3 use the same pattern.

### #15. [verification] Live-system verification per plan §322 deferred to test-only
- **Plan-spec ref:** §322 — "**Live verify**: nothing additional — the test is the verification."
- **Actual state:** Plan explicitly states D4's live-verify gate is the test passing. V9/V10 confirm 3/3 watcher tests pass post-D4. No daemon kickstart, no fresh-session smoke test, no cross-cutting verification beyond cargo test.
- **Observation:** Per the plan, this is the design — D4 is test-only, not a deployment-relevant change. Plan §322 is a tighter live-verify spec than D1/D2/D3 received.
- **Determinism:** deterministic
- **Verification evidence:** Plan §320–§323 read; V9, V10 results.

### #16. [verification] Daemon at PID 2344 is on a pre-D4 binary
- **Plan-spec ref:** N/A (cross-cutting verification §327–§332)
- **Actual state:** `launchctl list` (V13) shows PID 2344 with exit-status `-9`; STATE.md D2 row records "current PID 2344 still on pre-99ee138 binary". V12 `/v1/health` returns `version 0.1.0` and `db_size 3316375552`. No daemon kickstart performed.
- **Observation:** D4 is test-only and does not modify the production daemon binary, so daemon-state inconsistency is moot for D4 itself. The cross-cutting "Daemon respawn survival" check (§330) is not exercised. Audit notes for D1/D2/D3 record verification gaps "pending daemon kickstart" — D4 carries the same gap forward by inheritance.
- **Determinism:** deterministic
- **Verification evidence:** V12, V13; STATE.md:170 ("Daemon kickstart pending — current PID 2344").

### #17. [test-coverage] Phase-1 `vh_count == 3` assertion does not pin per-row content
- **Plan-spec ref:** N/A
- **Actual state:** Phase 1 asserts `COUNT(*) FROM version_history == 3` then iterates the fixture array asserting per-version `session_count == 1`. It does not assert that the three rows have distinct version strings or that there are no extra null/empty-version rows beyond the three expected.
- **Observation:** The COUNT(*)==3 assertion plus per-version session_count==1 (across exactly 3 named versions) implicitly forces the three rows to be the expected three, since extra rows would push count above 3. The assertion shape is sound but slightly indirect; an explicit `SELECT version FROM version_history ORDER BY version` returning `["2.1.99", "2.1.100", "2.1.101"]` would be more direct.
- **Determinism:** deterministic
- **Verification evidence:** V4 hunk lines 1024–1041.

### #18. [commit-message] Subject prefix `test(watcher):` matches conventional-commits style
- **Plan-spec ref:** N/A (project mandate per CLAUDE.md)
- **Actual state:** Subject `test(watcher): add D1+D3 integration regression test (D4)` — 57 chars (V11), Conventional-Commits-style `<type>(<scope>): …` form, ends with the item tag `(D4)` matching D1/D2/D3 commits.
- **Observation:** Format consistent with project precedent. No Co-Authored-By line in body (verified V2 longer scroll). No "ensures"/"fixes"/"guarantees" definitive language; body uses "Aim: produce a test that fails if either …" framing, project-mandate compliant per CLAUDE.md instructions.
- **Determinism:** deterministic
- **Verification evidence:** V2, V11; full commit-message body read at start of audit.

### #19. [commit-message] Body explains the load-bearing math + Option A choice
- **Plan-spec ref:** N/A
- **Actual state:** Commit body has explicit "Aim:", "Test placement (Option A):", "Test shape (load-bearing math):", "Discriminating assertion:", "Additional invariants asserted:", "No production code touched." sections — closely matching project mandate "detailed, specific, measured, descriptive commit messages that leaves meticulous forensic evidence".
- **Observation:** Body content largely duplicates the in-code doc comment (Observation #2 above) — the explanation lives in two places, which is redundancy rather than scope creep.
- **Determinism:** deterministic
- **Verification evidence:** V2 longer-form output read.

### #20. [documentation] No CLAUDE.md / MEMORY.md / .planning edits in this commit
- **Plan-spec ref:** N/A
- **Actual state:** V1 confirms only `crates/server/src/watcher.rs` modified. No edits to CLAUDE.md, MEMORY.md, plan file, or .planning/ tree.
- **Observation:** Plan does not require D4 documentation updates. STATE.md was updated by the parent agent (per row #194 of STATE.md showing D4 as `[x]` landed in 21ec5b6) — but that's a separate commit and parent-agent bookkeeping, not part of 21ec5b6 itself. Confirmed: D4 implementation commit's working set is exactly `crates/server/src/watcher.rs`.
- **Determinism:** deterministic
- **Verification evidence:** V1, V7.

### #21. [sequence] D4 commits after D1, D2, D3 in linear history
- **Plan-spec ref:** §192–§196 Spawn-order DAG: `[D4] ── depends on D1+D3 (test fixture exercises both)`
- **Actual state:** V3 log: `21ec5b6` ← `8e87e56` (D3-Review checkpoint) ← `2e44080` (D3 impl) ← `bef95a7` (D2-Review) ← `b7e18d1` (D2 impl) ← `636aa9b` (D1-Review) ← `…` ← `6a91894` (D1 impl) ← `99ee138` (B1.1 impl).
- **Observation:** D4 is the final node in the B1.1→D1→D2→D3→D4 chain. STATE.md row #194 confirms D4 spawned after both D1-Review and D3-Review closed. Spawn order honors the DAG dependency.
- **Determinism:** deterministic
- **Verification evidence:** V3, STATE.md:158, :169, :187, :193–194.

### #22. [dependency] D4 imports `claude_history_store::sync::sync_all` (workspace-public)
- **Plan-spec ref:** §244 "D1-D4 | crates/store/src/sync.rs, crates/server/src/serve.rs, crates/server/src/watcher.rs, regression tests"
- **Actual state:** Test calls `claude_history_store::sync::sync_all(&conn, &projects_dir)` and `claude_history_store::db::init_db(&db_path)` at watcher.rs:1018 / 1001. server/Cargo.toml already lists `claude-history-store = { path = "../store" }`.
- **Observation:** No new dev-dependency required. `pub fn sync_all` and `pub async fn init_db` are both already public per existing usage at serve.rs:166, main.rs:845. Cargo.toml unchanged in this commit.
- **Determinism:** deterministic
- **Verification evidence:** V7, server/Cargo.toml read, sync.rs:309 / db public surface.

### #23. [test-coverage] Fixture JSONL realism vs JSONLRecord shape
- **Plan-spec ref:** N/A
- **Actual state:** Phase 1 JSONL line at watcher.rs:1005–1014 is:
  ```
  {"type":"user","uuid":"d4-uuid-{i}","timestamp":"2026-02-20T01:00:00Z","sessionId":"{session}","version":"{version}","cwd":"/tmp","isSidechain":false,"userType":"external","gitBranch":"main","message":{"role":"user","content":"hello"}}
  ```
- **Observation:** Shape matches JSONLRecord::User minimal-required-fields; same fields used by D1's `valid_user_line_with_version` (sync.rs:482). Phase-2 direct `INSERT INTO sessions` uses literal column-list `(session_id, project_path, first_seen_at, version)` matching the sessions table schema. Both fixture forms are valid for their purposes. The cargo-test 3/3 pass result confirms no deserialization failure.
- **Determinism:** deterministic
- **Verification evidence:** V4 hunk + V9 test pass.

### #24. [test-coverage] Daemon-state independence
- **Plan-spec ref:** N/A
- **Actual state:** Test uses PID-scoped temp DB at `std::env::temp_dir()/claude-history-watcher-d4-test/d4-test-{pid}.db`. No reference to `~/.claude/.claude-history.db`. No daemon-state polling.
- **Observation:** Test is fully isolated from production DB and daemon. V12 confirms daemon health intact (record_count 733285 unchanged class).
- **Determinism:** deterministic
- **Verification evidence:** V4 hunk line 999, V12.

### #25. [verification] sqlite3 PRAGMA integrity_check + foreign_key_check on production DB
- **Plan-spec ref:** §332 cross-cutting safeguard
- **Actual state:** V14 returns `ok` and zero foreign-key violation rows.
- **Observation:** Production DB integrity unaffected — D4 didn't touch it (daemon hasn't been kickstarted to a new binary, but the binary itself is unchanged outside the test mod).
- **Determinism:** deterministic
- **Verification evidence:** V14.

### #26. [verification] `claude-history sync` no-op verification per §435 not run
- **Plan-spec ref:** §435 — "After every commit, run `claude-history sync` once with no JSONL changes; expect `files_synced=0, total_records=0`."
- **Observation:** This safeguard belongs to commits that touch sync_metadata semantics. D4 does not modify production code, so sync_metadata semantics cannot regress through this commit. The safeguard is mechanically inapplicable rather than skipped.
- **Determinism:** deterministic
- **Verification evidence:** V7 (no production code changes).

### #27. [meta-plan] Plan §167 LOC estimate vs actual differs by ~5×; plan-text staleness
- **Plan-spec ref:** §167
- **Actual state:** "(~50 LOC)" estimate; actual `+245 LOC`.
- **Observation:** Same kind of LOC-estimate inaccuracy that D1-Audit (15 vs 36+110), D2-Audit (30 vs 30+117), and D3-Audit (5 vs 1+89) flagged for the prior nodes. The plan's estimates persistently undercount because they appear to budget production code only, while in practice each node ships with substantial in-file regression-test scaffolding (which D-tier reviews retroactively accepted as project-pattern compliant). The estimate text in §167 has not been refined to reflect this pattern despite three prior reviews surfacing it.
- **Determinism:** deterministic
- **Verification evidence:** STATE.md:156, :171, :185 (LOC tables in audit summaries).

### #28. [meta-plan] Plan §167 / §244 / §314–§322 / Tier 4 intro paragraph cross-coherence
- **Plan-spec ref:** §163 (Tier 4 intro paragraph), §164–§167 (per-item), §244 (file table), §307–§323 (verification per item)
- **Actual state:**
  - §163 framing: "Cleanup (non-blocking technical debt)"
  - §164: D1 — "regression test ~100 LOC … D4 separately owns the integration test that exercises both D1 and D3 together"
  - §165: D2 — "regression test additive — each D-tier item may include its own focused regression test in the file it modifies, while D4 owns the integration test that exercises D1 and D3 together"
  - §166: D3 — "an in-file regression test in watcher.rs's existing `#[cfg(test)] mod tests` block is permitted"
  - §167: D4 — "Regression test locking in correctness for D1/D3 (~50 LOC)"
  - §244: file table lists "regression tests" generically
  - §320–§322: D4 verification — test-suite gate only
- **Observation:** Internal coherence between §164–§167 about who owns the integration test is good (multiple sections all converge on D4 owning it). §322's terse "the test is the verification" is consistent with §163's "test-only" framing. No contradiction; the §167 LOC budget is the only stale element (item #27 above). Plan §17 line range "`crates/core/src/record.rs:177-524`" is informational and unrelated to D4 (deferred per D-tier prior reviews).
- **Determinism:** deterministic
- **Verification evidence:** Plan §163–§244 read end-to-end.

### #29. [addition] Test instantiates WatcherState with Instant::now() and broadcast::channel manually
- **Plan-spec ref:** N/A
- **Actual state:** watcher.rs:1075–1080 builds WatcherState manually with the four expected fields; constructs `(event_tx, _event_rx) = broadcast::channel::<SseEvent>(16)` to provide the channel.
- **Observation:** Pattern matches D3-test at watcher.rs:927–933 byte-identically (modulo session id strings). No leakage — `_event_rx` is dropped at scope exit.
- **Determinism:** deterministic
- **Verification evidence:** V4 hunk; D3-test cross-read.

### #30. [test-coverage] Untouched-row invariant checked for 2.1.100 / 2.1.101 only
- **Plan-spec ref:** N/A
- **Actual state:** Final loop at watcher.rs:1107–1124 asserts version_history rows for "2.1.100" and "2.1.101" remain at session_count=1.
- **Observation:** Invariant supports the proposition that `check_version_change` only re-derives the row for the version observed on the queried session. The invariant is one-sided — it does not assert there are NO other rows in version_history (e.g., a spurious '' or NULL row). Per #17 above, the COUNT(*)==3 from Phase 1 plus the per-row checks make extra rows hard to introduce, but no Phase-2 post-call COUNT(*) check is performed.
- **Determinism:** deterministic
- **Verification evidence:** V4 hunk last loop.

### #31. [verification] D2 gate code byte-identical pre/post D4
- **Plan-spec ref:** N/A (project-pattern compliance — D4 is constrained to "no production code changes")
- **Actual state:** V7 returns empty diff for `crates/server/src/serve.rs` and the gate region of watcher.rs (the diff shown is exclusively inside `mod tests`, not the `pub async fn watcher_loop` body or its sync_all_done parameter).
- **Observation:** D2's `tokio::sync::oneshot` sender (serve.rs) and receiver (`watcher_loop`'s first action) are byte-identical between `8e87e56` and `21ec5b6`.
- **Determinism:** deterministic
- **Verification evidence:** V7.

### #32. [verification] D3 ON CONFLICT SQL byte-identical pre/post D4
- **Plan-spec ref:** N/A
- **Actual state:** V7 confirms no diff outside watcher.rs's `mod tests` block. The `check_version_change` function (watcher.rs:380–466) is unchanged from the post-D3 state at `2e44080`. ON CONFLICT clause at watcher.rs:436–438: `session_count = (SELECT COUNT(*) FROM sessions WHERE version = ?1)`.
- **Observation:** D3's contribution unchanged.
- **Determinism:** deterministic
- **Verification evidence:** V7; direct read of watcher.rs:380–466.

### #33. [verification] D1 sync_all backfill byte-identical pre/post D4
- **Plan-spec ref:** N/A
- **Actual state:** V7 returns empty diff for `crates/store/src/sync.rs`. D1's INSERT OR IGNORE block at sync.rs:401–414 is unchanged.
- **Observation:** D1's contribution unchanged.
- **Determinism:** deterministic
- **Verification evidence:** V7; sync.rs:383–417 read.

### #34. [test-coverage] No FTS5 rebuild interaction tested
- **Plan-spec ref:** N/A
- **Actual state:** D1's sync_all also triggers an FTS5 index rebuild at the end (per the comment at sync.rs:418–421 "Rebuild FTS5 index if any files were synced"). D4's Phase 1 will exercise that rebuild path with 3 fixture user records. The test does not assert any FTS5 invariant.
- **Observation:** The FTS5 rebuild is a side effect not in scope for D4's stated focus (D1's version_history backfill + D3's session_count re-derive). D1's own sync.rs test similarly does not assert FTS5 state. Pattern consistent.
- **Determinism:** deterministic
- **Verification evidence:** V4 hunk; sync.rs:418–460.

### #35. [test-coverage] Phase-2 `pre_check_vh_count == 1` precondition assertion is load-bearing
- **Plan-spec ref:** N/A
- **Actual state:** watcher.rs:1058–1071 reads version_history.session_count for 2.1.99 BEFORE the check_version_change call and asserts it equals 1.
- **Observation:** This precondition assertion is a key element of the discriminating-math chain: if D1's backfill ever changes its semantics to write `session_count=3` directly (e.g., if D1's COUNT-from-sessions path is inverted), Phase 2's "1+1=2 vs COUNT*=3" math collapses and the test could pass for the wrong reason. The precondition assertion catches that drift early.
- **Determinism:** deterministic
- **Verification evidence:** V4 hunk lines 1058–1071.

### #36. [meta-plan] D4-Review unblock criterion implicit in STATE.md, not in plan
- **Plan-spec ref:** N/A
- **Actual state:** Plan §167 frames D4 as "regression test … (~50 LOC)" without specifying a downstream gate. STATE.md:194 records D4 as the leaf node ("D1-Review AND D3-Review — both now closed").
- **Observation:** D4 is the terminal node of the B1.1→D1→D2→D3→D4 chain; no downstream node depends on D4-Review. Plan §192–§196 confirms `[D4]` has no downstream arrow. Coherent.
- **Determinism:** deterministic
- **Verification evidence:** Plan §192–§196; STATE.md:194.

### #37. [verification] Cross-cutting Live ingestion latency check (§328) not exercised
- **Plan-spec ref:** §328 — fresh user prompt → DB record < 5 s
- **Observation:** This is a daemon-pipeline check; D4 is test-only. Inapplicable per #15. Daemon at PID 2344 is on a pre-99ee138 binary per #16, so even a manual smoke test at this point would not test anything D-tier-specific.
- **Determinism:** deterministic
- **Verification evidence:** V12, V13.

### #38. [verification] No new WARN categories check (§329) not exercised
- **Plan-spec ref:** §329
- **Observation:** Same as #37 — daemon-state check, mechanically inapplicable to a test-only commit until daemon is kickstarted to the new binary. Future kickstart will need to verify err.log post-rotation.
- **Determinism:** deterministic
- **Verification evidence:** V12, V13.

---

## Summary

- **Total observations:** 38
- **Per-category breakdown:**
  - omission: 0
  - divergence: 2 (#1, #6)
  - addition: 8 (#2, #5, #9, #10, #11, #12, #19, #29)
  - sequence: 1 (#21)
  - dependency: 1 (#22)
  - verification: 9 (#15, #16, #25, #26, #31, #32, #33, #37, #38)
  - regression: 0
  - commit-message: 1 (#18)
  - documentation: 1 (#20)
  - test-coverage: 12 (#3, #4, #7, #8, #13, #14, #17, #23, #24, #30, #34, #35)
  - meta-plan: 3 (#27, #28, #36)

Tally: 0+2+8+1+1+9+0+1+1+12+3 = 38.

- **Verification commands:** 14/14 ran successfully (V1–V14, all exit 0). No verification command failed.
- **Headline observation:** D4 is +245 LOC against a plan-stated ~50-LOC budget; the expansion is comment-heavy with extensive in-code rationale duplicating the commit-message body, while the load-bearing `session_count == 3` assertion correctly distinguishes D3's COUNT(*) re-derive from a hypothetical pre-D3 +1 path (no false-pass risk). No production code modified outside the `mod tests` block; D1/D2/D3 production surfaces verified byte-identical via empty diffs.

