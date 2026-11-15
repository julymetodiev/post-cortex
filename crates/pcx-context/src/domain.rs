// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The context engine's domain: what a summary is asked, and what a
//! recall adds to a turn. Pure.

use pcx_module_api::model::{Block, ModelMessage, ModelRequest, Role, ToolChoice};

/// The most tokens of a summary.
pub const SUMMARY_TOKENS: u32 = 4000;

/// The most characters of a tool's result a summary is shown (Axon's): the
/// rest is cut.
pub const TOOL_RESULT_CHARS: usize = 2000;

/// What the model is told when it sums up a conversation.
const SUMMARY_SYSTEM: &str = "You sum up a conversation between a person and an agent so the agent can go on from your summary alone. Keep what was asked, what was decided and why, what was done (files, commands, results), what failed, and what is still open; drop greetings and repetition. Write plain prose, at most a page.";

/// What closes the conversation to sum up.
const SUMMARY_ASK: &str = "Sum up the conversation above, as the system prompt says.";

/// The request that sums up `messages` with `model`, each tool's result
/// cut to [`TOOL_RESULT_CHARS`].
#[must_use]
pub fn summary_request(model: &str, messages: &[ModelMessage]) -> ModelRequest {
    let mut messages: Vec<ModelMessage> = messages
        .iter()
        .map(|m| ModelMessage {
            role: m.role,
            blocks: m
                .blocks
                .iter()
                .map(|b| match b {
                    Block::ToolResult {
                        tool_use_id,
                        content,
                        is_error,
                    } => Block::ToolResult {
                        tool_use_id: tool_use_id.clone(),
                        content: pcx_module_api::text::clip(content, TOOL_RESULT_CHARS),
                        is_error: *is_error,
                    },
                    other => other.clone(),
                })
                .collect(),
        })
        .collect();
    messages.push(ModelMessage {
        role: Role::User,
        blocks: vec![Block::Text {
            text: SUMMARY_ASK.to_string(),
        }],
    });
    ModelRequest {
        model: model.to_string(),
        system: SUMMARY_SYSTEM.to_string(),
        messages,
        tools: Vec::new(),
        max_tokens: SUMMARY_TOKENS,
        tool_choice: ToolChoice::Auto,
    }
}

/// The summary in an answer's `blocks`: its text, trimmed; none when it
/// says nothing.
#[must_use]
pub fn summary_of(blocks: &[Block]) -> Option<String> {
    let text: Vec<&str> = blocks
        .iter()
        .filter_map(|b| match b {
            Block::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    let text = text.join("\n").trim().to_string();
    (!text.is_empty()).then_some(text)
}

/// What a recall adds to a turn: memory's text, when it found items.
#[must_use]
pub fn recalled(items: usize, text: String) -> String {
    if items == 0 { String::new() } else { text }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_summary_asks_for_it_last_and_reads_the_answers_text() {
        let said = vec![ModelMessage {
            role: Role::User,
            blocks: vec![Block::Text { text: "hi".into() }],
        }];
        let asked = summary_request("glm", &said);
        assert_eq!(asked.messages.len(), 2);
        assert!(asked.tools.is_empty());
        let answer = vec![
            Block::Thinking {
                text: "hm".into(),
                signature: None,
            },
            Block::Text {
                text: " They greeted. ".into(),
            },
        ];
        assert_eq!(summary_of(&answer).as_deref(), Some("They greeted."));
        assert_eq!(summary_of(&answer[..1]), None);
        assert_eq!(recalled(0, "header".into()), "");
        assert_eq!(recalled(2, "found".into()), "found");
    }

    #[test]
    fn a_long_tool_result_is_cut_for_the_summary() {
        let result = |content: String| ModelMessage {
            role: Role::User,
            blocks: vec![Block::ToolResult {
                tool_use_id: "t1".into(),
                content,
                is_error: false,
            }],
        };
        let asked = summary_request("glm", &[result("é".repeat(5000)), result("short".into())]);
        let contents: Vec<String> = asked.messages[..2]
            .iter()
            .map(|m| match &m.blocks[0] {
                Block::ToolResult { content, .. } => content.clone(),
                _ => panic!("not a tool result"),
            })
            .collect();
        assert_eq!(contents[0], format!("{}...", "é".repeat(TOOL_RESULT_CHARS)));
        assert_eq!(contents[1], "short");
    }
}
