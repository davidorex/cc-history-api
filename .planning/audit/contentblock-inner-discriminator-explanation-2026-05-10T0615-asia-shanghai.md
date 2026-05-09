Concrete explanation:

A Claude Code JSONL line for an assistant turn looks like:

```json
{"type": "assistant", "message": {"content": [
  {"type": "text", "text": "Let me help..."},
  {"type": "tool_use", "name": "Bash", "input": {...}},
  {"type": "thinking", "thinking": "..."}
]}, "uuid": "...", ...}
```

Two layers of `type` discriminators:

- **Outer discriminator** — the top-level `"type": "assistant"` selects which `JSONLRecord` variant to deserialize into. Seven known variants: user, assistant, system, summary, attachment (added in C1.1), and pre-B1.1 the fall-through was a hard parse failure. **B1.1 closed this** with `JSONLRecord::Unknown { type_name, raw }` + manual two-pass Deserialize. Records with new top-level types (last-prompt, custom-title, ai-title, permission-mode, agent-name) now land in `record_type_drift_log` instead of being silently dropped.

- **Inner discriminator** — within `message.content`, each element's `"type"` selects which `ContentBlock` variant. Four known: `text`, `thinking`, `tool_use`, `tool_result`. Defined at `crates/core/src/message.rs`:

```rust
pub enum MessageContent {
    Text(String),
    Blocks(Vec<ContentBlock>),
}

#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    Text { text: String },
    Thinking { thinking: String, ... },
    ToolUse { id: String, name: String, input: Value },
    ToolResult { tool_use_id: String, content: ... },
}
```

**The blind spot**: `ContentBlock` uses serde's plain tagged-enum derive. If Claude Code emits a block with `type: "image"` (or any other not-in-the-four discriminator), serde rejects the ContentBlock, which propagates the rejection up:

```
ContentBlock deserialization fails
  → MessageContent::Blocks deserialization fails (vec of failed elements)
    → AssistantRecord/UserRecord deserialization fails
      → JSONLRecord falls through to Unknown via B1.1's catch-all
        → record_type_drift_log gets a row with type_name="assistant"
          → BUT the entire enclosing message is lost from the typed tables
```

Net effect: an `image` content block in one of an assistant message's 5 blocks costs you the *entire message* — the prose text, the other tool calls, the envelope (uuid, parent_uuid, timestamp, model, token usage) — all gone from `messages`/`message_content`/`tool_executions`/`token_usage`. Only the outer discriminator name shows up in drift-log.

**Empirical evidence from this kickstart**: post-resume err.log shows `data did not match any variant of untagged enum MessageContent` for the word-roots project. Some content block in that project's recent JSONL has a discriminator outside the four known. The records aren't being captured at all in typed tables.

**Resolution shape** (mirrors B1.1):
1. Manual two-pass Deserialize on `ContentBlock`: first pass into `serde_json::Value`, peek at `type`, dispatch to known variant OR fall through to a new `ContentBlock::Unknown { block_type: String, raw: Value }` catch-all.
2. Same on `MessageContent` if its untagged shape also fails on element-rejection (which it does — Vec<ContentBlock> deserialize is all-or-nothing under serde derive).
3. Decompose path that walks `MessageContent::Blocks` adds an arm for `ContentBlock::Unknown`: write the parent message normally; record the inner-discriminator drift via `log_record_type_drift` with `record_type = "content_block.<block_type>"` (mirrors C1.2's `"attachment.<subtype>"` namespace pattern).

Estimated ~150 LOC: changes confined to `crates/core/src/message.rs` (the manual impl + Unknown variant) and `crates/store/src/decompose.rs` (the inner-loop arm). No new migration. No new tables. No new CLI/MCP/REST surfacing — the existing `record-type-drift` surface already covers it via the namespace prefix. Tests against real corpus samples that triggered the WARN.

It's the exact same architectural pattern as the JSONLRecord fix at the outer level. The original investigation (`.planning/audit/jsonl-unknown-record-type-attachment-investigation-…`) flagged both levels at the same time as having identical structural shape; the post-MVP roadmap addressed only the outer level on the assumption that the inner level would follow naturally. It hasn't, and the gap is now empirically observable in the live err.log.