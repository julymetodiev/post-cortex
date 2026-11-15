// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The Messages API body of a model request: the system prompt, each
//! message's blocks as content blocks (reasoning sent back with its
//! signature, as the API asks; reasoning of another provider, unsigned, is
//! not sent), the tools, streaming, the prompt cache on its stable prefix,
//! and adaptive thinking for a model that has it.

use pcx_module_api::model::{
    Block, ModelFailure, ModelFailureKind, ModelMessage, ModelRequest, Role, ToolChoice,
};
use serde_json::{Map, Value, json};

fn invalid(message: String) -> ModelFailure {
    ModelFailure::new(ModelFailureKind::Invalid, message)
}

/// The body of `request`; `thinks` when the model thinks adaptively (its
/// reasoning summed up, so a person reads it).
///
/// # Errors
/// `Invalid` for a block in a message of a role that cannot carry it.
pub fn body(request: &ModelRequest, thinks: bool) -> Result<Value, ModelFailure> {
    let mut messages = Vec::new();
    for message in &request.messages {
        let content = content(message)?;
        // A message with nothing to send (only another provider's
        // reasoning) is left out.
        if content.is_empty() {
            continue;
        }
        let role = match message.role {
            Role::User => "user",
            Role::Assistant => "assistant",
        };
        messages.push(json!({ "role": role, "content": content }));
    }
    let mut body = Map::from_iter([
        ("model".to_string(), json!(request.model)),
        ("max_tokens".to_string(), json!(request.max_tokens)),
        ("stream".to_string(), json!(true)),
        // The cache on the conversation's stable prefix, placed by the API.
        ("cache_control".to_string(), json!({ "type": "ephemeral" })),
        ("messages".to_string(), Value::Array(messages)),
    ]);
    if !request.system.is_empty() {
        body.insert("system".to_string(), json!(request.system));
    }
    if thinks {
        body.insert(
            "thinking".to_string(),
            json!({ "type": "adaptive", "display": "summarized" }),
        );
    }
    if !request.tools.is_empty() {
        let tools: Vec<Value> = request
            .tools
            .iter()
            .map(|t| {
                json!({ "name": t.name, "description": t.description, "input_schema": t.input_schema })
            })
            .collect();
        body.insert("tools".to_string(), Value::Array(tools));
    }
    if let ToolChoice::Tool(name) = &request.tool_choice {
        body.insert(
            "tool_choice".to_string(),
            json!({ "type": "tool", "name": name }),
        );
    }
    Ok(Value::Object(body))
}

/// The content blocks of `message`.
fn content(message: &ModelMessage) -> Result<Vec<Value>, ModelFailure> {
    let mut blocks = Vec::new();
    for block in &message.blocks {
        match (message.role, block) {
            (_, Block::Text { text } | Block::Summary { text }) => {
                // The API refuses an empty text block.
                if !text.is_empty() {
                    blocks.push(json!({ "type": "text", "text": text }));
                }
            }
            (Role::Assistant, Block::Thinking { text, signature }) => {
                if let Some(signature) = signature {
                    blocks.push(
                        json!({ "type": "thinking", "thinking": text, "signature": signature }),
                    );
                }
            }
            (Role::User, Block::Thinking { .. }) => {
                return Err(invalid("a user message carries reasoning".into()));
            }
            (_, Block::End { stop, .. }) => {
                return Err(invalid(format!(
                    "a turn's end ({stop}) is kept in its transcript, never sent to a model"
                )));
            }
            (Role::Assistant, Block::ToolUse { id, name, input }) => {
                blocks.push(json!({ "type": "tool_use", "id": id, "name": name, "input": input }));
            }
            (
                Role::User,
                Block::ToolResult {
                    tool_use_id,
                    content,
                    is_error,
                },
            ) => blocks.push(json!({
                "type": "tool_result", "tool_use_id": tool_use_id,
                "content": content, "is_error": is_error
            })),
            (Role::User, Block::ToolUse { id, .. }) => {
                return Err(invalid(format!("a user message calls tool {id}")));
            }
            (Role::Assistant, Block::ToolResult { tool_use_id, .. }) => {
                return Err(invalid(format!(
                    "an assistant message answers tool call {tool_use_id}"
                )));
            }
        }
    }
    // A user message's tool results come before its text.
    blocks.sort_by_key(|b| b.get("type") != Some(&json!("tool_result")));
    Ok(blocks)
}

#[cfg(test)]
mod tests;
