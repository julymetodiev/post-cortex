// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use pcx_module_api::model::ModelTool;
use serde_json::Map;

use super::*;

fn request(messages: Vec<ModelMessage>) -> ModelRequest {
    ModelRequest {
        model: "claude-opus-5-5".into(),
        system: "Be brief.".into(),
        messages,
        tools: vec![ModelTool {
            name: "fs_read".into(),
            description: "Read a file".into(),
            input_schema: Map::from_iter([("type".to_string(), json!("object"))]),
        }],
        max_tokens: 1000,
        tool_choice: ToolChoice::Auto,
    }
}

#[test]
fn a_tool_turn_with_signed_reasoning_and_the_cache() {
    let messages = vec![
        ModelMessage {
            role: Role::User,
            blocks: vec![Block::Text {
                text: "Read a.rs".into(),
            }],
        },
        ModelMessage {
            role: Role::Assistant,
            blocks: vec![
                Block::Thinking {
                    text: "Look.".into(),
                    signature: Some("sig".into()),
                },
                // Another provider's reasoning, unsigned: not sent.
                Block::Thinking {
                    text: "GLM thought".into(),
                    signature: None,
                },
                Block::Text {
                    text: String::new(),
                },
                Block::ToolUse {
                    id: "c1".into(),
                    name: "fs_read".into(),
                    input: json!({ "path": "a.rs" }),
                },
            ],
        },
        ModelMessage {
            role: Role::User,
            blocks: vec![
                Block::Text {
                    text: "and then?".into(),
                },
                Block::ToolResult {
                    tool_use_id: "c1".into(),
                    content: "fn main() {}".into(),
                    is_error: false,
                },
            ],
        },
    ];
    let body = body(&request(messages), true).unwrap();
    assert_eq!(
        body["messages"],
        json!([
            { "role": "user", "content": [{ "type": "text", "text": "Read a.rs" }] },
            { "role": "assistant", "content": [
                { "type": "thinking", "thinking": "Look.", "signature": "sig" },
                { "type": "tool_use", "id": "c1", "name": "fs_read", "input": { "path": "a.rs" } }
            ] },
            { "role": "user", "content": [
                { "type": "tool_result", "tool_use_id": "c1", "content": "fn main() {}", "is_error": false },
                { "type": "text", "text": "and then?" }
            ] }
        ])
    );
    assert_eq!(body["system"], json!("Be brief."));
    assert_eq!(
        body["thinking"],
        json!({ "type": "adaptive", "display": "summarized" })
    );
    assert_eq!(body["cache_control"], json!({ "type": "ephemeral" }));
    assert_eq!(
        body["tools"][0]["input_schema"],
        json!({ "type": "object" })
    );
    assert!(body.get("tool_choice").is_none());
}

#[test]
fn a_model_that_does_not_think_adaptively_is_not_asked_to() {
    let body = body(&request(Vec::new()), false).unwrap();
    assert!(body.get("thinking").is_none());
}

#[test]
fn a_misplaced_block_is_refused() {
    let misplaced = request(vec![ModelMessage {
        role: Role::User,
        blocks: vec![Block::ToolUse {
            id: "c1".into(),
            name: "x".into(),
            input: json!({}),
        }],
    }]);
    assert_eq!(
        body(&misplaced, false).unwrap_err().kind,
        ModelFailureKind::Invalid
    );
}
