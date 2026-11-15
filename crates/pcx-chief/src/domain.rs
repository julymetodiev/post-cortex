// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The chief's domain (ARCHITECTURE section 8, the chief): its
//! conversations, the record of each tool call of its turns, read off the
//! turn's events, and its daily budget. Pure: no host, no memory, no clock.

use std::fmt;

use pcx_module_api::agent::TurnEvent;
use pcx_module_api::model::Usage;
use serde_json::Value;

/// The module's id, and the prefix of its document kinds.
pub const MODULE_ID: &str = "chief";

/// The kinds of documents the chief keeps, `chief/<name>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A conversation: who started it, from where, its session.
    Talk,
    /// One tool call of a turn.
    Action,
    /// A day's use of the budget.
    Day,
}

impl Kind {
    /// Its name in the module.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Talk => "talk",
            Self::Action => "action",
            Self::Day => "day",
        }
    }

    /// Its full name, `chief/<name>`, as memory keeps it.
    #[must_use]
    pub fn document(self) -> String {
        format!("{MODULE_ID}/{}", self.name())
    }

    /// Every kind.
    pub const ALL: [Self; 3] = [Self::Talk, Self::Action, Self::Day];
}

pub use post_cortex_proto::chief::{Action, Day, Talk};

/// The rules of a conversation (its message is the chief's package's).
pub trait TalkRules {
    /// Refuse `asker`, from `channel`, going on with it: only who started
    /// it, from the same channel, does.
    ///
    /// # Errors
    /// [`Refused::NotYours`].
    fn continued_by(&self, asker: &str, channel: &str) -> Result<(), Refused>;
}

impl TalkRules for Talk {
    fn continued_by(&self, asker: &str, channel: &str) -> Result<(), Refused> {
        if self.asked_by == asker && self.channel == channel {
            Ok(())
        } else {
            Err(Refused::NotYours)
        }
    }
}

/// Why the chief refuses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refused {
    /// The day's budget is spent.
    Spent {
        /// The tokens used today.
        used: u64,
        /// The budget.
        budget: u64,
    },
    /// The conversation is someone else's, or from another channel.
    NotYours,
}

impl fmt::Display for Refused {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Spent { used, budget } => write!(
                f,
                "the chief used {used} of its {budget} tokens today: no more turns until tomorrow (UTC)"
            ),
            Self::NotYours => write!(
                f,
                "the conversation was started by someone else or from another channel"
            ),
        }
    }
}

/// The tokens a turn's `usage` counts against the budget: its input, its
/// output and its cache writes (not its cache reads).
#[must_use]
pub fn spent(usage: &Usage) -> u64 {
    usage
        .input
        .saturating_add(usage.output)
        .saturating_add(usage.cache_write)
}

/// Refuse a turn when `used` tokens of `budget` are spent.
///
/// # Errors
/// [`Refused::Spent`].
pub fn within(used: u64, budget: u64) -> Result<(), Refused> {
    if used < budget {
        Ok(())
    } else {
        Err(Refused::Spent { used, budget })
    }
}

/// A tool call, as a turn's events tell it.
#[derive(Debug, Clone, PartialEq)]
pub struct Called {
    /// The tool.
    pub tool: String,
    /// Its input.
    pub input: Value,
    /// Whether it failed; none: no result came.
    pub failed: Option<bool>,
}

impl Called {
    /// Its record in conversation `talk`, asked by `asked_by`: its input
    /// the object of arguments the model gave, by the canonical proto JSON
    /// mapping.
    ///
    /// # Errors
    /// The input is not an object.
    pub fn action(self, talk: &str, asked_by: &str) -> Result<Action, serde_json::Error> {
        Ok(Action {
            talk: talk.to_string(),
            asked_by: asked_by.to_string(),
            tool: self.tool,
            input: Some(serde_json::from_value(self.input)?),
            failed: self.failed,
        })
    }
}

/// The calls of a turn whose result has not come yet.
#[derive(Debug, Default)]
pub struct Calls {
    open: Vec<(String, Called)>,
}

impl Calls {
    /// Take `event`: a call that its result answers now.
    pub fn take(&mut self, event: TurnEvent) -> Option<Called> {
        match event {
            TurnEvent::ToolCall { id, name, input } => {
                self.open.push((
                    id,
                    Called {
                        tool: name,
                        input,
                        failed: None,
                    },
                ));
                None
            }
            TurnEvent::ToolResult { id, is_error, .. } => {
                let at = self.open.iter().position(|(open, _)| *open == id)?;
                let (_, mut called) = self.open.remove(at);
                called.failed = Some(is_error);
                Some(called)
            }
            TurnEvent::Text(_) | TurnEvent::Thinking(_) | TurnEvent::Retrying { .. } => None,
        }
    }

    /// The calls no result answered, in the order they were made.
    #[must_use]
    pub fn unanswered(self) -> Vec<Called> {
        self.open.into_iter().map(|(_, called)| called).collect()
    }
}

#[cfg(test)]
mod tests;
