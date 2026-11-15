// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use super::*;

use pcx_module_api::model::ModelTool;

fn request(messages: Vec<ModelMessage>) -> ModelRequest {
    ModelRequest {
        model: "glm-5.3".into(),
        system: "Be brief.".into(),
        messages,
        tools: vec![ModelTool {
            name: "fs_read".into(),
            description: "Read a file".into(),
            input_schema: json!({ "type": "object" }).as_object().cloned().unwrap(),
        }],
        max_tokens: 100,
        tool_choice: ToolChoice::Auto,
    }
}

#[test]
fn a_tool_turn_is_an_assistant_call_then_its_tool_result() {
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
                    text: "hm".into(),
                    signature: None,
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
            blocks: vec![Block::ToolResult {
                tool_use_id: "c1".into(),
                content: "fn main() {}".into(),
                is_error: false,
            }],
        },
    ];
    let wire = Wire {
        max_tokens: "max_completion_tokens",
        extra: Map::from_iter([("enable_thinking".to_string(), json!(true))]),
    };
    let body = body(&request(messages), &wire).unwrap();
    assert_eq!(
        body["messages"],
        json!([
            { "role": "system", "content": "Be brief." },
            { "role": "user", "content": "Read a.rs" },
            { "role": "assistant", "content": "", "tool_calls": [
                { "id": "c1", "type": "function",
                  "function": { "name": "fs_read", "arguments": "{\"path\":\"a.rs\"}" } }
            ] },
            { "role": "tool", "tool_call_id": "c1", "content": "fn main() {}" }
        ])
    );
    assert_eq!(body["tools"][0]["function"]["name"], json!("fs_read"));
    assert_eq!(body["stream_options"]["include_usage"], json!(true));
    assert_eq!(body["enable_thinking"], json!(true));
    // The most tokens in the field the wire names, only there.
    assert!(body.get("max_completion_tokens").is_some() && body.get("max_tokens").is_none());
    assert!(body.get("tool_choice").is_none());
}

#[test]
fn a_forced_tool_and_misplaced_blocks() {
    let mut forced = request(Vec::new());
    forced.tool_choice = ToolChoice::Tool("fs_read".into());
    let body = body(&forced, &plain()).unwrap();
    assert_eq!(body["tool_choice"]["function"]["name"], json!("fs_read"));

    let misplaced = request(vec![ModelMessage {
        role: Role::User,
        blocks: vec![Block::ToolUse {
            id: "c1".into(),
            name: "x".into(),
            input: json!({}),
        }],
    }]);
    let refused = super::body(&misplaced, &plain()).unwrap_err();
    assert_eq!(refused.kind, ModelFailureKind::Invalid);
}

fn plain() -> Wire {
    Wire {
        max_tokens: "max_tokens",
        extra: Map::new(),
    }
}
