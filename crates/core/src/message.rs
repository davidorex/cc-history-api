//! Content block models and message types.
//!
//! This module handles the polymorphic content structures found in Claude Code
//! JSONL records:
//!
//! - [`MessageContent`]: Untagged enum handling the dual representation of user
//!   message content (plain string ~15% vs block array ~85%)
//! - [`ContentBlock`]: Tagged enum for the 4 content block types (text, thinking,
//!   tool_use, tool_result)
//! - [`AssistantMessage`]: The inner message object from assistant records
//! - [`UsageStats`]: Token usage statistics with overflow for evolving billing fields

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// User message content — can be either a plain string or an array of content blocks.
///
/// Empirical data: 15% of user messages use plain string content,
/// 85% use block arrays (mostly tool_result blocks).
/// The `serde(untagged)` attribute tries each variant in order.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MessageContent {
    Text(String),
    Blocks(Vec<ContentBlock>),
}

/// Content block types found in both user and assistant messages.
///
/// Assistant messages contain: text, thinking, tool_use
/// User messages contain: tool_result, text
///
/// Note: No overflow on enum variants — serde(tag) + serde(flatten) on
/// enum variants can cause deserialization issues. Polymorphic fields
/// (tool_use.input, tool_result.content) use serde_json::Value instead.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ContentBlock {
    #[serde(rename = "text")]
    Text { text: String },

    #[serde(rename = "thinking")]
    Thinking {
        thinking: String,
        /// Present on ~94.6% of thinking blocks
        #[serde(default)]
        signature: Option<String>,
    },

    #[serde(rename = "tool_use")]
    ToolUse {
        id: String,
        name: String,
        /// Polymorphic JSON — tool inputs vary by tool name
        input: serde_json::Value,
        /// Present on ~7.4% of tool_use blocks, always `{type: "direct"}`
        #[serde(default)]
        caller: Option<serde_json::Value>,
    },

    #[serde(rename = "tool_result")]
    ToolResult {
        tool_use_id: String,
        /// Can be string (~96.7%) or array of `{type: "text", text: "..."}` objects (~3.3%)
        content: serde_json::Value,
        /// Present on ~39.5% of tool_result blocks
        #[serde(default)]
        is_error: Option<bool>,
    },
}

/// User message wrapper — contains role and content.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserMessage {
    pub role: String,
    pub content: MessageContent,
}

/// Assistant inner message — the API response message object.
///
/// The overflow HashMap captures evolving fields like context_management,
/// container, and the inner "type" field (always "message").
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssistantMessage {
    pub id: String,
    pub model: String,
    pub content: Vec<ContentBlock>,
    pub role: String,
    #[serde(default)]
    pub stop_reason: Option<String>,
    #[serde(default)]
    pub stop_sequence: Option<String>,
    #[serde(default)]
    pub usage: Option<UsageStats>,
    /// Catches context_management, container, type field, etc.
    #[serde(flatten)]
    pub overflow: HashMap<String, serde_json::Value>,
}

/// Token usage statistics from assistant message responses.
///
/// Core fields (input_tokens, output_tokens) are always present.
/// Cache-related fields appeared starting around version 2.1.x.
/// Newer fields (server_tool_use, iterations, inference_geo) appear
/// in <3% of records and are captured via overflow.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageStats {
    pub input_tokens: u64,
    pub output_tokens: u64,
    #[serde(default)]
    pub cache_creation_input_tokens: Option<u64>,
    #[serde(default)]
    pub cache_read_input_tokens: Option<u64>,
    #[serde(default)]
    pub cache_creation: Option<serde_json::Value>,
    #[serde(default)]
    pub service_tier: Option<String>,
    /// Catches inference_geo, server_tool_use, iterations, and future billing fields
    #[serde(flatten)]
    pub overflow: HashMap<String, serde_json::Value>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_message_content_text() {
        let json = r#""Hello, world!""#;
        let content: MessageContent = serde_json::from_str(json).expect("should deserialize text content");
        match content {
            MessageContent::Text(t) => assert_eq!(t, "Hello, world!"),
            _ => panic!("Expected Text variant"),
        }
    }

    #[test]
    fn test_message_content_blocks() {
        let json = r#"[{"type": "text", "text": "Hello"}, {"type": "tool_result", "tool_use_id": "t1", "content": "done"}]"#;
        let content: MessageContent = serde_json::from_str(json).expect("should deserialize block content");
        match content {
            MessageContent::Blocks(blocks) => assert_eq!(blocks.len(), 2),
            _ => panic!("Expected Blocks variant"),
        }
    }

    #[test]
    fn test_content_block_text() {
        let json = r#"{"type": "text", "text": "Hello, world!"}"#;
        let block: ContentBlock = serde_json::from_str(json).expect("should deserialize text block");
        match block {
            ContentBlock::Text { text } => assert_eq!(text, "Hello, world!"),
            _ => panic!("Expected Text block"),
        }
    }

    #[test]
    fn test_content_block_thinking_with_signature() {
        let json = r#"{"type": "thinking", "thinking": "Let me consider...", "signature": "sig-abc123"}"#;
        let block: ContentBlock = serde_json::from_str(json).expect("should deserialize thinking block");
        match block {
            ContentBlock::Thinking { thinking, signature } => {
                assert_eq!(thinking, "Let me consider...");
                assert_eq!(signature, Some("sig-abc123".to_string()));
            }
            _ => panic!("Expected Thinking block"),
        }
    }

    #[test]
    fn test_content_block_thinking_without_signature() {
        let json = r#"{"type": "thinking", "thinking": "Hmm..."}"#;
        let block: ContentBlock = serde_json::from_str(json).expect("should deserialize thinking block without signature");
        match block {
            ContentBlock::Thinking { signature, .. } => {
                assert!(signature.is_none());
            }
            _ => panic!("Expected Thinking block"),
        }
    }

    #[test]
    fn test_content_block_tool_use() {
        let json = r#"{"type": "tool_use", "id": "tool-001", "name": "Read", "input": {"file_path": "/tmp/test.txt"}}"#;
        let block: ContentBlock = serde_json::from_str(json).expect("should deserialize tool_use block");
        match block {
            ContentBlock::ToolUse { id, name, input, caller } => {
                assert_eq!(id, "tool-001");
                assert_eq!(name, "Read");
                assert!(input.is_object());
                assert!(caller.is_none());
            }
            _ => panic!("Expected ToolUse block"),
        }
    }

    #[test]
    fn test_content_block_tool_use_with_caller() {
        let json = r#"{"type": "tool_use", "id": "tool-002", "name": "Bash", "input": {"command": "ls"}, "caller": {"type": "direct"}}"#;
        let block: ContentBlock = serde_json::from_str(json).expect("should deserialize tool_use block with caller");
        match block {
            ContentBlock::ToolUse { caller, .. } => {
                assert!(caller.is_some());
            }
            _ => panic!("Expected ToolUse block"),
        }
    }

    #[test]
    fn test_content_block_tool_result_string_content() {
        let json = r#"{"type": "tool_result", "tool_use_id": "tool-001", "content": "File contents here", "is_error": false}"#;
        let block: ContentBlock = serde_json::from_str(json).expect("should deserialize tool_result block");
        match block {
            ContentBlock::ToolResult { tool_use_id, content, is_error } => {
                assert_eq!(tool_use_id, "tool-001");
                assert!(content.is_string());
                assert_eq!(is_error, Some(false));
            }
            _ => panic!("Expected ToolResult block"),
        }
    }

    #[test]
    fn test_content_block_tool_result_array_content() {
        let json = r#"{"type": "tool_result", "tool_use_id": "tool-002", "content": [{"type": "text", "text": "result data"}]}"#;
        let block: ContentBlock = serde_json::from_str(json).expect("should deserialize tool_result with array content");
        match block {
            ContentBlock::ToolResult { content, is_error, .. } => {
                assert!(content.is_array());
                assert!(is_error.is_none());
            }
            _ => panic!("Expected ToolResult block"),
        }
    }

    #[test]
    fn test_usage_stats_with_overflow() {
        let json = r#"{
            "input_tokens": 1500,
            "output_tokens": 300,
            "cache_creation_input_tokens": 100,
            "cache_read_input_tokens": 900,
            "service_tier": "standard",
            "inference_geo": "us-west-2",
            "server_tool_use": {"web_search_requests": 1, "web_fetch_requests": 0},
            "iterations": [{"count": 1}]
        }"#;
        let stats: UsageStats = serde_json::from_str(json).expect("should deserialize usage stats with overflow");
        assert_eq!(stats.input_tokens, 1500);
        assert_eq!(stats.output_tokens, 300);
        assert_eq!(stats.cache_creation_input_tokens, Some(100));
        assert_eq!(stats.cache_read_input_tokens, Some(900));
        assert_eq!(stats.service_tier, Some("standard".to_string()));
        assert!(stats.overflow.contains_key("inference_geo"), "inference_geo in overflow");
        assert!(stats.overflow.contains_key("server_tool_use"), "server_tool_use in overflow");
        assert!(stats.overflow.contains_key("iterations"), "iterations in overflow");
    }

    #[test]
    fn test_usage_stats_minimal() {
        let json = r#"{"input_tokens": 10, "output_tokens": 5}"#;
        let stats: UsageStats = serde_json::from_str(json).expect("should deserialize minimal usage stats");
        assert_eq!(stats.input_tokens, 10);
        assert_eq!(stats.output_tokens, 5);
        assert!(stats.cache_creation_input_tokens.is_none());
        assert!(stats.overflow.is_empty());
    }

    #[test]
    fn test_assistant_message_overflow() {
        let json = r#"{
            "id": "msg_003",
            "type": "message",
            "role": "assistant",
            "model": "claude-opus-4-6",
            "content": [{"type": "text", "text": "Hello"}],
            "stop_reason": "end_turn",
            "usage": {"input_tokens": 100, "output_tokens": 50},
            "context_management": {"strategy": "truncate"},
            "container": null
        }"#;
        let msg: AssistantMessage = serde_json::from_str(json).expect("should deserialize assistant message with overflow");
        assert_eq!(msg.id, "msg_003");
        assert_eq!(msg.model, "claude-opus-4-6");
        // "type" field from JSON should land in overflow since it's not a named field on AssistantMessage
        assert!(msg.overflow.contains_key("type"), "inner 'type' field should be in overflow");
        assert!(msg.overflow.contains_key("context_management"), "context_management should be in overflow");
        assert!(msg.overflow.contains_key("container"), "container should be in overflow");
    }
}
