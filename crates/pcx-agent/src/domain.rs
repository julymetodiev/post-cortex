// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The agent's domain (ARCHITECTURE section 7, the agent): a turn as the
//! transcript keeps it, the conversation a model is sent (the chain of
//! parents back from the newest turn), what follows a model's answer, and
//! what follows a failure. Pure: no host, no model, no clock.

use std::time::Duration;

use pcx_module_api::agent::TurnStop;
use pcx_module_api::model::{
    Block, ModelFailure, ModelFailureKind, ModelMessage, ModelTool, Role, StopReason,
};
use serde_json::Value;

mod window;

pub use window::{
    KEEP_RECENT_TOKENS, MAX_COMPACTION_DEFERRALS, carries, context_block, estimate, over,
    prompt_turn, shown, to_compact,
};

/// The most times a failure that may pass is retried.
pub const MAX_RETRIES: u32 = 3;

/// The first wait before a retry; it doubles each time.
pub const FIRST_BACKOFF: Duration = Duration::from_secs(1);

/// The longest wait before a retry.
pub const MAX_BACKOFF: Duration = Duration::from_secs(30);

/// The agent's module id, and who its turns are written as when a turn
/// names no speaker (W25).
pub const AGENT: &str = "agent";

/// A turn of the transcript.
#[derive(Debug, Clone, PartialEq)]
pub struct Turn {
    /// Its id, unique in the session.
    pub turn_id: String,
    /// Who speaks it.
    pub role: Role,
    /// What search reads: its text blocks joined.
    pub text: String,
    /// The turn it follows; none: the one before it in the stream.
    pub parent: Option<String>,
    /// The turn whole; empty for a turn captured as text only.
    pub blocks: Vec<Block>,
    /// Who wrote it (W25): the person who asked a prompt, the agent's
    /// speaker the rest; none: not known (a turn written before W25).
    pub author: Option<String>,
}

impl Turn {
    /// A turn of `role` with `blocks`, following `parent`, by no one known.
    #[must_use]
    pub fn new(turn_id: String, role: Role, parent: Option<String>, blocks: Vec<Block>) -> Self {
        Self {
            text: text_of(&blocks),
            turn_id,
            role,
            parent,
            blocks,
            author: None,
        }
    }

    /// It, written by `author`.
    #[must_use]
    pub fn by(self, author: Option<String>) -> Self {
        Self { author, ..self }
    }

    /// The tools it calls, by name.
    #[must_use]
    pub fn tools(&self) -> Vec<String> {
        self.blocks
            .iter()
            .filter_map(|b| match b {
                Block::ToolUse { name, .. } => Some(name.clone()),
                _ => None,
            })
            .collect()
    }
}

/// The text blocks of `blocks`, joined.
#[must_use]
pub fn text_of(blocks: &[Block]) -> String {
    blocks
        .iter()
        .filter_map(|b| match b {
            Block::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// What a summary says to the model before its text.
pub const SUMMARY_HEAD: &str = "Summary of the conversation so far:";

impl Turn {
    /// Whether it is a compaction: a summary of the turns before it.
    #[must_use]
    pub fn summarizes(&self) -> bool {
        self.blocks
            .iter()
            .any(|b| matches!(b, Block::Summary { .. }))
    }

    /// Whether it is the end of a turn (W25): kept in the transcript, never
    /// sent to a model.
    #[must_use]
    pub fn ends(&self) -> bool {
        !self.blocks.is_empty() && self.blocks.iter().all(|b| matches!(b, Block::End { .. }))
    }
}

/// The end of a turn that stopped as `stop`, as the transcript keeps it.
#[must_use]
pub fn end_block(stop: &TurnStop) -> Block {
    Block::End {
        stop: stop.name().to_string(),
        detail: stop.detail(),
    }
}

/// What a tool call the system stopped under answers.
const UNANSWERED: &str = "The system stopped during this call: it may or may not have finished. Look before you repeat it.";

/// The same failure of a tool in a row at which a turn stops.
const SAME_FAILURES: u32 = 3;

/// The failures of its tools at which a turn stops.
const ALL_FAILURES: u32 = 12;

/// How much of a failure tells it from another.
const FAILURE_SAME_BYTES: usize = 200;

/// A turn's failed tool calls, to stop a model going in circles: it reads each
/// refusal, but only this count can end what it does not see itself doing.
#[derive(Debug, Default)]
pub struct Failures {
    last: String,
    same: u32,
    all: u32,
}

impl Failures {
    /// Take in a step's `results`; the last failure when the turn should stop.
    pub fn see(&mut self, results: &[Block]) -> Option<String> {
        for result in results {
            let Block::ToolResult {
                content,
                is_error: true,
                ..
            } = result
            else {
                continue;
            };
            let start: String = content.chars().take(FAILURE_SAME_BYTES).collect();
            self.same = if start == self.last { self.same + 1 } else { 1 };
            self.last = start;
            self.all += 1;
            if self.same >= SAME_FAILURES || self.all >= ALL_FAILURES {
                return Some(self.last.clone());
            }
        }
        None
    }
}

/// The turns a model is sent: the newest turn and the chain of turns it
/// follows (its parent, else the turn before it), back to the newest
/// summary, oldest first.
#[must_use]
pub fn chain(turns: &[Turn]) -> Vec<&Turn> {
    let mut chain = Vec::new();
    let mut at = turns.len().checked_sub(1);
    while let Some(index) = at {
        let turn = &turns[index];
        chain.push(turn);
        if turn.summarizes() {
            break;
        }
        at = match &turn.parent {
            Some(parent) => turns[..index].iter().rposition(|t| &t.turn_id == parent),
            None => index.checked_sub(1),
        };
    }
    chain.reverse();
    chain
}

/// The conversation a model is sent: [`chain`] as messages, without the
/// ends of turns. A turn kept as text only is one text block; a summary is
/// text that says it is one.
#[must_use]
pub fn conversation(turns: &[Turn]) -> Vec<ModelMessage> {
    messages(&chain(turns))
}

/// `chain` as messages.
fn messages(chain: &[&Turn]) -> Vec<ModelMessage> {
    let sent: Vec<ModelMessage> = chain
        .iter()
        .filter(|turn| !turn.ends())
        .map(|turn| ModelMessage {
            role: turn.role,
            blocks: if turn.blocks.is_empty() {
                vec![Block::Text {
                    text: turn.text.clone(),
                }]
            } else {
                turn.blocks
                    .iter()
                    .map(|b| match b {
                        Block::Summary { text } => Block::Text {
                            text: format!("{SUMMARY_HEAD}\n{text}"),
                        },
                        other => other.clone(),
                    })
                    .collect()
            },
        })
        .collect();
    answered(sent)
}

/// `messages` with each tool call answered: a provider refuses a conversation
/// with a call no result follows, which is what a stop in the middle of a call
/// leaves.
fn answered(messages: Vec<ModelMessage>) -> Vec<ModelMessage> {
    let mut sent: Vec<ModelMessage> = Vec::with_capacity(messages.len());
    for mut message in messages {
        let missing = sent
            .last()
            .map_or_else(Vec::new, |last| missing(last, &message));
        if !missing.is_empty() {
            if message.role == Role::User {
                message.blocks.splice(0..0, missing);
            } else {
                sent.push(ModelMessage {
                    role: Role::User,
                    blocks: missing,
                });
            }
        }
        sent.push(message);
    }
    let last = sent.last().map(|last| {
        missing(
            last,
            &ModelMessage {
                role: Role::User,
                blocks: Vec::new(),
            },
        )
    });
    if let Some(blocks) = last.filter(|blocks| !blocks.is_empty()) {
        sent.push(ModelMessage {
            role: Role::User,
            blocks,
        });
    }
    sent
}

/// The results `next` lacks of the tool calls of `message`.
fn missing(message: &ModelMessage, next: &ModelMessage) -> Vec<Block> {
    message
        .blocks
        .iter()
        .filter_map(|block| match block {
            Block::ToolUse { id, .. }
                if !next.blocks.iter().any(
                    |b| matches!(b, Block::ToolResult { tool_use_id, .. } if tool_use_id == id),
                ) =>
            {
                Some(Block::ToolResult {
                    tool_use_id: id.clone(),
                    content: UNANSWERED.to_string(),
                    is_error: true,
                })
            }
            _ => None,
        })
        .collect()
}

/// The tools a turn may use of `tools` (each with its module): those of
/// `modules` (empty: every module), then of `names` (empty: all of them),
/// but none of `withheld`; by name, so every request lists them alike and
/// the provider's cache serves them.
#[must_use]
pub fn allowed(
    tools: Vec<(String, ModelTool)>,
    modules: &[String],
    names: &[String],
    withheld: &[String],
) -> Vec<ModelTool> {
    let mut given: Vec<ModelTool> = tools
        .into_iter()
        .filter(|(module, _)| modules.is_empty() || modules.contains(module))
        .filter(|(_, tool)| names.is_empty() || names.contains(&tool.name))
        .filter(|(_, tool)| !withheld.contains(&tool.name))
        .map(|(_, tool)| tool)
        .collect();
    given.sort_by(|a, b| a.name.cmp(&b.name));
    given
}

/// A tool call of an answer.
#[derive(Debug, Clone, PartialEq)]
pub struct Call {
    /// Its id.
    pub id: String,
    /// The tool.
    pub name: String,
    /// Its input.
    pub input: Value,
}

/// What follows a model's answer.
#[derive(Debug, Clone, PartialEq)]
pub enum Next {
    /// Run these tools, then ask the model again with their results.
    Tools(Vec<Call>),
    /// The turn ends.
    End(StopReason),
}

/// What follows the answer `blocks` that stopped for `stop`: its tool
/// calls when it called tools, else the end.
#[must_use]
pub fn next(blocks: &[Block], stop: StopReason) -> Next {
    let calls: Vec<Call> = blocks
        .iter()
        .filter_map(|b| match b {
            Block::ToolUse { id, name, input } => Some(Call {
                id: id.clone(),
                name: name.clone(),
                input: input.clone(),
            }),
            _ => None,
        })
        .collect();
    if stop == StopReason::ToolUse && !calls.is_empty() {
        Next::Tools(calls)
    } else {
        Next::End(stop)
    }
}

/// How many times one turn goes on after an answer the most tokens cut.
pub const MAX_CUTS: u32 = 2;

/// What the model is told after an answer the most tokens cut.
pub const CUT_NOTE: &str = "Your last answer went past the most tokens it may have and was cut. \
A tool call in it was not run. Go on in smaller steps: split long text, and send a large tool \
input in several smaller calls.";

/// An answer the most tokens cut, as the conversation keeps it: a model is
/// never sent an answer with neither text nor a tool call.
#[must_use]
pub fn cut_answer(mut blocks: Vec<Block>) -> Vec<Block> {
    if !blocks
        .iter()
        .any(|b| matches!(b, Block::Text { .. } | Block::ToolUse { .. }))
    {
        blocks.push(Block::Text {
            text: "(cut by the most tokens)".to_string(),
        });
    }
    blocks
}

/// What follows a failed model call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Recover {
    /// Ask again after this wait.
    Retry(Duration),
    /// The conversation is longer than the model's context: compact it,
    /// then ask again.
    Compact,
    /// The turn ends with the failure.
    Stop,
}

/// The failures of one model call so far.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Recovery {
    retries: u32,
    compacted: bool,
}

impl Recovery {
    /// What follows `failure`, counted: a rate limit, an overload or an
    /// unreachable provider is retried with backoff (or after the
    /// provider's own wait) up to [`MAX_RETRIES`] times; a context too long
    /// is compacted once; anything else, or past those, stops.
    pub fn after(&mut self, failure: &ModelFailure) -> Recover {
        match failure.kind {
            ModelFailureKind::RateLimited
            | ModelFailureKind::Overloaded
            | ModelFailureKind::Unavailable
                if self.retries < MAX_RETRIES =>
            {
                let backoff = FIRST_BACKOFF
                    .saturating_mul(1 << self.retries)
                    .min(MAX_BACKOFF);
                self.retries += 1;
                Recover::Retry(failure.retry_after.unwrap_or(backoff).min(MAX_BACKOFF))
            }
            ModelFailureKind::ContextTooLong if !self.compacted => {
                self.compacted = true;
                Recover::Compact
            }
            _ => Recover::Stop,
        }
    }
}

#[cfg(test)]
mod tests;
