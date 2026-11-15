// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! What a member's turn did, as its task shows it when the turn gave no
//! answer (docs/modules/team.md, a turn without an answer): its last
//! steps, a tool's call each (`✗` when it failed), and the comment that
//! says how it ended. Pure.

use std::collections::VecDeque;

use pcx_module_api::agent::{TurnEnd, TurnEvent, TurnStop};
use serde_json::Value;

/// The steps a comment shows.
const SHOWN: usize = 5;

/// The most characters of a call's input or a failure a step shows.
const WIDTH: usize = 120;

/// A turn's last tool calls, as they come.
#[derive(Debug, Default)]
pub struct Steps {
    /// Each call's id and its line, the oldest first.
    calls: VecDeque<(String, String)>,
}

impl Steps {
    /// Take in `event`: a call is a step; a failed result marks its call.
    pub fn see(&mut self, event: &TurnEvent) {
        match event {
            TurnEvent::ToolCall { id, name, input } => {
                if self.calls.len() == SHOWN {
                    self.calls.pop_front();
                }
                self.calls
                    .push_back((id.clone(), format!("→ {name} {}", short(&text(input)))));
            }
            TurnEvent::ToolResult {
                id,
                content,
                is_error: true,
            } => {
                if let Some((_, line)) = self.calls.iter_mut().find(|(call, _)| call == id) {
                    line.push_str(&format!(" ✗ {}", short(content)));
                }
            }
            TurnEvent::ToolResult { .. }
            | TurnEvent::Text(_)
            | TurnEvent::Thinking(_)
            | TurnEvent::Retrying { .. } => {}
        }
    }

    /// The steps seen last, the oldest first.
    #[must_use]
    pub fn last(&self) -> Vec<String> {
        self.calls.iter().map(|(_, line)| line.clone()).collect()
    }
}

/// Whether a turn that ended as `end` gave no answer for its task: it took
/// its most steps, or the model ended with no text.
#[must_use]
pub fn unanswered(end: &TurnEnd) -> bool {
    matches!(
        end.stop,
        TurnStop::StepLimit | TurnStop::Repeating(_) | TurnStop::Answered(_)
    ) && end.answer.trim().is_empty()
}

/// The comment of `member` whose turn ended as `end` without an answer:
/// how it ended, its `last` steps and the `session` that keeps them all.
#[must_use]
pub fn said(member: &str, end: &TurnEnd, last: &[String], session: &str) -> String {
    let how = match end.stop {
        TurnStop::StepLimit => format!(
            "{member} stopped after {} steps, the most a turn may take, without an answer.",
            end.steps
        ),
        TurnStop::Repeating(ref what) => format!(
            "{member} stopped after {} steps: its tools kept failing, last with: {what}",
            end.steps
        ),
        _ => format!(
            "{member}'s turn ended ({}) after {} steps without an answer.",
            end.stop.name(),
            end.steps
        ),
    };
    let steps = if last.is_empty() {
        "It called no tool.".to_string()
    } else {
        let lines: Vec<String> = last.iter().map(|l| format!("- `{l}`")).collect();
        format!("Its last steps:\n{}", lines.join("\n"))
    };
    format!(
        "{how}\n\n{steps}\n\nEvery step is in its session `{session}` (Conversations). Answer this comment for {member} to go on."
    )
}

/// `value` as one line of text: a string as it is, anything else as JSON.
fn text(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// `text` on one line, cut to [`WIDTH`] characters, with no backtick (a
/// step is shown as code).
fn short(text: &str) -> String {
    let line = text
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .replace('`', "'");
    if line.chars().count() <= WIDTH {
        return line;
    }
    let cut: String = line.chars().take(WIDTH).collect();
    format!("{cut}…")
}

#[cfg(test)]
mod tests;
