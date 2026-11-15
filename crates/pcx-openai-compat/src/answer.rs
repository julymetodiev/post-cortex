// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! An answer built from the events of its stream: text and reasoning
//! deltas as they come, tool calls gathered by their index, usage, and the
//! finish reason; an error the stream sends is a failure of the kind its
//! code names.

use std::collections::BTreeMap;

use pcx_module_api::model::{
    AnswerReader, Block, ModelEvent, ModelFailure, ModelFailureKind, StopReason, Usage,
};
use serde_json::Value;

use crate::http::Codes;

/// A tool call being streamed.
#[derive(Debug, Default)]
struct Call {
    id: String,
    name: String,
    arguments: String,
}

/// An answer being read.
#[derive(Debug)]
pub struct Answer<'a> {
    codes: &'a Codes,
    text: String,
    thinking: String,
    calls: BTreeMap<u64, Call>,
    usage: Usage,
    stop: Option<StopReason>,
}

fn invalid(message: String) -> ModelFailure {
    ModelFailure::new(ModelFailureKind::Invalid, message)
}

fn count(usage: &Value, pointer: &str) -> u64 {
    usage.pointer(pointer).and_then(Value::as_u64).unwrap_or(0)
}

impl<'a> Answer<'a> {
    /// Nothing read yet; an error the stream sends is classified by
    /// `codes`.
    #[must_use]
    pub fn new(codes: &'a Codes) -> Self {
        Self {
            codes,
            text: String::new(),
            thinking: String::new(),
            calls: BTreeMap::new(),
            usage: Usage::default(),
            stop: None,
        }
    }

    /// Read the event `data`: the deltas it carries.
    ///
    /// # Errors
    /// The stream's own error; `Invalid` for data that does not read, a
    /// tool call without its index, or a finish reason the model stops for
    /// another cause (a filter, its network).
    pub fn read(&mut self, data: &str) -> Result<Vec<ModelEvent>, ModelFailure> {
        let event: Value = serde_json::from_str(data)
            .map_err(|e| invalid(format!("the stream sent data that is not JSON: {e}")))?;
        if let Some(error) = event.get("error") {
            return Err(self.codes.failure(None, error));
        }
        if let Some(usage) = event.get("usage").filter(|u| u.is_object()) {
            // Providers name the cached input in one of these places.
            let cached = [
                "/prompt_tokens_details/cached_tokens",
                "/cached_tokens",
                "/prompt_cache_hit_tokens",
            ]
            .iter()
            .map(|p| count(usage, p))
            .max()
            .unwrap_or(0);
            self.usage = Usage {
                input: count(usage, "/prompt_tokens").saturating_sub(cached),
                output: count(usage, "/completion_tokens"),
                cache_read: cached,
                cache_write: 0,
            };
        }
        let mut events = Vec::new();
        for choice in event
            .get("choices")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if let Some(delta) = choice.get("delta") {
                self.delta(delta, &mut events)?;
            }
            if let Some(reason) = choice.get("finish_reason").and_then(Value::as_str) {
                self.stop = Some(match reason {
                    "stop" => StopReason::EndTurn,
                    "length" => StopReason::MaxTokens,
                    "tool_calls" => StopReason::ToolUse,
                    // A stream that ends abnormally names its cause here.
                    "network_error" => {
                        return Err(ModelFailure::new(
                            ModelFailureKind::Unavailable,
                            "the model stopped: network_error",
                        ));
                    }
                    other => return Err(invalid(format!("the model stopped: {other}"))),
                });
            }
        }
        Ok(events)
    }

    fn delta(&mut self, delta: &Value, events: &mut Vec<ModelEvent>) -> Result<(), ModelFailure> {
        // Z.AI names the reasoning `reasoning_content`, others `reasoning`.
        let reasoning = ["reasoning_content", "reasoning"]
            .iter()
            .find_map(|field| delta.get(*field).and_then(Value::as_str));
        if let Some(text) = reasoning.filter(|t| !t.is_empty()) {
            self.thinking.push_str(text);
            events.push(ModelEvent::ThinkingDelta(text.to_string()));
        }
        if let Some(text) = delta
            .get("content")
            .and_then(Value::as_str)
            .filter(|t| !t.is_empty())
        {
            self.text.push_str(text);
            events.push(ModelEvent::TextDelta(text.to_string()));
        }
        for call in delta
            .get("tool_calls")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let index = call
                .get("index")
                .and_then(Value::as_u64)
                .ok_or_else(|| invalid("the stream sent a tool call without its index".into()))?;
            let kept = self.calls.entry(index).or_default();
            if let Some(id) = call.get("id").and_then(Value::as_str) {
                kept.id = id.to_string();
            }
            if let Some(name) = call.pointer("/function/name").and_then(Value::as_str) {
                kept.name = name.to_string();
            }
            if let Some(part) = call.pointer("/function/arguments").and_then(Value::as_str) {
                kept.arguments.push_str(part);
            }
        }
        Ok(())
    }

    /// The end of the stream: each tool call, whole, then the answer.
    ///
    /// # Errors
    /// `Unavailable` when it ended without a finish reason; `Invalid` for
    /// a tool call without its id or name, or whose arguments are not a
    /// JSON object.
    pub fn finish(self) -> Result<Vec<ModelEvent>, ModelFailure> {
        let stop = self.stop.ok_or_else(|| {
            ModelFailure::new(
                ModelFailureKind::Unavailable,
                "the stream ended before the model finished",
            )
        })?;
        let mut blocks = Vec::new();
        if !self.thinking.is_empty() {
            blocks.push(Block::Thinking {
                text: self.thinking,
                signature: None,
            });
        }
        if !self.text.is_empty() {
            blocks.push(Block::Text { text: self.text });
        }
        let mut events = Vec::new();
        for (index, call) in self.calls {
            if call.id.is_empty() || call.name.is_empty() {
                return Err(invalid(format!(
                    "tool call {index} came without its id or name"
                )));
            }
            // A call without arguments takes none.
            let arguments = if call.arguments.trim().is_empty() {
                "{}"
            } else {
                call.arguments.as_str()
            };
            let input: Value = match serde_json::from_str(arguments) {
                Ok(input) => input,
                // Cut by the most tokens: it never ran, and the agent asks again.
                Err(_) if stop == StopReason::MaxTokens => continue,
                Err(e) => {
                    return Err(invalid(format!(
                        "the arguments of tool call {}: {e}",
                        call.name
                    )));
                }
            };
            if !input.is_object() {
                return Err(invalid(format!(
                    "the arguments of tool call {} are not an object",
                    call.name
                )));
            }
            events.push(ModelEvent::ToolUse {
                id: call.id.clone(),
                name: call.name.clone(),
                input: input.clone(),
            });
            blocks.push(Block::ToolUse {
                id: call.id,
                name: call.name,
                input,
            });
        }
        events.push(ModelEvent::Done {
            blocks,
            usage: self.usage,
            stop,
        });
        Ok(events)
    }
}

impl AnswerReader for Answer<'_> {
    fn read(&mut self, data: &str) -> Result<Vec<ModelEvent>, ModelFailure> {
        Answer::read(self, data)
    }

    fn finish(self) -> Result<Vec<ModelEvent>, ModelFailure> {
        Answer::finish(self)
    }
}

#[cfg(test)]
mod tests;
