// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The chat completions body of a model request: the system prompt first,
//! then each message (a tool's results as `tool` messages after the call),
//! the tools as functions, streaming with usage; the provider's own fields
//! last.

use pcx_module_api::model::{
    Block, ModelFailure, ModelFailureKind, ModelMessage, ModelRequest, Role, ToolChoice,
};
use serde_json::{Map, Value, json};

use crate::Wire;

/// The body of `request` as `wire` has it: its most tokens in the field it
/// names, its own fields merged at the root.
///
/// # Errors
/// `Invalid` for a tool call whose input does not serialize, or a block
/// in a message of a role that cannot carry it.
pub fn body(request: &ModelRequest, wire: &Wire) -> Result<Value, ModelFailure> {
    let mut messages = Vec::new();
    if !request.system.is_empty() {
        messages.push(json!({ "role": "system", "content": request.system }));
    }
    for message in &request.messages {
        messages.extend(wire_messages(message)?);
    }
    let mut body = Map::from_iter([
        ("model".to_string(), json!(request.model)),
        (wire.max_tokens.to_string(), json!(request.max_tokens)),
        ("stream".to_string(), json!(true)),
        // Without it, the stream carries no usage.
        (
            "stream_options".to_string(),
            json!({ "include_usage": true }),
        ),
        ("messages".to_string(), Value::Array(messages)),
    ]);
    if !request.tools.is_empty() {
        let tools: Vec<Value> = request
            .tools
            .iter()
            .map(|t| {
                json!({ "type": "function", "function": {
                    "name": t.name, "description": t.description, "parameters": t.input_schema
                } })
            })
            .collect();
        body.insert("tools".to_string(), Value::Array(tools));
    }
    if let ToolChoice::Tool(name) = &request.tool_choice {
        body.insert(
            "tool_choice".to_string(),
            json!({ "type": "function", "function": { "name": name } }),
        );
    }
    body.extend(wire.extra.clone());
    Ok(Value::Object(body))
}

fn invalid(message: String) -> ModelFailure {
    ModelFailure::new(ModelFailureKind::Invalid, message)
}

/// The wire messages of `message`. A user message's tool results become
/// `tool` messages, its text one user message; an assistant's text and
/// tool calls one assistant message. Reasoning is not sent back: the wire
/// has no field for it.
fn wire_messages(message: &ModelMessage) -> Result<Vec<Value>, ModelFailure> {
    let mut texts = Vec::new();
    let mut calls = Vec::new();
    let mut results = Vec::new();
    for block in &message.blocks {
        match (message.role, block) {
            (_, Block::Text { text } | Block::Summary { text }) => texts.push(text.as_str()),
            (_, Block::Thinking { .. }) => {}
            (_, Block::End { stop, .. }) => {
                return Err(invalid(format!(
                    "a turn's end ({stop}) is kept in its transcript, never sent to a model"
                )));
            }
            (Role::Assistant, Block::ToolUse { id, name, input }) => {
                let arguments = serde_json::to_string(input)
                    .map_err(|e| invalid(format!("the input of tool call {id}: {e}")))?;
                calls.push(json!({ "id": id, "type": "function",
                    "function": { "name": name, "arguments": arguments } }));
            }
            (
                Role::User,
                Block::ToolResult {
                    tool_use_id,
                    content,
                    ..
                },
            ) => results
                .push(json!({ "role": "tool", "tool_call_id": tool_use_id, "content": content })),
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
    let text = texts.join("\n");
    Ok(match message.role {
        Role::User => {
            let mut all = results;
            if !text.is_empty() {
                all.push(json!({ "role": "user", "content": text }));
            }
            all
        }
        Role::Assistant => {
            let mut wire = Map::from_iter([
                ("role".to_string(), json!("assistant")),
                ("content".to_string(), json!(text)),
            ]);
            if !calls.is_empty() {
                wire.insert("tool_calls".to_string(), Value::Array(calls));
            }
            vec![Value::Object(wire)]
        }
    })
}

#[cfg(test)]
mod tests;
