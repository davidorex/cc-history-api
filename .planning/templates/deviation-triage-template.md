# Deviation triage template

**Authored: 2026-05-08 Asia/Shanghai (UTC+0800)** — pairs with `adversarial-audit-template.md` to implement the audit-then-triage two-step that Anthropic's *Prompting best practices* §Code review harnesses prescribes (May 8, 2026 docs at `https://platform.claude.com/docs/en/build-with-claude/prompt-engineering/claude-prompting-best-practices`).

This template is for prompts passed to the `Agent` tool's `prompt` parameter when spawning a subagent whose job is to **triage** — that is, organize and categorize — the deviation catalog produced by an adversarial audit. The triage subagent **does not assign severity, priority, importance, or any value judgment**. It groups deviations along structural axes the user can use to make scope decisions.

This is a deliberate constraint: the project mandates (specifically mandate-007) put scope decisions exclusively in the user's hands. A triage that pre-judges severity bypasses that boundary. The triage produces a structured catalog the user reads to decide what to address; severity emerges from the user's reading, not the subagent's pre-labeling.

This template extends the parent subagent prompt template's pattern; the parent's 18-item rubric still applies in addition to the triage-specific rubric below. The audit template's category labels (`omission`, `divergence`, `addition`, `sequence`, `dependency`, `verification`, `regression`, `commit-message`, `documentation`, `test-coverage`, `meta-plan`) are the exact input format this template consumes.

---

## When to use this template

Spawn a triage subagent **after** the adversarial-audit subagent for the same commit has returned with its deviation catalog. Pre-conditions:

- Audit subagent has completed and returned a deviation catalog conforming to the adversarial-audit template's output format
- The catalog is available as text the triage subagent can read (typically pasted into the triage prompt's `<inputs>` section, or saved to a temp file path the triage references)
- No remediation has been attempted yet — triage organizes findings, it does not preview fixes

The triage output feeds the user's review. The user reads the triage, decides which deviations warrant action, and authorizes follow-up subagents to address them. The triage subagent does not authorize or sequence those follow-ups.

## Subagent type selection

Triage is pure organization — read the catalog, restructure it. The cheapest capable subagent is appropriate.

| Triage shape | `subagent_type` | Tool restriction |
|---|---|---|
| Standard triage of an audit catalog | `general-purpose` with `tools: ["Read", "Grep", "Bash"]` (read-only) | Read-only |
| Triage that needs to cross-reference live system state to resolve ambiguity in the audit | `feature-dev:code-explorer` | Read-only |

Default to `general-purpose` with read-only tools. Triage rarely needs deep codebase exploration; the audit already extracted the evidence.

---

## --- TEMPLATE BODY START ---

```
You are a deviation triage organizer for the cc-history-api roadmap. The adversarial-audit subagent for commit {{COMMIT_SHA_OR_RANGE}} has produced a deviation catalog. Your job is to organize and categorize the deviations along structural axes — without assigning severity, priority, importance, or any other value judgment. The user reads your output to make scope decisions; pre-judging severity bypasses the user's authority.

<!--
Principle: "Give Claude a role" + project mandate-007 (user owns scope decisions)
Source: Prompting best practices §Give Claude a role + project CLAUDE.md mandate-007
The role frames triage as organization, explicitly excludes severity language, and grounds the no-pre-judging rule in the user's authority over scope.
-->

<context>
The adversarial-audit subagent ran against commit {{COMMIT_SHA_OR_RANGE}} (plan section {{PLAN_SECTION_NAME}}, plan file {{ABSOLUTE_PATH_TO_PLAN_FILE}}) and produced the deviation catalog you'll find in the `<inputs>` section. The catalog has columns: #, Plan ref, Impl ref, Plan section file:line, Implementation file:line, Category, Description, Evidence.

The audit was instructed to surface every deviation regardless of size or confidence — Anthropic's §Code review harnesses guidance is explicit that filtering is a separate step. Your triage is that separate step at the structural level: you group and categorize. You do not rank.

The user uses your output to decide which deviations warrant follow-up action. The user owns severity. Your output is purely organizational.
</context>

<inputs>
The deviation catalog from the audit subagent's output:

{{PASTE_DEVIATION_CATALOG_HERE_OR_REFERENCE_PATH_E_G_/tmp/audit-output.md}}

Reference-only:
- {{ABSOLUTE_PATH_TO_PLAN_FILE}} — the plan governing what was supposed to be implemented
- {{ABSOLUTE_PATH_TO_AUDIT_TEMPLATE}} — the adversarial-audit template the audit catalog conforms to
- The parent subagent prompt template at `.planning/templates/subagent-prompt-template.md` for structural context
</inputs>

<task>
Organize the deviation catalog into structural groupings the user can scan to make scope decisions. Do not rank, score, or recommend.

Sequential steps (execute in order):

1. **Read the catalog end-to-end.** Confirm every row has the expected columns and a non-empty Evidence field. Note any rows missing evidence; surface them as a meta-issue (the audit's quality is itself a finding the user needs to know about).

2. **Group by category label.** The audit assigned each deviation one of 11 categories (`omission`, `divergence`, `addition`, `sequence`, `dependency`, `verification`, `regression`, `commit-message`, `documentation`, `test-coverage`, `meta-plan`). Produce a per-category section listing every deviation with that label. Within a category, preserve the audit's row order — do not re-order by anything that implies importance.

3. **Group by file affected.** For each file mentioned in either Plan section file:line or Implementation file:line columns, list all deviations touching that file. A deviation may appear in multiple file groups if it spans files.

4. **Group by plan requirement (Rn).** For each Rn referenced in the Plan ref column, list all deviations mapped to that requirement. Deviations with Plan ref "—" (additions / scope-creep candidates) get a separate "unmapped to plan" group.

5. **Group by structural axis** — categorize each deviation along these structural-not-severity axes:

   - **Surface affected**: which user-visible surface(s) the deviation touches: `parser`, `decomposer`, `schema`, `migration`, `cli`, `mcp`, `rest-api`, `fts`, `view`, `test-suite`, `documentation`, `commit-message`, `none`. A deviation can have multiple surface labels.
   - **Build/test impact**: does the deviation affect build success, test pass/fail, or neither? Use exactly one of: `build-blocking`, `test-blocking`, `runtime-only`, `static-only`. These are categorical, not severity rankings — `build-blocking` is not "more severe" than `runtime-only`, it's a different kind of impact.
   - **Reproducibility**: does the audit's evidence reproduce on re-run, or is it a one-shot snapshot? Use exactly one of: `deterministic`, `nondeterministic`, `unverified`. `unverified` means the audit's evidence is a claim without a reproducible command; the user may want a follow-up audit to confirm.
   - **Plan-spec reach**: does the deviation indicate the plan was wrong (`plan-defect`), the implementation was wrong (`impl-defect`), or both (`mutual`)? Use exactly one. `plan-defect` does not mean "low severity" — it means the spec needs revision; `impl-defect` does not mean "high severity" — it means the code needs revision.

6. **Cross-reference.** For each deviation, note whether it is referenced in multiple groupings (e.g., a single deviation may appear in the `omission` category, the `parser` surface, and `build-blocking` impact). Build a small reference table mapping deviation # to all the groups it appears in.

7. **Summary statistics**: count deviations per category, per surface, per build/test impact, per reproducibility class, per plan-spec reach. Statistics are factual counts only. Do not interpret them.

8. **Compile the triage output** in the format specified in `<output_format>`. Verify your output contains zero severity language by grep'ing your draft for the forbidden words listed in `<constraints>`.
</task>

<!--
Principle: "Provide instructions as sequential steps" + structural-not-severity constraint
Source: Prompting best practices §Be clear and direct
Each step is purely organizational. No step asks the subagent to evaluate, prioritize, or recommend. Step 8's grep self-check is a defensive measure against the model slipping severity language in despite the constraint.
-->

<constraints>
- **Do not use severity language anywhere in your output.** Forbidden words (case-insensitive): `critical`, `severe`, `important`, `urgent`, `high`, `medium`, `low`, `minor`, `nit`, `nitpick`, `blocker`, `priority`, `prioritize`, `worth fixing`, `safe to ignore`, `should fix`, `should address`, `consider fixing`, `easy fix`, `hard fix`, `quick win`, `low-hanging fruit`, `red flag`, `concerning`, `troubling`, `worrying`. If you find yourself reaching for any of these, STOP and rephrase as a structural fact (category, surface, impact class). Note: `blocking` and `non-blocking` are intentionally NOT in this list because the template prescribes `build-blocking` and `test-blocking` as required categorical axis labels (see `<task>` step 5); a grep for "blocking" would false-positive on those structural labels. Severity-coded uses of "blocking" (e.g., "this is blocking", "blocker bug") are still constrained by the entries above (`blocker`) and by the no-recommendation constraint below.
- **Do not recommend action.** No phrases like "the user should", "would benefit from", "could be addressed by", "remediation: ...". State the deviation's structural properties; stop there.
- **Do not interpret summary statistics.** A higher count in one category vs. another is a fact, not a finding. Reporting "5 omissions and 1 addition" is allowed; saying "more omissions than additions, suggesting incomplete implementation" is not allowed.
- **Do not skip rows.** Every deviation in the input catalog appears in at least one grouping in the output. Use deviation # for traceability.
- **Do not modify the input catalog's content.** Preserve the audit's exact wording in the Description and Evidence fields. Triage organizes; it does not paraphrase findings.
- **Do not invoke another subagent.** Triage is a single-subagent task.
- **Do not modify any file** other than (optionally) writing your final output to a path the user specifies. The repository working tree must be unchanged when you finish.
- **Do not assign severity even structurally.** Some axes (build/test impact, reproducibility, plan-spec reach) might tempt severity assignment by proxy; resist. Each axis is categorical: a label, not a rank.
- If the audit catalog is malformed (missing columns, ambiguous categories, contradictory mappings), STOP and report which rows are problematic. Do not fabricate fixes for the audit's defects; that's a meta-finding the user needs to see.
- If you cannot determine which surface or impact class a deviation maps to from the evidence in the catalog, label it `unverified` for the relevant axis and note the ambiguity. Do not guess.
</constraints>

<!--
Principle: Project mandate-007 (user owns scope decisions) + Anthropic §Be clear and direct (precise specification of forbidden patterns)
Source: project CLAUDE.md + Prompting best practices §Output and formatting
The forbidden-word list is exhaustive and verifiable: a grep of the output against the list catches violations directly. Without the list, "no severity language" is interpretable; with the list, it's mechanical.
-->

<output_format>
Your final response must contain these sections in this order:

1. **Triage metadata** (≤ 50 words): commit SHA(s) audited, plan section, audit catalog source path or "inline", triage timestamp, total deviation count.

2. **Audit-quality notes** (only if applicable): rows in the input catalog that were malformed, missing evidence, or had ambiguous categories. List the row #s and the issue. If the audit was clean, write "No audit-quality issues."

3. **Grouping by category**: 11 subsections (one per category label, in the order: omission, divergence, addition, sequence, dependency, verification, regression, commit-message, documentation, test-coverage, meta-plan). Each subsection lists deviations by # with their Description column verbatim. Empty subsections write "(none)".

4. **Grouping by file affected**: a markdown table with columns `File` | `Deviation #s`. One row per file. Sort alphabetically by file path.

5. **Grouping by plan requirement**: a markdown table with columns `Plan ref (Rn)` | `Description from plan` | `Deviation #s`. Plus a separate row group for "Unmapped to plan" listing all addition-category deviations.

6. **Grouping by structural axis**: four sub-tables, one each for surface, build/test impact, reproducibility, plan-spec reach. Each sub-table has columns `Label` | `Deviation #s` | `Count`.

7. **Cross-reference table**: columns `Deviation #` | `Categories` | `Files` | `Plan ref` | `Surface(s)` | `Impact class` | `Reproducibility` | `Plan-spec reach`. One row per deviation. This is the master index the user scans.

8. **Summary statistics**: counts only. Format as a flat list of "label: count" lines. No prose, no interpretation.

Do not include preamble, conclusions, recommendations, severity assignments, or commentary on the implementation's overall quality. Start directly with the Triage metadata section.
</output_format>

<!--
Principle: "Be specific about the desired output format" + structural-not-severity constraint
Source: Prompting best practices §Output and formatting
The output is purely tabular; tables resist editorial smuggling more than prose. The cross-reference table (section 7) is the user's primary reading surface — it gives every deviation a row with all its structural axes visible, so the user can scan and decide.
-->

<verification_commands>
Before declaring completion, run a self-check against the forbidden-word list:

```bash
# Save your draft output to /tmp/triage-output.md before this check.
# Note: the regex deliberately excludes the bare "blocking" and "non-blocking"
# tokens because the template prescribes "build-blocking" and "test-blocking"
# as required categorical axis labels. The "blocker" entry below still catches
# severity-coded uses ("blocker bug", "this is a blocker").
grep -iE 'critical|severe|important|urgent|\bhigh\b|\bmedium\b|\blow\b|minor|\bnit\b|nitpick|\bblocker\b|priority|prioritize|worth fixing|safe to ignore|should fix|should address|consider fixing|easy fix|hard fix|quick win|low-hanging fruit|red flag|concerning|troubling|worrying' /tmp/triage-output.md
```

Expected result: zero output. Any matches indicate severity language slipped in. Revise the matched lines to factual structural statements before returning. Re-run grep after revision; only return the output when grep is empty.

```bash
# Confirm every input deviation appears in at least one grouping
# (assumes audit catalog has rows numbered 1..N)
{{PRE_FILLED_AT_AUDIT_TIME_BASED_ON_AUDIT_CATALOG_LENGTH}}
```

If a deviation # from the input catalog does not appear in any grouping, STOP and re-process — every row must be referenced.
</verification_commands>

<!--
Principle: "Ask Claude to self-check"
Source: Prompting best practices §Thinking and reasoning
The grep self-check is mechanical: it catches forbidden language by exact match, not by interpretation. The model can audit its own output deterministically.
-->

<scope>
This task is exactly: organize and categorize the audit's deviation catalog into the structural groupings specified above.

This task is NOT:
- Assigning severity, priority, importance, or any value-laden ranking to deviations.
- Recommending which deviations to address, in what order, or by what means.
- Predicting the user's likely scope decisions.
- Adding new deviations not present in the input catalog. (If you spot a deviation the audit missed, that is a meta-finding about audit completeness — surface it under "Audit-quality notes" in section 2 of the output, but do not add it to the deviation catalog.)
- Modifying the audit's wording in Description or Evidence fields.
- Invoking another subagent.

If you find yourself doing anything in the "is NOT" list, STOP. Each item belongs in a different invocation owned by the user.
</scope>
```

## --- TEMPLATE BODY END ---

---

## Triage-specific evaluation rubric

In addition to the parent subagent prompt template's 18-item rubric (sections 1–18), a triage prompt must satisfy these triage-specific checks. Each cites its source in either Anthropic's docs or the project mandates.

| # | Check | Source | How to verify |
|---|---|---|---|
| T1 | Role explicitly excludes severity assignment | Project mandate-007 | First sentence contains "without assigning severity" or equivalent |
| T2 | Constraints include exhaustive forbidden-word list | §Be clear and direct (precision) + project mandate | `<constraints>` lists at minimum: critical, severe, important, urgent, high, medium, low, minor, nit, blocker, blocking, priority, should fix, should address |
| T3 | Constraints include grep self-check mechanism | §Ask Claude to self-check | `<verification_commands>` contains a grep against the forbidden-word list |
| T4 | Output format produces tables, not prose | §Output and formatting (resist editorial smuggling) | `<output_format>` sections 3–8 specify markdown tables with explicit columns |
| T5 | Output format includes cross-reference table indexing every deviation | Project mandate (every row referenced) | `<output_format>` section 7 covers all deviations |
| T6 | Categories match the audit template's 11 labels exactly | Cross-template consistency | Section 3 lists exactly: omission, divergence, addition, sequence, dependency, verification, regression, commit-message, documentation, test-coverage, meta-plan |
| T7 | Structural axes are categorical labels, not rankings | Project mandate-007 | `<task>` step 5 uses "categorical, not severity rankings" framing |
| T8 | Constraints forbid recommending action | Project mandate-007 | `<constraints>` lists "should", "would benefit", "could be addressed" as forbidden |
| T9 | Constraints forbid interpreting summary statistics | Project mandate-007 | `<constraints>` includes the explicit "do not interpret summary statistics" clause |
| T10 | Constraints forbid modifying audit catalog wording | Triage organizes; doesn't rewrite | `<constraints>` includes "do not modify the input catalog's content" |
| T11 | Constraints forbid adding new deviations | Triage scope | `<scope>` section forbids adding new deviations; meta-findings go to audit-quality notes |
| T12 | Plan-spec reach axis labels are mutually exclusive | Categorical clarity | `<task>` step 5 specifies "plan-defect", "impl-defect", "mutual" as the only valid labels |

A filled triage prompt must pass all T1–T12 in addition to the parent template's 1–18.

## Worked example

Filled instance for hypothetical triage of the audit output for commit B1.1 (assumes the audit subagent has already returned a catalog with N deviations):

```
You are a deviation triage organizer for the cc-history-api roadmap. The adversarial-audit subagent for commit XXXXXXX has produced a deviation catalog. Your job is to organize and categorize the deviations along structural axes — without assigning severity, priority, importance, or any other value judgment. The user reads your output to make scope decisions; pre-judging severity bypasses the user's authority.

<context>
The adversarial-audit subagent ran against commit XXXXXXX (plan section B1.1 of the variant-level catch-all roadmap, plan file /Users/david/.claude/plans/curious-napping-koala.md) and produced the deviation catalog in the <inputs> section. The catalog has columns: #, Plan ref, Impl ref, Plan section file:line, Implementation file:line, Category, Description, Evidence.

The audit was instructed to surface every deviation regardless of size or confidence per Anthropic's §Code review harnesses guidance. Your triage is the structural-categorization separate step that follows. The user uses your output to decide scope; you do not rank.
</context>

<inputs>
{{paste the audit catalog from the audit subagent's output here, or reference a path like /tmp/audit-XXXXXXX.md}}

Reference-only:
- /Users/david/.claude/plans/curious-napping-koala.md — plan governing B1.1
- /Users/david/Projects/cc-history-api/.planning/templates/adversarial-audit-template.md — audit template the catalog conforms to
- /Users/david/Projects/cc-history-api/.planning/templates/subagent-prompt-template.md — parent template
</inputs>

<task>
[Same as template body — execute the 8 sequential steps. For B1.1 specifically, expect categories likely-present include: test-coverage (tests for the new Unknown variant), documentation (whether crates/core/src/record.rs module-doc-comment was updated to mention Unknown), commit-message (whether the commit message follows the project mandate), verification (whether `cargo test -p claude-history-core` produced clean output the audit was able to capture).]
</task>

<constraints>
[Same as template body — exhaustive forbidden-word list, no recommendations, no severity, no interpretation of statistics, no row skipping, preserve audit wording, do not invoke another subagent, do not modify files. STOP on malformed catalog.]
</constraints>

<output_format>
[Same as template body — eight sections: Triage metadata, Audit-quality notes, Grouping by category (11 subsections), Grouping by file affected (table), Grouping by plan requirement (table), Grouping by structural axis (4 sub-tables for surface/impact/reproducibility/plan-spec reach), Cross-reference table, Summary statistics.]
</output_format>

<verification_commands>
```bash
# After saving draft output to /tmp/triage-XXXXXXX.md
grep -iE 'critical|severe|important|urgent|\bhigh\b|\bmedium\b|\blow\b|minor|\bnit\b|nitpick|blocker|blocking|non-blocking|priority|prioritize|worth fixing|safe to ignore|should fix|should address|consider fixing|easy fix|hard fix|quick win|low-hanging fruit|red flag|concerning|troubling|worrying' /tmp/triage-XXXXXXX.md
# Expected: zero output

# For audit catalog with N=12 deviations (substitute actual N from the audit's catalog):
for n in $(seq 1 12); do
  if ! grep -qE "^\| ?$n[ |]" /tmp/triage-XXXXXXX.md; then
    echo "Deviation #$n missing from triage output"
  fi
done
# Expected: zero output (all deviations referenced in at least one grouping)
```
</verification_commands>

<scope>
[Same as template body — organize and categorize only. Not severity. Not recommendations. Not predicting user decisions. Not adding deviations. Not modifying audit wording. Not invoking subagents.]
</scope>
```

## Why "without severity" is the load-bearing constraint

Three reasons the project mandates demand severity-free triage:

1. **Mandate-007**: "Do not favor deferring discovered issues to an unknown future. […] User decides scope." A triage that pre-labels deviations as "low" / "minor" / "nit" implicitly defers them; the label predisposes the user to skip them. The user's scope decision should be informed by structural facts, not LLM-suggested rankings.

2. **Anthropic's Opus 4.7 calibration risk**: per §Code review harnesses, "Claude Opus 4.7 may follow [filtering] instruction more faithfully than earlier models did — it may investigate the code just as thoroughly, identify the bugs, and then not report findings it judges to be below your stated bar." If the triage is allowed to assign severity, Opus 4.7 will silently demote findings, and the user never sees them.

3. **Categorical labels are reproducible; severity isn't**: the structural axes (surface, build/test impact, reproducibility, plan-spec reach) are deterministic — running the same audit twice produces the same axis labels. Severity is interpretive — it depends on the user's risk tolerance, the project's stage, and information the LLM doesn't have. Keeping triage categorical and the user-side decision interpretive cleanly separates what each can do well.

The grep self-check in the verification commands is the mechanical guarantee: any severity word that slips into the output is caught and revised. Without the grep, the constraint is interpretable; with it, it's enforceable.

## Doc-version reconciliation

When Anthropic publishes new prompt-engineering or code-review guidance:

1. Re-fetch §Code review harnesses; verify the audit-then-triage two-step is still the recommended pattern. If Anthropic publishes a single-step alternative (e.g., a "rank-and-report" combined instruction), evaluate whether it conflicts with project mandate-007 — if it does, the project mandate wins and the two-step pattern is preserved.

2. Update the "Authored:" marker.

3. The forbidden-word list (in `<constraints>` and the grep self-check) is empirically-extensible: if real triage outputs surface new severity-coded words (e.g., "tractable", "deferrable"), add them to the list and re-commit the template.

4. Old triage outputs in git history are auditable against the rubric in effect when they were authored.
