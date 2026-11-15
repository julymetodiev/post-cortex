// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The models on the wire (`pcx_model.proto`, its blocks `pcx_blocks.proto`): each type to its message and
//! back. Reading refuses what the wire leaves unset or out of shape.

use std::time::Duration;

use post_cortex_proto::blocks;
use post_cortex_proto::module as wire;
use serde_json::Value;

use crate::json;
use crate::model::{
    Block, ModelEvent, ModelFailure, ModelFailureKind, ModelMessage, ModelRequest, ModelTool, Role,
    StopReason, ToolChoice, Usage,
};

fn invalid(message: impl Into<String>) -> ModelFailure {
    ModelFailure::new(ModelFailureKind::Invalid, message)
}

/// A tool call's input, an object, on the wire.
fn input_to_wire(id: &str, input: Value) -> Result<pbjson_types::Struct, ModelFailure> {
    match input {
        Value::Object(fields) => Ok(json::object_to_wire(fields)),
        _ => Err(invalid(format!(
            "the input of tool call {id} is not an object"
        ))),
    }
}

fn input_from_wire(id: &str, input: Option<pbjson_types::Struct>) -> Result<Value, ModelFailure> {
    input
        .map(|fields| Value::Object(json::object_from_wire(fields)))
        .ok_or_else(|| invalid(format!("tool call {id} has no input")))
}

impl Block {
    /// On the wire.
    ///
    /// # Errors
    /// `Invalid` for a tool call whose input is not an object.
    pub fn to_wire(self) -> Result<blocks::ModelBlock, ModelFailure> {
        use blocks::model_block::Block as B;
        let block = match self {
            Self::Text { text } => B::Text(text),
            Self::Thinking { text, signature } => {
                B::Thinking(blocks::ThinkingBlock { text, signature })
            }
            Self::ToolUse { id, name, input } => B::ToolUse(blocks::ToolUseBlock {
                input: Some(input_to_wire(&id, input)?),
                id,
                name,
            }),
            Self::Summary { text } => B::Summary(text),
            Self::End { stop, detail } => B::End(blocks::TurnEndBlock { stop, detail }),
            Self::ToolResult {
                tool_use_id,
                content,
                is_error,
            } => B::ToolResult(blocks::ToolResultBlock {
                tool_use_id,
                content,
                is_error,
            }),
        };
        Ok(blocks::ModelBlock { block: Some(block) })
    }

    /// From the wire.
    ///
    /// # Errors
    /// `Invalid` for a block that is unset.
    pub fn from_wire(block: blocks::ModelBlock) -> Result<Self, ModelFailure> {
        use blocks::model_block::Block as B;
        Ok(
            match block.block.ok_or_else(|| invalid("a block is unset"))? {
                B::Text(text) => Self::Text { text },
                B::Thinking(t) => Self::Thinking {
                    text: t.text,
                    signature: t.signature,
                },
                B::ToolUse(t) => Self::ToolUse {
                    input: input_from_wire(&t.id, t.input)?,
                    id: t.id,
                    name: t.name,
                },
                B::Summary(text) => Self::Summary { text },
                B::End(end) => Self::End {
                    stop: end.stop,
                    detail: end.detail,
                },
                B::ToolResult(t) => Self::ToolResult {
                    tool_use_id: t.tool_use_id,
                    content: t.content,
                    is_error: t.is_error,
                },
            },
        )
    }
}

fn blocks_to_wire(blocks: Vec<Block>) -> Result<Vec<blocks::ModelBlock>, ModelFailure> {
    blocks.into_iter().map(Block::to_wire).collect()
}

fn blocks_from_wire(blocks: Vec<blocks::ModelBlock>) -> Result<Vec<Block>, ModelFailure> {
    blocks.into_iter().map(Block::from_wire).collect()
}

impl ModelMessage {
    /// On the wire.
    ///
    /// # Errors
    /// As [`Block::to_wire`].
    pub fn to_wire(self) -> Result<wire::ModelMessage, ModelFailure> {
        Ok(wire::ModelMessage {
            role: match self.role {
                Role::User => wire::ModelRole::User,
                Role::Assistant => wire::ModelRole::Assistant,
            }
            .into(),
            blocks: blocks_to_wire(self.blocks)?,
        })
    }

    /// From the wire.
    ///
    /// # Errors
    /// `Invalid` for a role or a block that is unset.
    pub fn from_wire(message: wire::ModelMessage) -> Result<Self, ModelFailure> {
        let role = match wire::ModelRole::try_from(message.role) {
            Ok(wire::ModelRole::User) => Role::User,
            Ok(wire::ModelRole::Assistant) => Role::Assistant,
            Ok(wire::ModelRole::Unspecified) | Err(_) => {
                return Err(invalid("a message's role is unset"));
            }
        };
        Ok(Self {
            role,
            blocks: blocks_from_wire(message.blocks)?,
        })
    }
}

impl ModelRequest {
    /// On the wire, for `scope`.
    ///
    /// # Errors
    /// As [`Block::to_wire`].
    pub fn to_wire(self, scope: wire::Scope) -> Result<wire::GenerateRequest, ModelFailure> {
        let messages = self
            .messages
            .into_iter()
            .map(ModelMessage::to_wire)
            .collect::<Result<_, ModelFailure>>()?;
        Ok(wire::GenerateRequest {
            scope: Some(scope),
            model: self.model,
            system: self.system,
            messages,
            tools: self
                .tools
                .into_iter()
                .map(|t| wire::ModelTool {
                    name: t.name,
                    description: t.description,
                    input_schema: Some(json::object_to_wire(t.input_schema)),
                })
                .collect(),
            max_tokens: self.max_tokens,
            tool_choice: match self.tool_choice {
                ToolChoice::Auto => String::new(),
                ToolChoice::Tool(name) => name,
            },
        })
    }

    /// From the wire (its scope apart).
    ///
    /// # Errors
    /// `Invalid` for a message whose role or a block is unset.
    pub fn from_wire(request: wire::GenerateRequest) -> Result<Self, ModelFailure> {
        let messages = request
            .messages
            .into_iter()
            .map(ModelMessage::from_wire)
            .collect::<Result<_, ModelFailure>>()?;
        Ok(Self {
            model: request.model,
            system: request.system,
            messages,
            tools: request
                .tools
                .into_iter()
                .map(|t| {
                    let schema = t
                        .input_schema
                        .ok_or_else(|| invalid(format!("tool {} has no input schema", t.name)))?;
                    Ok(ModelTool {
                        name: t.name,
                        description: t.description,
                        input_schema: json::object_from_wire(schema),
                    })
                })
                .collect::<Result<_, ModelFailure>>()?,
            max_tokens: request.max_tokens,
            tool_choice: if request.tool_choice.is_empty() {
                ToolChoice::Auto
            } else {
                ToolChoice::Tool(request.tool_choice)
            },
        })
    }
}

impl ModelFailure {
    /// On the wire.
    #[must_use]
    pub fn to_wire(self) -> wire::ModelFailure {
        use wire::ModelFailureKind as K;
        let kind = match self.kind {
            ModelFailureKind::RateLimited => K::RateLimited,
            ModelFailureKind::Overloaded => K::Overloaded,
            ModelFailureKind::ContextTooLong => K::ContextTooLong,
            ModelFailureKind::Unauthorized => K::Unauthorized,
            ModelFailureKind::QuotaExhausted => K::QuotaExhausted,
            ModelFailureKind::Unavailable => K::Unavailable,
            ModelFailureKind::Invalid => K::Invalid,
        };
        wire::ModelFailure {
            kind: kind.into(),
            message: self.message,
            retry_after_ms: self
                .retry_after
                .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX)),
        }
    }

    /// From the wire; a kind it does not know is `Invalid`, its message
    /// kept.
    #[must_use]
    pub fn from_wire(failure: wire::ModelFailure) -> Self {
        use wire::ModelFailureKind as K;
        let (kind, message) = match K::try_from(failure.kind) {
            Ok(K::RateLimited) => (ModelFailureKind::RateLimited, failure.message),
            Ok(K::Overloaded) => (ModelFailureKind::Overloaded, failure.message),
            Ok(K::ContextTooLong) => (ModelFailureKind::ContextTooLong, failure.message),
            Ok(K::Unauthorized) => (ModelFailureKind::Unauthorized, failure.message),
            Ok(K::QuotaExhausted) => (ModelFailureKind::QuotaExhausted, failure.message),
            Ok(K::Unavailable) => (ModelFailureKind::Unavailable, failure.message),
            Ok(K::Invalid) => (ModelFailureKind::Invalid, failure.message),
            Ok(K::Unspecified) | Err(_) => (
                ModelFailureKind::Invalid,
                format!("a failure of an unknown kind: {}", failure.message),
            ),
        };
        Self {
            kind,
            message,
            retry_after: failure.retry_after_ms.map(Duration::from_millis),
        }
    }
}

impl ModelEvent {
    /// On the wire.
    ///
    /// # Errors
    /// As [`Block::to_wire`].
    pub fn to_wire(self) -> Result<wire::GenerateReply, ModelFailure> {
        use wire::generate_reply::Event as E;
        let event = match self {
            Self::TextDelta(text) => E::TextDelta(text),
            Self::ThinkingDelta(text) => E::ThinkingDelta(text),
            Self::ToolUse { id, name, input } => E::ToolUse(blocks::ToolUseBlock {
                input: Some(input_to_wire(&id, input)?),
                id,
                name,
            }),
            Self::Done {
                blocks,
                usage,
                stop,
            } => E::Done(wire::ModelDone {
                blocks: blocks_to_wire(blocks)?,
                usage: Some(wire::ModelUsage {
                    input: usage.input,
                    output: usage.output,
                    cache_read: usage.cache_read,
                    cache_write: usage.cache_write,
                }),
                stop: match stop {
                    StopReason::EndTurn => wire::StopReason::EndTurn,
                    StopReason::MaxTokens => wire::StopReason::MaxTokens,
                    StopReason::ToolUse => wire::StopReason::ToolUse,
                }
                .into(),
            }),
        };
        Ok(wire::GenerateReply { event: Some(event) })
    }

    /// From the wire: the event, or the failure the stream ends with.
    ///
    /// # Errors
    /// The failure it carries; `Invalid` for an event that is unset or out
    /// of shape.
    pub fn from_wire(reply: wire::GenerateReply) -> Result<Self, ModelFailure> {
        use wire::generate_reply::Event as E;
        Ok(
            match reply.event.ok_or_else(|| invalid("an event is unset"))? {
                E::TextDelta(text) => Self::TextDelta(text),
                E::ThinkingDelta(text) => Self::ThinkingDelta(text),
                E::ToolUse(t) => Self::ToolUse {
                    input: input_from_wire(&t.id, t.input)?,
                    id: t.id,
                    name: t.name,
                },
                E::Done(done) => {
                    let usage = done
                        .usage
                        .ok_or_else(|| invalid("an answer has no usage"))?;
                    Self::Done {
                        blocks: blocks_from_wire(done.blocks)?,
                        usage: Usage {
                            input: usage.input,
                            output: usage.output,
                            cache_read: usage.cache_read,
                            cache_write: usage.cache_write,
                        },
                        stop: match wire::StopReason::try_from(done.stop) {
                            Ok(wire::StopReason::EndTurn) => StopReason::EndTurn,
                            Ok(wire::StopReason::MaxTokens) => StopReason::MaxTokens,
                            Ok(wire::StopReason::ToolUse) => StopReason::ToolUse,
                            Ok(wire::StopReason::Unspecified) | Err(_) => {
                                return Err(invalid("an answer's stop reason is unset"));
                            }
                        },
                    }
                }
                E::Failure(failure) => return Err(ModelFailure::from_wire(failure)),
            },
        )
    }
}
