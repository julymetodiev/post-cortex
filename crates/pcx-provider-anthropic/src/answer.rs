// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! An answer built from the events of a Messages API stream: its content
//! blocks by their index (text, reasoning with its signature, tool calls
//! whose input streams as JSON), its usage, and its stop reason; an error
//! the stream sends is a failure of the kind its type names.

use std::collections::BTreeMap;

use pcx_module_api::model::{
    AnswerReader, Block, ModelEvent, ModelFailure, ModelFailureKind, StopReason, Usage,
};
use serde_json::Value;

use crate::http::failure;

/// A content block being streamed.
#[derive(Debug)]
enum Part {
    Text(String),
    Thinking {
        text: String,
        signature: String,
    },
    Tool {
        id: String,
        name: String,
        input: String,
    },
    /// A block it does not keep (reasoning it hides, a server tool's).
    Other,
}

/// An answer being read.
#[derive(Debug, Default)]
pub struct Answer {
    parts: BTreeMap<u64, Part>,
    usage: Usage,
    stop: Option<StopReason>,
}

fn invalid(message: String) -> ModelFailure {
    ModelFailure::new(ModelFailureKind::Invalid, message)
}

fn count(usage: &Value, field: &str) -> u64 {
    usage.get(field).and_then(Value::as_u64).unwrap_or(0)
}

fn text(value: &Value, field: &str) -> String {
    value
        .get(field)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

impl Answer {
    /// Nothing read yet.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Read the event `data`: the deltas it carries.
    ///
    /// # Errors
    /// The stream's own error; `Invalid` for data that does not read, a
    /// delta of a block that did not start, or a stop the model makes for
    /// another cause (a refusal); `ContextTooLong` when the conversation
    /// filled the model's window.
    pub fn read(&mut self, data: &str) -> Result<Vec<ModelEvent>, ModelFailure> {
        let event: Value = serde_json::from_str(data)
            .map_err(|e| invalid(format!("the stream sent data that is not JSON: {e}")))?;
        let index = || {
            event
                .get("index")
                .and_then(Value::as_u64)
                .ok_or_else(|| invalid("the stream sent a block without its index".into()))
        };
        let mut events = Vec::new();
        match event.get("type").and_then(Value::as_str) {
            Some("message_start") => {
                if let Some(usage) = event.pointer("/message/usage") {
                    self.usage = Usage {
                        input: count(usage, "input_tokens"),
                        output: count(usage, "output_tokens"),
                        cache_read: count(usage, "cache_read_input_tokens"),
                        cache_write: count(usage, "cache_creation_input_tokens"),
                    };
                }
            }
            Some("content_block_start") => {
                let block = event.get("content_block").unwrap_or(&Value::Null);
                let part = match block.get("type").and_then(Value::as_str) {
                    Some("text") => Part::Text(text(block, "text")),
                    Some("thinking") => Part::Thinking {
                        text: text(block, "thinking"),
                        signature: text(block, "signature"),
                    },
                    Some("tool_use") => Part::Tool {
                        id: text(block, "id"),
                        name: text(block, "name"),
                        input: String::new(),
                    },
                    _ => Part::Other,
                };
                self.parts.insert(index()?, part);
            }
            Some("content_block_delta") => {
                let at = index()?;
                let part = self.parts.get_mut(&at).ok_or_else(|| {
                    invalid(format!("a delta of block {at}, which did not start"))
                })?;
                let delta = event.get("delta").unwrap_or(&Value::Null);
                match (part, delta.get("type").and_then(Value::as_str)) {
                    (Part::Text(kept), Some("text_delta")) => {
                        let more = text(delta, "text");
                        kept.push_str(&more);
                        events.push(ModelEvent::TextDelta(more));
                    }
                    (Part::Thinking { text: kept, .. }, Some("thinking_delta")) => {
                        let more = text(delta, "thinking");
                        kept.push_str(&more);
                        events.push(ModelEvent::ThinkingDelta(more));
                    }
                    (Part::Thinking { signature, .. }, Some("signature_delta")) => {
                        signature.push_str(&text(delta, "signature"));
                    }
                    (Part::Tool { input, .. }, Some("input_json_delta")) => {
                        input.push_str(&text(delta, "partial_json"));
                    }
                    // Deltas of blocks it does not keep, and kinds it does
                    // not read (citations).
                    _ => {}
                }
            }
            Some("message_delta") => {
                if let Some(output) = event
                    .pointer("/usage/output_tokens")
                    .and_then(Value::as_u64)
                {
                    self.usage.output = output;
                }
                if let Some(reason) = event.pointer("/delta/stop_reason").and_then(Value::as_str) {
                    self.stop = Some(match reason {
                        "end_turn" | "stop_sequence" => StopReason::EndTurn,
                        "max_tokens" => StopReason::MaxTokens,
                        "tool_use" => StopReason::ToolUse,
                        "model_context_window_exceeded" => {
                            return Err(ModelFailure::new(
                                ModelFailureKind::ContextTooLong,
                                "the conversation filled the model's window",
                            ));
                        }
                        other => return Err(invalid(format!("the model stopped: {other}"))),
                    });
                }
            }
            Some("error") => {
                return Err(failure(None, event.get("error").unwrap_or(&Value::Null)));
            }
            // `ping`, `content_block_stop`, `message_stop`, and kinds added
            // later.
            _ => {}
        }
        Ok(events)
    }

    /// The end of the stream: each tool call, whole, then the answer, its
    /// blocks in their order.
    ///
    /// # Errors
    /// `Unavailable` when it ended without a stop reason; `Invalid` for a
    /// tool call without its id or name, or whose input is not a JSON
    /// object.
    pub fn finish(self) -> Result<Vec<ModelEvent>, ModelFailure> {
        let stop = self.stop.ok_or_else(|| {
            ModelFailure::new(
                ModelFailureKind::Unavailable,
                "the stream ended before the model finished",
            )
        })?;
        let mut blocks = Vec::new();
        let mut events = Vec::new();
        for (index, part) in self.parts {
            match part {
                Part::Text(text) if !text.is_empty() => blocks.push(Block::Text { text }),
                Part::Thinking { text, signature } => blocks.push(Block::Thinking {
                    text,
                    signature: Some(signature).filter(|s| !s.is_empty()),
                }),
                Part::Tool { id, name, input } => {
                    if id.is_empty() || name.is_empty() {
                        return Err(invalid(format!(
                            "tool call {index} came without its id or name"
                        )));
                    }
                    // A call without input takes none.
                    let input = if input.trim().is_empty() {
                        "{}"
                    } else {
                        input.as_str()
                    };
                    let input: Value = match serde_json::from_str(input) {
                        Ok(input) => input,
                        // Cut by the most tokens: it never ran, and the agent asks again.
                        Err(_) if stop == StopReason::MaxTokens => continue,
                        Err(e) => {
                            return Err(invalid(format!("the input of tool call {name}: {e}")));
                        }
                    };
                    if !input.is_object() {
                        return Err(invalid(format!(
                            "the input of tool call {name} is not an object"
                        )));
                    }
                    events.push(ModelEvent::ToolUse {
                        id: id.clone(),
                        name: name.clone(),
                        input: input.clone(),
                    });
                    blocks.push(Block::ToolUse { id, name, input });
                }
                Part::Text(_) | Part::Other => {}
            }
        }
        events.push(ModelEvent::Done {
            blocks,
            usage: self.usage,
            stop,
        });
        Ok(events)
    }
}

impl AnswerReader for Answer {
    fn read(&mut self, data: &str) -> Result<Vec<ModelEvent>, ModelFailure> {
        Answer::read(self, data)
    }

    fn finish(self) -> Result<Vec<ModelEvent>, ModelFailure> {
        Answer::finish(self)
    }
}

#[cfg(test)]
mod tests;
