// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! A turn on the wire (`pcx.agent.v1`): its request, its events and its
//! end, both ways, so every server and client of turns maps them alike.

use std::time::Duration;

use post_cortex_proto::agent as wire;
use serde_json::Value;

use crate::agent::{TurnEnd, TurnEvent, TurnRequest, TurnStop};
use crate::model::{ModelFailure, StopReason, Usage};
use crate::{ModuleError, ModuleErrorKind, json};

impl TurnRequest {
    /// On the wire, in `scope` (the call's, between a host and a module).
    #[must_use]
    pub fn to_wire(&self, scope: Option<post_cortex_proto::module::Scope>) -> wire::RunTurnRequest {
        wire::RunTurnRequest {
            session_id: self.session.clone(),
            prompt: self.prompt.clone(),
            scope,
            instructions: self.instructions.clone(),
            model: self.model.clone().unwrap_or_default(),
            modules: self.modules.clone(),
            tools: self.tools.clone(),
            withheld: self.withheld.clone(),
            agent: String::new(),
            member: self.member.clone().unwrap_or_default(),
            speaker: self.speaker.clone().unwrap_or_default(),
            asker: self.asker.clone().unwrap_or_default(),
            workspace: self.workspace.clone().unwrap_or_default(),
        }
    }

    /// From the wire (its scope is the server's to read).
    #[must_use]
    pub fn from_wire(request: wire::RunTurnRequest) -> Self {
        Self {
            session: request.session_id,
            prompt: request.prompt,
            instructions: request.instructions,
            model: Some(request.model).filter(|m| !m.is_empty()),
            modules: request.modules,
            tools: request.tools,
            withheld: request.withheld,
            member: Some(request.member).filter(|m| !m.is_empty()),
            speaker: Some(request.speaker).filter(|s| !s.is_empty()),
            asker: Some(request.asker).filter(|a| !a.is_empty()),
            workspace: Some(request.workspace).filter(|w| !w.is_empty()),
        }
    }
}

fn reply(event: wire::turn_reply::Event) -> wire::TurnReply {
    wire::TurnReply { event: Some(event) }
}

/// The first reply of the turn `turn_id`.
#[must_use]
pub fn started(turn_id: String) -> wire::TurnReply {
    reply(wire::turn_reply::Event::Started(wire::TurnStarted {
        turn_id,
    }))
}

/// A duration in whole milliseconds, the longest one saturating.
fn millis(d: Duration) -> u64 {
    u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
}

impl TurnEvent {
    /// On the wire.
    #[must_use]
    pub fn to_wire(self) -> wire::TurnReply {
        use wire::turn_reply::Event as E;
        reply(match self {
            Self::Text(text) => E::Text(text),
            Self::Thinking(text) => E::Thinking(text),
            Self::ToolCall { id, name, input } => E::ToolCall(wire::ToolCall {
                id,
                name,
                input: match input {
                    Value::Object(fields) => Some(json::object_to_wire(fields)),
                    _ => None,
                },
            }),
            Self::ToolResult {
                id,
                content,
                is_error,
            } => E::ToolResult(wire::ToolResult {
                id,
                content,
                is_error,
            }),
            Self::Retrying { failure, after } => E::Retrying(wire::Retrying {
                kind: failure.kind.name().to_string(),
                message: failure.message.clone(),
                after_ms: millis(after),
                failure: Some(failure.to_wire()),
            }),
        })
    }
}

impl TurnEnd {
    /// On the wire, as a turn's last event.
    #[must_use]
    pub fn to_wire(self) -> wire::TurnReply {
        reply(wire::turn_reply::Event::End(self.end_wire()))
    }

    /// On the wire, the end alone (a module's answer that carries it).
    #[must_use]
    pub fn end_wire(self) -> wire::TurnEnd {
        let (stop, detail) = (self.stop.name(), self.stop.detail());
        let failure = match self.stop {
            TurnStop::Failed(failure) => Some(failure.to_wire()),
            _ => None,
        };
        let Usage {
            input,
            output,
            cache_read,
            cache_write,
        } = self.usage;
        wire::TurnEnd {
            stop: stop.to_string(),
            detail,
            steps: self.steps,
            answer: self.answer,
            usage: Some(wire::TurnUsage {
                input,
                output,
                cache_read,
                cache_write,
            }),
            failure,
        }
    }
}

/// A reply that does not read.
fn unreadable(reason: impl Into<String>) -> ModuleError {
    ModuleError::new(ModuleErrorKind::Internal, reason)
}

/// A turn's reply read: its start (first), an event, or its end (last).
pub enum Reply {
    /// The turn started, with its id on the server.
    Started(String),
    /// An event of the running turn.
    Event(TurnEvent),
    /// How it ended.
    End(TurnEnd),
}

impl Reply {
    /// From the wire.
    ///
    /// # Errors
    /// `Internal` for a reply without an event, a retry or a failed end
    /// without its failure, an end without its usage, or a stop it does
    /// not know.
    pub fn from_wire(reply: wire::TurnReply) -> Result<Self, ModuleError> {
        use wire::turn_reply::Event as E;
        Ok(match reply.event {
            Some(E::Text(text)) => Self::Event(TurnEvent::Text(text)),
            Some(E::Thinking(text)) => Self::Event(TurnEvent::Thinking(text)),
            Some(E::ToolCall(call)) => Self::Event(TurnEvent::ToolCall {
                id: call.id,
                name: call.name,
                input: call
                    .input
                    .map_or(Value::Null, |i| Value::Object(json::object_from_wire(i))),
            }),
            Some(E::ToolResult(result)) => Self::Event(TurnEvent::ToolResult {
                id: result.id,
                content: result.content,
                is_error: result.is_error,
            }),
            Some(E::Retrying(retrying)) => Self::Event(TurnEvent::Retrying {
                failure: retrying
                    .failure
                    .map(ModelFailure::from_wire)
                    .ok_or_else(|| unreadable("a retry without its failure"))?,
                after: Duration::from_millis(retrying.after_ms),
            }),
            Some(E::End(end)) => Self::End(end_from_wire(end)?),
            Some(E::Started(started)) => Self::Started(started.turn_id),
            None => return Err(unreadable("a turn's reply without an event")),
        })
    }
}

fn end_from_wire(end: wire::TurnEnd) -> Result<TurnEnd, ModuleError> {
    let stop = match end.stop.as_str() {
        "end_turn" => TurnStop::Answered(StopReason::EndTurn),
        "max_tokens" => TurnStop::Answered(StopReason::MaxTokens),
        "tool_use" => TurnStop::Answered(StopReason::ToolUse),
        "blocked" => TurnStop::Blocked(end.detail),
        "step_limit" => TurnStop::StepLimit,
        "repeating" => TurnStop::Repeating(end.detail),
        "cancelled" => TurnStop::Cancelled,
        "failed" => TurnStop::Failed(
            end.failure
                .map(ModelFailure::from_wire)
                .ok_or_else(|| unreadable("a failed turn without its failure"))?,
        ),
        other => return Err(unreadable(format!("a turn that stopped as {other}"))),
    };
    let usage = end
        .usage
        .ok_or_else(|| unreadable("a turn's end without its usage"))?;
    Ok(TurnEnd {
        stop,
        steps: end.steps,
        usage: Usage {
            input: usage.input,
            output: usage.output,
            cache_read: usage.cache_read,
            cache_write: usage.cache_write,
        },
        answer: end.answer,
    })
}

#[cfg(test)]
mod tests;
