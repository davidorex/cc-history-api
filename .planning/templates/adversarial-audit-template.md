# Adversarial audit template

**Authored: 2026-05-08 Asia/Shanghai (UTC+0800)** — based on the same Anthropic doc fetches that ground `subagent-prompt-template.md`. Specifically anchored to the §Code review harnesses passage in *Prompting best practices*, which prescribes the audit-then-triage two-step that this template (audit) and `deviation-triage-template.md` (triage) implement.

This template is for prompts passed to the `Agent` tool's `prompt` parameter when spawning a subagent whose job is to **adversarially audit** a plan's implementation: take a plan section + the implementation commit(s) + the live system state and produce a complete catalog of deviations. The template explicitly instructs the auditor to report every deviation, without filtering by severity, importance, or confidence. Severity assignment is a separate downstream step (the triage template).

The authoritative passage from Anthropic's docs (May 8, 2026):

> Report every issue you find, including ones you are uncertain about or consider low-severity. Do not filter for importance or confidence at this stage - a separate verification step will do that. Your goal here is coverage: it is better to surface a finding that later gets filtered out than to silently drop a real bug. For each finding, include your confidence level and an estimated severity so a downstream filter can rank them.
>
> — *Prompting best practices*, §Code review harnesses

This template extends the parent subagent prompt template's pattern; the parent's 18-item rubric still applies in addition to the audit-specific rubric below.

---

## When to use this template

Spawn an adversarial-audit subagent after **every** implementation commit in the roadmap DAG, before authorizing the next subagent in the chain. Pre-conditions:

- The implementation commit is on the local branch (committed but not yet pushed if a fix may follow)
- The subagent that produced the commit has returned and stopped
- The plan file (`/Users/david/.claude/plans/curious-napping-koala.md` or whichever plan governs the roadmap) is in its current state
- Any audit reports the implementation depended on (`.planning/audit/*.md`) are in their current committed state

Output is a deviation catalog. The catalog is the input to the triage template. Audit and triage are spawned as separate subagents — never combined — because Anthropic's guidance is explicit that filtering is a separate step from finding.

## Subagent type selection

Per *Create custom subagents* §Built-in subagents and the §Code review harnesses guidance:

| Audit shape | `subagent_type` | Tool restriction |
|---|---|---|
| Audit a code commit (Rust changes, migration SQL, test coverage) | `feature-dev:code-reviewer` | Read-only via the agent's bundled tool list |
| Audit a documentation/plan-only commit (markdown changes only) | `general-purpose` with `tools: ["Read", "Grep", "Glob", "Bash"]` | Read-only via tool restriction |
| Audit a multi-commit implementation chain (B1.1 + B1.2 together, for example) | `feature-dev:code-reviewer` with explicit instruction to read the full commit range | Read-only |

In all cases the audit subagent is **read-only**. It does not modify the working tree, does not stage/commit/push, and does not run destructive operations. Per Anthropic's §Code review harnesses passage, the auditor's only job is coverage of finding; the user (or a downstream subagent invocation) decides what to do about the findings.

---

## --- TEMPLATE BODY START ---

```
You are an adversarial auditor reviewing the implementation of {{PLAN_SECTION_NAME}} (commit {{COMMIT_SHA_OR_RANGE}}) of the cc-history-api roadmap. Your goal is **coverage of finding** — surface every deviation between what the plan specified and what was implemented, including deviations you are uncertain about or consider trivial. A separate downstream subagent (deviation-triage) handles categorization and the user assigns severity. Your job is to find, not to filter.

<!--
Principle: "Give Claude a role" + Anthropic §Code review harnesses (May 2026)
Source: Prompting best practices §Code review harnesses
Quote: "Report every issue you find, including ones you are uncertain about or consider low-severity. Do not filter for importance or confidence at this stage."
The role is "adversarial auditor" — explicitly stating coverage as the goal counters Opus 4.7's tendency to self-filter findings below an inferred bar.
-->

<context>
The plan file at {{ABSOLUTE_PATH_TO_PLAN_FILE}} contains the specification for {{PLAN_SECTION_NAME}}. The implementation in commit {{COMMIT_SHA_OR_RANGE}} claims to satisfy that specification. Audit reports the plan depends on (read these to understand the plan's "why"):

- {{ABSOLUTE_PATH_TO_AUDIT_DOC_1}} — {{ONE_LINE_RELEVANCE}}
- {{ABSOLUTE_PATH_TO_AUDIT_DOC_2}} — {{RELEVANCE}}

Why this audit matters: this commit is the prerequisite for {{NEXT_DAG_NODE}}. If deviations exist that haven't been surfaced, they propagate forward into dependent work. The audit-then-triage pattern is explicitly recommended by Anthropic's guidance for code-review harnesses; this template implements its first step (find every deviation).

Operational state of the live system at audit time:
- {{LIVE_SYSTEM_FACT_1_E_G_DAEMON_PID_AND_HEALTH}}
- {{LIVE_SYSTEM_FACT_2}}
- {{LIVE_SYSTEM_FACT_3}}

Read those operational facts and verify them yourself before relying on them.
</context>

<inputs>
Required reading, in order:

1. {{ABSOLUTE_PATH_TO_PLAN_FILE}} — read the section identified as {{PLAN_SECTION_NAME}} end-to-end. Treat the plan as the spec.
2. The implementation diff: `git show --stat {{COMMIT_SHA_OR_RANGE}}` followed by `git show {{COMMIT_SHA_OR_RANGE}}` for full hunks.
3. Each file the commit modified, in its current state, end-to-end.
4. Each file the plan said would be modified, in its current state, end-to-end. (May overlap with #3, but the plan may have specified files the commit didn't touch — that itself is a deviation.)
5. The relevant audit doc(s) under `.planning/audit/` for context on why the plan said what it said.

Reference-only:
- The parent subagent prompt template at `.planning/templates/subagent-prompt-template.md` for the structural pattern this template extends.
- The triage template at `.planning/templates/deviation-triage-template.md` for the format your output will feed into.
</inputs>

<task>
Adversarially audit the implementation against the plan and the live system. Surface every deviation, regardless of size or your confidence about it. Do not filter, rank, or recommend.

Sequential steps (execute in order):

1. **Build a requirements ledger.** Read the plan section end-to-end. Extract every requirement the plan states — file changes, table additions, column additions, function signatures, test cases, verification commands, output specifications, behavioral assertions, sequencing claims, scope exclusions ("this task is NOT…"). Number each requirement (R1, R2, …). Include the exact plan-file line range for each.

2. **Build an implementation ledger.** Run `git show {{COMMIT_SHA_OR_RANGE}}` and read the full diff. For each file in the diff, list every code change as I1, I2, …. Include the file:line range for each. Also note files in the diff that the plan did NOT mention.

3. **Cross-reference R-list against I-list.** For every Rn, find the corresponding Im(s). Record the mapping (Rn → I*) or note "no implementation found" if absent. For every Im, find the corresponding Rn. Record (Im → R*) or note "scope-creep candidate" if no plan requirement justifies it.

4. **Run the plan's verification commands.** The plan section's `<verification_commands>` block (or equivalent) lists commands the implementation must satisfy. Run each one against the post-commit live system. Record actual output and compare to expected output. Any divergence is a deviation, regardless of how minor.

5. **Run the parent plan's "Specific assurances against existing-functionality regression" steps that apply to this commit.** This includes: pre/post regression baselines if captured (`/tmp/regress.*.before` files), `PRAGMA integrity_check`, `PRAGMA foreign_key_check`, `claude-history sync` returning 0 records on no-change, etc. Cite the parent plan's safeguard step number for each check.

6. **Audit the commit message.** Project mandates require detailed forensic commit messages without unjustified definitives ("ensures", "fixes") and without Co-Authored-By credit lines. Compare the commit message against those requirements. Cite specific lines that violate.

7. **Audit for sequence and dependency violations.** Verify the commit is at the expected position in the spawn-order DAG. Verify all dependencies the commit references are already merged (no references to not-yet-committed migration numbers, struct variants, table columns, etc.). Verify the commit does not include changes that belong in a later DAG node.

8. **Audit for documentation gaps.** The plan may require updates to `CLAUDE.md`, `README.md`, the project's auto-memory at `~/.claude/projects/-Users-david-Projects-cc-history-api/memory/MEMORY.md`, or `.planning/audit/` doc addenda. Verify each required documentation surface was updated.

9. **Audit test coverage.** For every code change, verify a test exists. The bar is **coverage of finding**: missing tests are deviations even if the existing test suite passes. Note tests that exist but are weak (assert the call doesn't panic but not the actual semantic outcome).

10. **Compile a deviation catalog.** Format every finding as a structured row with the fields specified in `<output_format>` below. Include findings you are uncertain about. Include findings you consider trivial. Do not omit anything.
</task>

<!--
Principle: "Provide instructions as sequential steps" + Anthropic §Code review harnesses (coverage over filtering)
Source: Prompting best practices §Be clear and direct
Quote: "Provide instructions as sequential steps using numbered lists or bullet points when the order or completeness of steps matters."
Step 1 (requirements ledger) and Step 2 (implementation ledger) are the load-bearing parts: without those two ledgers, cross-referencing in step 3 is impressionistic. Each step explicitly forbids filtering at the find stage.
-->

<constraints>
- Do not modify any file. Read-only audit; the working tree must be unchanged when you finish.
- Do not stage, commit, push, revert, reset, or kill any process. The audit is observation only.
- Do not run destructive commands. `claude-history sync` and `cargo test` and `sqlite3 ... PRAGMA integrity_check` are safe; anything that mutates state is forbidden.
- Do not invoke another subagent. The audit is a single-subagent task.
- Do not assign severity to findings. Do not use words like "critical", "high", "medium", "low", "minor", "nit", "important", "blocking", "non-blocking" in your output. The downstream triage subagent handles categorization; the user assigns severity.
- Do not recommend fixes or remediation. Phrases like "should be addressed", "consider", "could be left", "easy fix" are forbidden. State what you found and where; stop there.
- Do not omit findings because they seem out of scope. Per project mandate-007, a finding the plan didn't specify is itself a finding (scope-creep candidate). Surface it.
- Do not omit findings because the implementation chose a defensible-looking alternative. State the deviation; the triage downstream and the user evaluate whether the alternative is acceptable.
- If the audit reveals an issue with the plan itself (the plan was internally inconsistent, or the plan's verification commands cannot be run), report that as a meta-deviation in a separate section. Do not paper over plan defects to make the implementation look clean.
- If a step fails for an unexpected reason (a verification command errors out for a reason unrelated to the implementation, the live system is not in the expected state, etc.), STOP at that step. Report the obstacle. Do not skip the remaining steps.
</constraints>

<!--
Principle: "Tell Claude what to do, not what not to do" — but hard prohibitions are exceptions
Source: Prompting best practices §Output and formatting
The hard prohibitions on severity language and remediation language are stated negatively because positive phrasing ("only state findings") is weaker than the explicit forbidden-word list. Anthropic's §Code review harnesses passage is explicit that filtering is a separate step.
Mandate-008 (project mandate): the audit subagent stops on obstacles and reports rather than working around them.
-->

<output_format>
Your final response must be a structured deviation catalog with these sections in this order:

1. **Audit metadata** (≤ 50 words): commit SHA(s) audited, plan section name, plan file path, audit timestamp.

2. **Requirements ledger summary** (≤ 100 words): how many requirements you extracted from the plan; the highest-numbered Rn; any plan sections you found ambiguous or non-extractable (cite file:line). The full ledger is too long to inline; it lives in your scratch state during the audit.

3. **Deviation catalog**: a markdown table with one row per deviation. Columns:
   | # | Plan ref (Rn or "—") | Impl ref (Im or "—") | Plan section file:line | Implementation file:line | Category | Description | Evidence |

   Categories (use exactly these labels — the triage template depends on them):
   - `omission` — plan specified Rn, implementation does not contain it
   - `divergence` — plan specified Rn one way, implementation does it differently
   - `addition` — implementation contains Im, plan does not specify it (scope-creep candidate)
   - `sequence` — commit is out of expected DAG position, or contains work that belongs in a later node
   - `dependency` — implementation references a not-yet-merged element
   - `verification` — plan-required verification command did not produce expected output
   - `regression` — pre/post regression baseline diff (or schema integrity check) shows existing functionality affected
   - `commit-message` — commit message violates project mandate format
   - `documentation` — plan-required documentation surface was not updated
   - `test-coverage` — code change without corresponding test, or test exists but is weak
   - `meta-plan` — the plan itself contains internal inconsistency, ambiguity, or non-runnable verification

   Description: one factual sentence stating what was specified vs. what was found. No severity language, no remediation suggestion.

   Evidence: a bash one-liner, sql query, file:line reference, or diff hunk that proves the finding. Every row must have evidence. A finding without evidence is not a finding.

4. **Verification command results** (full output): for each plan-specified verification command, the command run, exit status, full output. Include even successful commands so the user can see what was checked.

5. **Plan-defect notes** (only if applicable): meta-deviations where the plan itself was internally inconsistent or non-actionable. Cite plan-file:line.

6. **Audit completeness self-check**: confirm you executed all 10 steps in the task list. For each step, one sentence on what you found or "no findings".

Do not include preamble, conclusions, recommendations, summaries-of-summary, or commentary on the implementation's overall quality. Start directly with the Audit metadata section.
</output_format>

<!--
Principle: "Be specific about the desired output format" + Anthropic §Code review harnesses (per-finding fields enable downstream filtering)
Source: Prompting best practices §Output and formatting + §Code review harnesses
Quote: "For each finding, include your confidence level and an estimated severity so a downstream filter can rank them."
This template OMITS confidence and severity columns from the deviation catalog because the project mandate is stricter: severity assignment is the user's, not even the auditor's estimation. The triage template handles category-grouping; the user handles severity. Anthropic's guidance permits the downstream filter to receive severity estimates; the project mandate is more conservative.
The category labels are an exact match to the triage template's expected input format. Changing the labels in this template requires synchronizing with deviation-triage-template.md.
-->

<verification_commands>
Before declaring completion, run these commands and include output in section 4 of your response:

```bash
git show --stat {{COMMIT_SHA_OR_RANGE}}
git log -n 1 --format=fuller {{COMMIT_SHA_OR_RANGE}}
sqlite3 ~/.claude/.claude-history.db "PRAGMA integrity_check; PRAGMA foreign_key_check;"
{{PLAN_SECTION_VERIFICATION_COMMAND_1}}
{{PLAN_SECTION_VERIFICATION_COMMAND_2}}
{{PLAN_SECTION_VERIFICATION_COMMAND_3}}
```

These are the minimum. Add additional commands from the plan's `<verification_commands>` block as appropriate.
</verification_commands>

<scope>
This task is exactly: produce a deviation catalog for {{COMMIT_SHA_OR_RANGE}} against {{PLAN_SECTION_NAME}}.

This task is NOT:
- Triaging or grouping the deviations (the deviation-triage subagent handles that as a separate invocation).
- Assigning severity, priority, or "blocker" labels to deviations.
- Recommending fixes, workarounds, or alternative approaches.
- Authorizing or blocking the next subagent in the DAG. The user makes that decision after reading your catalog and the triage output.
- Modifying the implementation to address findings.
- Updating the plan to match the implementation.

If you find yourself doing anything in the "is NOT" list, STOP. Each item belongs in a different invocation owned by the user.
</scope>
```

## --- TEMPLATE BODY END ---

---

## Audit-specific evaluation rubric

In addition to the parent subagent prompt template's 18-item rubric (sections 1–18), an adversarial-audit prompt must satisfy these audit-specific checks. Each cites its Anthropic source.

| # | Check | Anthropic source | How to verify |
|---|---|---|---|
| A1 | Role explicitly states "adversarial auditor" and "coverage of finding" | §Code review harnesses | First sentence of prompt contains both phrases |
| A2 | Anthropic's coverage-over-filtering quote is reproduced or paraphrased in the role/context | §Code review harnesses | "Report every issue" or equivalent appears in prompt |
| A3 | Constraints forbid severity language with explicit forbidden-word list | §Be clear and direct (precision) + project mandate | `<constraints>` lists at minimum: critical, high, medium, low, minor, nit, important, blocking |
| A4 | Constraints forbid remediation/recommendation language | §Code review harnesses | `<constraints>` lists at minimum: should be addressed, consider, could be left, easy fix |
| A5 | Output format requires evidence column on every finding row | §Be clear and direct + project mandate-004 | `<output_format>` description column says "every row must have evidence" |
| A6 | Output format uses category labels matching the triage template's expected input | Cross-template consistency | Labels match `omission, divergence, addition, sequence, dependency, verification, regression, commit-message, documentation, test-coverage, meta-plan` exactly |
| A7 | Task includes ledger-building before cross-referencing | §Be clear and direct (precision) + Anthropic Opus 4.7 literal-instruction-following | Steps 1 and 2 build R-ledger and I-ledger before step 3 cross-references |
| A8 | Verification commands include `PRAGMA integrity_check` and the plan's own verification block | §Ask Claude to self-check + project parent-plan safeguards | `<verification_commands>` includes both |
| A9 | Constraints include meta-deviation reporting (plan defects) | Project mandate-007 + audit completeness | `<constraints>` instructs subagent to report plan defects rather than paper over them |
| A10 | Scope explicitly excludes triage/severity/remediation | §Prompting Claude Opus 4.7 (literal instruction following) | `<scope>` lists triage, severity assignment, recommendation as out of scope |

A filled audit prompt must pass all A1–A10 in addition to the parent template's 1–18.

## Worked example

Filled instance for hypothetical audit of commit B1.1 (variant catch-all migration + parser/record changes), commit SHA `XXXXXXX` (placeholder; substitute real SHA at audit time):

```
You are an adversarial auditor reviewing the implementation of B1.1 (variant-level catch-all: migration 007 + JSONLRecord::Unknown variant + manual Deserialize impl) at commit XXXXXXX of the cc-history-api roadmap. Your goal is coverage of finding — surface every deviation between what the plan specified and what was implemented, including deviations you are uncertain about or consider trivial. A separate downstream subagent (deviation-triage) handles categorization and the user assigns severity. Your job is to find, not to filter.

<context>
The plan file at /Users/david/.claude/plans/curious-napping-koala.md contains the specification for the B1 section (lines 60-78 approximately, plus the verification block in the "Verification per item" section §B1, and the regression safeguards in the "Regression and continuity safeguards" section §16-§21). The implementation in commit XXXXXXX claims to satisfy B1.1 specifically (the first of B1's two sub-commits). Audit reports the plan depends on:

- /Users/david/Projects/cc-history-api/.planning/audit/jsonl-unknown-record-type-attachment-investigation-2026-05-08T0551-asia-shanghai.md — full investigation of the variant-level architectural blind spot, including Path A's resolution mechanics that B1.1 implements

Why this audit matters: B1.1 is the prerequisite for B1.2, which is the prerequisite for both C1.1 and C2.1. A deviation in B1.1 propagates into the entire Tier 2 → Tier 3 chain.

Operational state of the live system at audit time:
- Daemon supervised by launchd, currently PID 49130 (verifiable via `launchctl list | grep claude-history`).
- DB at ~/.claude/.claude-history.db, integrity verified ok via `PRAGMA integrity_check` pre-commit.
- Pre-commit DB backup at /tmp/claude-history.db.bak-XXXXXXX exists and is integrity-clean.

Verify those operational facts yourself before relying on them.
</context>

<inputs>
Required reading, in order:

1. /Users/david/.claude/plans/curious-napping-koala.md — read the B1 section, the §B1 entry in "Verification per item", and §16-§21 in "Regression and continuity safeguards".
2. The implementation diff: `git show --stat XXXXXXX` then `git show XXXXXXX` for full hunks.
3. Each file the commit modified, in its current state: crates/core/src/record.rs, crates/store/migrations/007_record_type_drift.sql, crates/store/src/schema.rs, plus any test modules touched.
4. Files the plan said would be modified: same list as #3 per the B1.1 plan section. Cross-check.
5. /Users/david/Projects/cc-history-api/.planning/audit/jsonl-unknown-record-type-attachment-investigation-2026-05-08T0551-asia-shanghai.md — Path A specs.

Reference-only:
- /Users/david/Projects/cc-history-api/.planning/templates/subagent-prompt-template.md — parent template structural pattern.
- /Users/david/Projects/cc-history-api/.planning/templates/deviation-triage-template.md — your output format will feed this.
</inputs>

<task>
Adversarially audit the B1.1 implementation against the plan and the live system. Execute the 10 steps in the template body's task block, substituting B1.1-specific specifics:

- Requirements ledger (step 1): the plan's B1.1 sub-commit lists exactly: (a) JSONLRecord::Unknown { type_name, raw } variant, (b) manual Deserialize impl with two-pass dispatch, (c) migration 007_record_type_drift.sql creating record_type_drift_log table mirroring schema_drift_log shape, (d) registration of migration 007 in crates/store/src/schema.rs MIGRATIONS array, (e) tests covering known-variant deserialization unchanged + unknown-variant capture + idempotent re-observation. Extract each as Rn with the exact plan file:line.
- Implementation ledger (step 2): every hunk in `git show XXXXXXX`.
- Cross-reference (step 3): R1..Rn → Im, and Im → R*; flag any orphaned Im (scope creep) and any unmapped Rn (omission).
- Verification commands (step 4): `cargo build --release`, `cargo test -p claude-history-core`, `cargo test -p claude-history-store`, `git diff --stat`, sqlite3 dry-run of migration 007 against /tmp/test_migration.db. Run each, capture full output, compare to plan's expected.
- Regression safeguards (step 5): `PRAGMA integrity_check`, `PRAGMA foreign_key_check`, `claude-history sync` returning 0 records on no-change, `claude-history version-check` showing all known versions still present, the seven analytical views still queryable.
- Commit message audit (step 6): cite specific lines violating the no-unjustified-definitives, no-Co-Authored-By, forensic-detail mandates.
- Sequence/dependency audit (step 7): verify B1.1 lands before B1.2 (B1.2 should not be in this commit); verify no references to not-yet-merged Migration 008 (C1.1) or 009 (C2.1) elements.
- Documentation audit (step 8): plan does not require CLAUDE.md or MEMORY.md updates for B1.1 specifically (those are for B1.2 and later). Verify the plan's silence is honored — no unauthorized doc edits.
- Test coverage audit (step 9): every new code path in record.rs and schema.rs has an associated test; existing tests at crates/core/src/record.rs:177-524 still pass byte-identically.
- Compile (step 10): produce the deviation catalog in the output format.
</task>

<constraints>
[Same as template body — no modifications to working tree, no severity language, no remediation suggestions, no triage/grouping. STOP and report on obstacles. Surface meta-plan defects separately.]
</constraints>

<output_format>
[Same as template body — Audit metadata, Requirements ledger summary, Deviation catalog table with category labels matching the triage template, Verification command results, Plan-defect notes if any, Audit completeness self-check.]
</output_format>

<verification_commands>
```bash
git show --stat XXXXXXX
git log -n 1 --format=fuller XXXXXXX
sqlite3 ~/.claude/.claude-history.db "PRAGMA integrity_check; PRAGMA foreign_key_check;"
cargo build --release 2>&1 | tail -10
cargo test -p claude-history-core 2>&1 | tail -30
cargo test -p claude-history-store 2>&1 | tail -30
sqlite3 /tmp/test_migration_audit.db < crates/store/migrations/007_record_type_drift.sql && echo "schema apply OK"
claude-history sync 2>&1 | grep -E 'files_synced|total_records'
claude-history version-check 2>&1 | tail -5
```
</verification_commands>

<scope>
[Same as template body — adversarial audit of B1.1 only, not triage, not severity assignment, not remediation, not authorization for next DAG node.]
</scope>
```

## Notes on the audit-then-triage two-step

Anthropic's §Code review harnesses passage is the exact pattern this template implements:

> Report every issue you find, including ones you are uncertain about or consider low-severity. Do not filter for importance or confidence at this stage - a separate verification step will do that. Your goal here is coverage: it is better to surface a finding that later gets filtered out than to silently drop a real bug.

The audit template is the "find" stage. The triage template is the "filter / categorize" stage. The user (or a third subagent invocation if scope changes are warranted) is the "rank / decide" stage. Combining find and filter into one subagent invocation produces the regression Anthropic describes for Opus 4.7: "the model doing the same depth of investigation but converting fewer investigations into reported findings."

## Doc-version reconciliation

When Anthropic publishes new prompt-engineering or code-review guidance:

1. Re-fetch `https://platform.claude.com/docs/en/build-with-claude/prompt-engineering/claude-prompting-best-practices` (specifically §Code review harnesses) and `https://code.claude.com/docs/en/sub-agents`.
2. Diff the §Code review harnesses passage against the role statement and the constraints in this template. The "report every issue" / "coverage over filtering" framing is the load-bearing principle; if Anthropic's framing changes, update the role statement and the A1/A2 rubric items.
3. Update the "Authored:" marker.
4. Old audit prompts in git history are auditable against the rubric in effect when they were authored.
