# Subagent prompt template

**Authored: 2026-05-08 Asia/Shanghai (UTC+0800)** — based on Anthropic's *Prompting best practices* (`https://platform.claude.com/docs/en/build-with-claude/prompt-engineering/claude-prompting-best-practices`) and *Create custom subagents* (`https://code.claude.com/docs/en/sub-agents`) as published on May 8, 2026. Docs cover Claude Opus 4.7 / Opus 4.6 / Sonnet 4.6 / Haiku 4.5.

This template is for prompts passed to the `Agent` tool's `prompt` parameter when spawning a subagent from this project's main session. It is NOT for authoring custom subagent definitions (those live in `~/.claude/agents/` or `.claude/agents/` and use YAML frontmatter for `name`, `description`, `tools`, etc. — see the *Create custom subagents* doc above for that surface).

The template is structured to be evaluable against the canonical Anthropic guidance: each section names the prompting principle it implements and quotes or paraphrases the relevant doc passage. An auditor reading this template against the docs at any later date can verify each section still matches current best practice.

---

## How to use this template

1. Copy the body below the `--- TEMPLATE BODY START ---` marker into the `prompt` parameter of an `Agent` tool call.
2. Fill in every `{{PLACEHOLDER}}` slot. Do not leave placeholders unsubstituted — placeholders that survive into the final prompt are ambiguous instructions and the subagent will hallucinate or guess.
3. Delete sections that don't apply to the specific subagent task (e.g., delete the `<examples>` block if no in-prompt examples are warranted; delete `<verification_commands>` if the subagent is read-only research).
4. Set `Agent` tool parameters explicitly: `description` (3–5 word task name), `subagent_type` (matching the kind of work — see selection guide below), `isolation: "worktree"` when the subagent will write code that must not collide with the parent's working tree.
5. Before invoking, evaluate the filled prompt against the rubric at the bottom of this template.

### Pre-spawn batching analysis (mandatory before grouping subagents in parallel)

Before invoking multiple `Agent` tool calls in a single message for parallel execution, enumerate the files each candidate subagent will touch and verify pairwise compile-time independence. **DAG-independence (the plan's claim that no commit must precede another) is necessary but not sufficient for safe parallel execution.**

A parallel batch is safe iff, for every pair (X, Y) of subagents in the batch, X's intermediate working-tree state cannot break Y's `cargo build --release` or `cargo test` verification. Compile-time coupling that surfaces during X's mid-run state will cause Y to stop per mandate-008, losing Y's work.

**Procedure**:

1. For each candidate subagent, enumerate the files it will modify (read its prompt's `<task>` and `<scope>` sections; cross-reference against the plan's files-to-modify table).
2. For each pair, check: does either subagent modify a file in `crates/core/src/` (load-bearing types) or any crate that the other subagent's verification compiles against? If yes, the pair is compile-time coupled.
3. For any compile-time-coupled pair, **do not parallelize**. Either:
   - **Serialize**: spawn one, wait for its commit, then spawn the next. The plan's spawn-order DAG already specifies sequencing for hard logical dependencies; this rule extends sequencing to compile-time dependencies the DAG doesn't surface.
   - **Worktree isolation**: pass `isolation: "worktree"` to each `Agent` call so each subagent operates on its own git worktree. Intermediate states cannot collide. Cost: more disk + slower clone; appropriate for genuinely-independent-DAG-but-compile-coupled batches.
4. If no compile-coupling for any pair, parallelization is safe. Spawn in a single message with multiple `Agent` calls.

**Reference incident (2026-05-09)**: batch was {B1.1, D1, D2, D3}, all DAG-independent per the plan. B1.1 modified `JSONLRecord` enum in `crates/core/src/record.rs`, which `crates/server/src/{serve,watcher}.rs` (D2's and D3's working set) transitively compile against via the workspace dependency graph. When D2 and D3 ran their `cargo build` verification mid-batch, they observed B1.1's intermediate state where the new `Unknown` variant was added to the enum but not yet matched in `crates/store/src/drift.rs::log_record_overflow`. They stopped per mandate-008. WIP discarded; tasks reverted to pending; re-spawn required. Cost of recovery exceeded what pre-spawn analysis would have cost.

**Rule of thumb for this project**:
- B-tier and C-tier items modify load-bearing core types — never parallel with anything in server or store crates that compiles against those types.
- Multiple D-tier items: check pairwise file-touched sets; D2 and D3 both modify `crates/server/src/watcher.rs` and cannot be batched even with each other.
- A-tier items (A1: outside-repo files; A2: build-output and tracked manifest only) safely parallelize with each other but check before mixing with non-A-tier.
- Tier-4 items that touch genuinely independent files (e.g., D1 modifies only `crates/store/src/sync.rs`) can parallelize with one B/C item only if that item doesn't compile-couple to D1's surface.

## Subagent type selection guide

Per *Create custom subagents* §Built-in subagents (May 8, 2026):

| Task shape | `subagent_type` | Tool restriction | Model |
|---|---|---|---|
| Read-only codebase exploration, single targeted lookup | `Explore` (specify thoroughness: `quick` / `medium` / `very thorough`) | Read-only enforced | Haiku (fast) |
| Implementation that touches files, runs tests, executes commands | `general-purpose` | All tools | Inherits |
| Research that must inform a written plan, no implementation | `feature-dev:code-explorer` | Read-only | Inherits |
| Architecture / design validation before implementation | `feature-dev:code-architect` | Read-only | Inherits |
| Code review of an already-implemented change | `feature-dev:code-reviewer` | Read-only | Inherits |
| Plan-mode design work | `Plan` (auto-invoked from plan mode) | Read-only | Inherits |

Default to `general-purpose` if the task involves both research and writing/editing. Use `Explore` only when the work is genuinely read-only and the user wants a fast Haiku-driven response.

---

## --- TEMPLATE BODY START ---

```
You are {{ROLE_NOUN_PHRASE}} working on the {{REPO_NAME}} project at {{REPO_ABSOLUTE_PATH}}. {{ONE_SENTENCE_TASK_PURPOSE}}.

<!--
Principle: "Give Claude a role" + "Be clear and direct"
Source: Prompting best practices §General principles (May 2026)
Quote: "Setting a role in the system prompt focuses Claude's behavior and tone for your use case. Even a single sentence makes a difference."
Quote: "Think of Claude as a brilliant but new employee who lacks context on your norms and workflows. The more precisely you explain what you want, the better the result."
The first paragraph above is the role + one-sentence purpose statement. Keep it terse.
-->

<context>
{{CONTEXT_NEEDED_BY_SUBAGENT_THAT_IS_NOT_DERIVABLE_FROM_THE_CODEBASE_ALONE}}

Specifically:
- {{LOAD_BEARING_FACT_1_WITH_FILE_PATH_OR_LINE_NUMBER}}
- {{LOAD_BEARING_FACT_2}}
- {{LOAD_BEARING_FACT_3_INCLUDING_WHY_IT_MATTERS}}

Why this matters for your work: {{ONE_PARAGRAPH_EXPLAINING_THE_CONNECTION_BETWEEN_CONTEXT_AND_TASK}}.
</context>

<!--
Principle: "Add context to improve performance" — explain WHY behind instructions
Source: Prompting best practices §General principles
Quote: "Providing context or motivation behind your instructions, such as explaining to Claude why such behavior is important, can help Claude better understand your goals and deliver more targeted responses. Claude is smart enough to generalize from the explanation."
Constraint: load-bearing facts must be specific (file paths, line numbers, function names, exact counts). Vague context produces vague work.
-->

<inputs>
The following resources are required reading before you start. Read them in order:

1. {{ABSOLUTE_PATH_TO_AUDIT_DOC_OR_PLAN_SECTION}} — {{ONE_LINE_DESCRIPTION_OF_WHY}}
2. {{ABSOLUTE_PATH_TO_RELATED_FILE_2}} — {{REASON}}
3. {{ABSOLUTE_PATH_TO_RELATED_FILE_3}} — {{REASON}}

The following are reference-only (consult as needed during work, not required reading):
- {{REFERENCE_PATH_1}} — {{WHY_IT_MIGHT_BE_USEFUL}}
- {{REFERENCE_PATH_2}} — {{WHY}}
</inputs>

<task>
{{ONE_SENTENCE_TASK_STATEMENT_USING_AN_IMPERATIVE_VERB}}.

Sequential steps (execute in order):

1. {{STEP_1_CONCRETE_VERB_AND_OBJECT}}
2. {{STEP_2}}
3. {{STEP_3}}
4. {{STEP_4_INCLUDING_INTERMEDIATE_VERIFICATION}}
5. {{STEP_5_FINAL_DELIVERABLE_PRODUCTION}}

If you discover that a step is unnecessary because the desired state already holds, skip it and note that in your final report. Do not skip steps because they're inconvenient.
</task>

<!--
Principle: "Be specific about the desired output format and constraints" + "Provide instructions as sequential steps using numbered lists or bullet points when the order or completeness of steps matters"
Source: Prompting best practices §General principles
Numbered lists are mandatory when ordering matters. Bullets when it doesn't. Mixing them confuses the model.
-->

<constraints>
- {{HARD_CONSTRAINT_1_INCLUDING_FORBIDDEN_OPERATIONS_LIKE_RM_OR_FORCE_PUSH}}
- Do not modify files outside {{SCOPE_DIRECTORY_OR_FILE_GLOB}}.
- Do not run destructive operations ({{LIST_SPECIFIC_FORBIDDEN_COMMANDS_FOR_THIS_TASK}}).
- Do not commit. The parent agent reviews your diff before any commit is authorized.
- {{IF_RELEVANT_DO_NOT_SPAWN_FURTHER_SUBAGENTS_NOTE}}.
- If you encounter ambiguity that this prompt does not resolve, STOP and report what you found rather than guessing or fabricating.
- If a step fails for an unexpected reason (compilation error, test failure, missing file), STOP at that point. Report the failure and what you observed. Do not attempt workarounds without authorization.
</constraints>

<!--
Principle: "Tell Claude what to do, not what not to do" — phrase positively where possible
Source: Prompting best practices §Output and formatting
However, hard prohibitions are exceptions: destructive operations and authorization boundaries warrant explicit "do not" framing because positive phrasing ("preserve all files outside this scope") is weaker than the prohibition.
Mandate-008 (project mandate): "If a subagent returns an issue, STOP. Report to user." — encoded above as the "STOP and report" instruction on ambiguity and failure.
-->

<output_format>
Your final response must contain:

1. **Summary** (≤ {{N}} words): one paragraph describing what you did, what you found, and what state the working tree / DB / system is in now.
2. **Diff overview**: list every file you created or modified with a one-line description of the change.
3. **Verification results**: for each step in `<verification_commands>` below, the command run, exit status, and a one-line interpretation.
4. **Open issues** (if any): things you discovered that were out of scope but warrant the parent's attention. Per project mandate-007, do not omit discovered issues even if they're tangential.
5. **Continuation guidance** (if applicable): the next subagent in the spawn-order DAG and what input it needs.

Do not include preamble like "I'll now analyze..." or "Based on my work...". Start directly with the Summary.
</output_format>

<!--
Principle: "Be specific about the desired output format" + "Tell Claude what to do instead of what not to do" + "Eliminating preambles"
Source: Prompting best practices §Output and formatting + §Migrating away from prefilled responses
Quote: "Use direct instructions in the system prompt: 'Respond directly without preamble. Do not start with phrases like Here is..., Based on..., etc.'"
Word counts ('≤ N words') must be specific. Vague constraints like 'concise' are weaker.
-->

<verification_commands>
Before declaring completion, run these commands and include results in your output:

```bash
{{COMMAND_1_TO_PROVE_WORK_DID_WHAT_WAS_INTENDED}}
{{COMMAND_2_TO_CHECK_NO_REGRESSION}}
{{COMMAND_3_TO_VALIDATE_BUILD_OR_TEST_SUITE_PASSES}}
{{COMMAND_4_TO_CONFIRM_DB_OR_OPERATIONAL_STATE}}
```

Expected results:
- Command 1: {{EXPECTED_OUTPUT_OR_RANGE}}
- Command 2: {{EXPECTED}}
- Command 3: {{EXPECTED}}
- Command 4: {{EXPECTED}}

If any command produces output diverging materially from expected, STOP and report the divergence rather than continuing.
</verification_commands>

<!--
Principle: "Ask Claude to self-check"
Source: Prompting best practices §Thinking and reasoning
Quote: "Append something like 'Before you finish, verify your answer against [test criteria].' This catches errors reliably, especially for coding and math."
Verification commands close the loop: the subagent's claim that work is done is grounded in observable command output rather than the model's confidence.
-->

<examples>
<!--
Principle: "Use examples effectively" — multishot/few-shot prompting
Source: Prompting best practices §General principles
Quote: "Examples are one of the most reliable ways to steer Claude's output format, tone, and structure. A few well-crafted examples (known as few-shot or multishot prompting) can dramatically improve accuracy and consistency."
Quote: "Include 3–5 examples for best results."
Include examples ONLY when:
  (a) the desired output format is non-obvious from the structural prompt above, OR
  (b) prior similar work exists and would prevent the subagent from re-deriving conventions.
For the cc-history-api roadmap, prior work includes the three audit reports under .planning/audit/ — pointing at those is usually sufficient and a few-shot block is unnecessary. Delete this <examples> block if not needed.
-->

<example index="1">
<input>{{INPUT_FOR_EXAMPLE_1}}</input>
<output>{{EXPECTED_OUTPUT_OR_BEHAVIOR_FOR_EXAMPLE_1}}</output>
</example>

<example index="2">
<input>{{INPUT_FOR_EXAMPLE_2}}</input>
<output>{{EXPECTED_OUTPUT_OR_BEHAVIOR_FOR_EXAMPLE_2}}</output>
</example>
</examples>

<scope>
This task is exactly: {{ONE_SENTENCE_RESTATEMENT_OF_TASK}}.

This task is NOT:
- {{COMMONLY_CONFUSED_ADJACENT_TASK_1_AND_WHY_NOT_NOW}}
- {{COMMONLY_CONFUSED_ADJACENT_TASK_2}}
- Any work that touches {{OUT_OF_SCOPE_AREA}}, even if it appears related.

If you find yourself doing something that doesn't match the task statement above, STOP. The boundary is intentional — adjacent work belongs in a separate subagent invocation per the spawn-order DAG.
</scope>

<!--
Principle: "More literal instruction following" + scope-specification
Source: Prompting best practices §Prompting Claude Opus 4.7
Quote: "Claude Opus 4.7 interprets prompts more literally and explicitly than Claude Opus 4.6, particularly at lower effort levels. It will not silently generalize an instruction from one item to another, and it will not infer requests you didn't make. […] If you need Claude to apply an instruction broadly, state the scope explicitly"
The corollary applies in reverse: when scope must be NARROW, state explicitly what is NOT in scope. Opus 4.7 will respect both inclusions and exclusions when stated.
-->
```

## --- TEMPLATE BODY END ---

---

## Evaluation rubric (run before invoking the Agent tool)

For each filled-in copy of this template, verify the following before invoking. Each item maps to a specific Anthropic best-practice citation and can be re-evaluated against future doc updates.

| # | Check | Anthropic source | How to verify |
|---|---|---|---|
| 1 | Role statement is one sentence and specific to the task | *Best practices* §Give Claude a role | First sentence of prompt names the role and the purpose |
| 2 | All `{{PLACEHOLDER}}` slots substituted | *Best practices* §Be clear and direct | `grep -F '{{' <filled-prompt>` returns nothing |
| 3 | Context section explains WHY, not just WHAT | *Best practices* §Add context to improve performance | `<context>` block contains the word "because" / "so that" / "Why this matters" |
| 4 | Inputs section names absolute paths, not relative | *Best practices* §Be clear and direct (precision) | All paths in `<inputs>` start with `/Users/david/...` or are explicitly described as repo-relative |
| 5 | Task uses numbered steps when order matters | *Best practices* §Be clear and direct | `<task>` block uses `1.` `2.` `3.` numbered list |
| 6 | Constraints include destructive-action prohibition | *Best practices* §Balancing autonomy and safety + project mandate | `<constraints>` lists `rm -rf`, force-push, etc. as forbidden |
| 7 | Constraints include STOP-on-ambiguity instruction | Project mandate-008 | `<constraints>` contains "STOP and report" |
| 8 | Output format specifies word count or length cap | *Best practices* §Output and formatting | `<output_format>` includes a numeric `≤ N words` or equivalent |
| 9 | Output format forbids preamble | *Best practices* §Migrating away from prefilled responses | `<output_format>` contains "Do not include preamble" |
| 10 | Verification commands are concrete and have expected results | *Best practices* §Ask Claude to self-check | `<verification_commands>` block contains executable commands AND expected results |
| 11 | Scope section states what task is NOT | *Best practices* §Prompting Claude Opus 4.7 (literal instruction following) | `<scope>` block contains "This task is NOT:" with at least one negative example |
| 12 | XML tags are descriptive and consistent | *Best practices* §Structure prompts with XML tags | Tags use names like `<context>`, `<task>`, `<constraints>`, `<output_format>` — not generic `<info>` or numeric `<section1>` |
| 13 | Examples section deleted when not needed | *Best practices* §Use examples effectively (3–5 examples for best results, but only if needed) | If `<examples>` is present, it has 1–3 examples relevant to actual task; if irrelevant, removed entirely |
| 14 | No "do not" framing except for hard prohibitions | *Best practices* §Output and formatting (positive phrasing) | Most instructions phrase positively; "do not" reserved for destructive operations and authorization boundaries |
| 15 | No vague qualifiers (`important`, `try to`, `consider`) standing alone | *Best practices* §Be clear and direct | Search for `important|try to|please consider` in the prompt — each instance must be paired with a concrete operationalization |
| 16 | Subagent type matches task shape | *Create custom subagents* §Built-in subagents | Match against the selection guide table above |
| 17 | `isolation: worktree` set when subagent writes files that should not collide with parent's working tree | `Agent` tool docs (parent doc) | Filled prompt is paired with explicit `isolation: "worktree"` when applicable |
| 18 | Tool restrictions match the task | *Create custom subagents* §Available tools | Read-only tasks specify `Explore` or use `disallowedTools`; write tasks use `general-purpose` |

A filled prompt that fails any rubric check should be revised before invocation. Failing checks #1, #2, #5, #7, #8, #11 produces measurable degradation in subagent output quality per Anthropic guidance.

## Worked example

The following is a filled instance of this template for a hypothetical B1.1 invocation (variant-level catch-all migration + parser/record changes), shown to demonstrate the slot-filling pattern. **This is illustrative only — do not copy verbatim; produce a fresh fill per actual subagent task.**

```
You are a Rust systems engineer working on the cc-history-api project at /Users/david/Projects/cc-history-api. You will implement migration 007 plus the JSONLRecord::Unknown variant and its manual Deserialize impl, completing commit B1.1 of the variant-level catch-all roadmap.

<context>
This project ingests Claude Code JSONL session files into a SQLite database. The JSONLRecord enum at crates/core/src/record.rs:23-46 is `#[serde(tag = "type")]` with seven known variants. When Claude Code emits a record whose `type` is not one of those seven, serde rejects the entire object, the parser at crates/core/src/parser.rs:125-156 logs a warning, and the record is silently lost.

Specifically:
- Six unknown discriminators have been observed corpus-wide (~13.5K records dropped to date): attachment (10,085), last-prompt (1,396), custom-title (884), permission-mode (868), agent-name (296), ai-title (22). The investigation report at .planning/audit/jsonl-unknown-record-type-attachment-investigation-2026-05-08T0551-asia-shanghai.md documents this.
- The schema_drift_log mechanism preserves field-level evolution but cannot capture record-type-level evolution — there is no current path for unknown variants to land in the DB.
- The codebase has no precedent for unknown-discriminator handling; this is the first such pattern.

Why this matters for your work: B1.1 establishes the structural floor that all subsequent ingestion-touching work depends on. The C1 (typed AttachmentRecord) and C2 (planContent promotion) tracks both rely on B1.1's pattern landing first.
</context>

<inputs>
1. /Users/david/Projects/cc-history-api/.planning/audit/jsonl-unknown-record-type-attachment-investigation-2026-05-08T0551-asia-shanghai.md — full investigation including resolution Path A specs
2. /Users/david/.claude/plans/curious-napping-koala.md — read the B1 section for exact requirements
3. /Users/david/Projects/cc-history-api/crates/core/src/record.rs — current JSONLRecord definition you will extend
4. /Users/david/Projects/cc-history-api/crates/store/migrations/006_version_monitoring.sql — pattern for migrations including IF NOT EXISTS, INSERT OR IGNORE, idempotent backfill
5. /Users/david/Projects/cc-history-api/crates/core/src/record.rs:177-524 — existing test suite that is the regression net for the seven known variants

Reference-only:
- /Users/david/Projects/cc-history-api/crates/store/src/drift.rs — the existing field-level drift logging pattern your work parallels
- /Users/david/Projects/cc-history-api/crates/store/src/schema.rs — migration runner, MIGRATIONS array
</inputs>

<task>
Implement commit B1.1: add JSONLRecord::Unknown { type_name, raw } variant via manual Deserialize impl + create migration 007 record_type_drift_log table.

Sequential steps:

1. Read all five required inputs end-to-end. Verify the audit report's Path A description matches the plan's B1 section.
2. Implement the manual Deserialize impl on JSONLRecord. Two-pass approach: deserialize into serde_json::Value first, dispatch to typed variant by `type` field via from_value, fall back to Unknown if type is not one of the seven.
3. Add JSONLRecord::Unknown { type_name: String, raw: serde_json::Value } variant with Debug, Clone, Serialize, Deserialize derive (Deserialize handled by manual impl above).
4. Write migration 007_record_type_drift.sql: CREATE TABLE record_type_drift_log mirroring schema_drift_log shape — type_name, version, sample_value, source_context, occurrence_count, first_seen_at, last_seen_at, UNIQUE(type_name, version). Idempotent (IF NOT EXISTS). Register in crates/store/src/schema.rs MIGRATIONS array.
5. Add tests in crates/core/src/record.rs covering: (a) all seven existing variants continue to deserialize byte-identically against fixtures already in the suite, (b) unknown variant captures full discriminator + raw payload from a synthetic line with type "fictitious-test-type", (c) round-trip serialize/deserialize preserves Unknown content.
6. Run `cargo build --release` and `cargo test -p claude-history-core` and `cargo test -p claude-history-store` — all green.
</task>

<constraints>
- Do not modify files outside crates/core/src/record.rs, crates/store/migrations/, crates/store/src/schema.rs, and the corresponding test modules.
- Do not run `git commit`, `git push`, `git reset --hard`, `rm -rf`, or any destructive operation. The parent agent reviews your diff before any commit is authorized.
- Do not modify the existing seven JSONLRecord variants' Deserialize behavior. They must produce identical structs as before.
- Do not implement the B1.2 drift-logging extension or CLI surfacing — that is a separate commit and a separate subagent invocation.
- If you encounter ambiguity in the audit doc or the plan that this prompt does not resolve, STOP and report what you found rather than guessing.
- If `cargo test` fails for an unexpected reason, STOP at that point. Report the failure and what you observed.
</constraints>

<output_format>
Your final response must contain:

1. **Summary** (≤ 100 words): what you did, current state of the working tree, what's ready for review.
2. **Diff overview**: every file created or modified with a one-line description.
3. **Verification results**: each command from <verification_commands> below with exit status and one-line interpretation.
4. **Open issues** (if any): discovered issues outside the B1.1 scope.
5. **Continuation guidance**: confirmation that B1.2 is the next subagent in the DAG and that B1.1's commit is its prerequisite.

Do not include preamble. Start directly with the Summary.
</output_format>

<verification_commands>
```bash
cargo build --release 2>&1 | tail -10
cargo test -p claude-history-core 2>&1 | tail -20
cargo test -p claude-history-store 2>&1 | tail -20
git diff --stat
sqlite3 /tmp/test_migration.db < crates/store/migrations/007_record_type_drift.sql && echo OK
```

Expected results:
- cargo build: "Finished `release` profile" with no errors
- cargo test core: all tests pass, includes new Unknown-variant tests
- cargo test store: all tests pass, includes migration 007 schema test
- git diff --stat: only crates/core/src/record.rs, crates/store/migrations/007_record_type_drift.sql, crates/store/src/schema.rs, and any new test fixtures
- sqlite3 dry-run: `OK` printed
</verification_commands>

<scope>
This task is exactly: implement commit B1.1 (JSONLRecord::Unknown variant + manual Deserialize impl + migration 007) per the plan and audit report.

This task is NOT:
- B1.2 (drift logging extension, CLI surfacing, REST endpoint, bytewise re-ingestion backfill) — separate subagent.
- C1 (typed AttachmentRecord) — must wait for B1.2 to land.
- Any work that touches the watcher (crates/server/src/watcher.rs), serve infrastructure (crates/server/src/serve.rs), or analytical views.

If you find yourself doing something that doesn't match "implement B1.1", STOP. The boundary is intentional — adjacent work belongs in separate subagent invocations per the spawn-order DAG.
</scope>
```

## Notes for the parent agent (me)

- The template is for one-shot subagent invocations from this session via the `Agent` tool. The `prompt` parameter receives the filled body between the BEGIN/END markers; the `Agent` tool's other parameters (`description`, `subagent_type`, `isolation`, `model`) are set separately per the parameter docs.
- The rubric at the top is the evaluation gate. A filled prompt that fails any rubric check is revised before invocation. The user can audit any past filled prompt by running it through the same rubric.
- This template carries a date marker (`Authored: 2026-05-08`) and explicit Anthropic doc citations so a future audit can verify each guideline still matches Anthropic's guidance. If Anthropic's docs change, the template's rubric needs to be re-grounded against the new versions.
- Worked example uses the cc-history-api B1.1 task to demonstrate slot-filling. Real B1.1 invocation produces its own fill per the same template; the worked example is reference, not template.

---

## Doc-version reconciliation

When the Anthropic prompt-engineering docs are updated:

1. Re-fetch `https://platform.claude.com/docs/en/build-with-claude/prompt-engineering/claude-prompting-best-practices` and `https://code.claude.com/docs/en/sub-agents`.
2. Diff against this template's principle citations.
3. For each new or modified principle, decide whether it warrants a template section, a rubric item, or both.
4. Update this template's "Authored:" marker to the reconciliation date.
5. The prior template content is preserved in git history; old subagent prompts can still be audited against the version of the rubric in effect when they were authored.
