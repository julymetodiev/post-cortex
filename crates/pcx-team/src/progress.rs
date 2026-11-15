// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! How far a member's turn has come, while it runs: what a person needs to
//! tell a slow model from a stuck one (docs/modules/team.md, every step on the
//! task).

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use pcx_module_api::agent::TurnEvent;

/// A turn that runs now, as a person sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Busy {
    /// Its task.
    pub task: String,
    /// Its member.
    pub member: String,
    /// The seconds since it last received or did anything.
    pub quiet_seconds: u64,
    /// What its model sent so far, in characters.
    pub received: u64,
}

/// What a running turn has done so far.
#[derive(Debug)]
pub struct Progress {
    last: Mutex<Instant>,
    received: AtomicU64,
}

impl Default for Progress {
    fn default() -> Self {
        Self {
            last: Mutex::new(Instant::now()),
            received: AtomicU64::new(0),
        }
    }
}

impl Progress {
    /// Take in `event` of the turn.
    pub fn see(&self, event: &TurnEvent) {
        *self.last.lock() = Instant::now();
        let got = match event {
            TurnEvent::Text(text) | TurnEvent::Thinking(text) => text.chars().count(),
            TurnEvent::ToolCall { input, .. } => input.to_string().chars().count(),
            TurnEvent::ToolResult { .. } | TurnEvent::Retrying { .. } => 0,
        };
        self.received.fetch_add(got as u64, Ordering::Relaxed);
    }

    /// How long since anything came.
    #[must_use]
    pub fn quiet(&self) -> Duration {
        self.last.lock().elapsed()
    }

    /// What its model sent so far, in characters.
    #[must_use]
    pub fn received(&self) -> u64 {
        self.received.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_counts_what_the_model_sent_and_a_tool_answer_is_not_that() {
        let progress = Progress::default();
        progress.see(&TurnEvent::Text("héllo".to_string()));
        progress.see(&TurnEvent::Thinking("abc".to_string()));
        progress.see(&TurnEvent::ToolResult {
            id: "1".to_string(),
            content: "a long answer".to_string(),
            is_error: false,
        });
        assert_eq!(progress.received(), 8);
        assert!(progress.quiet() < Duration::from_secs(5));
    }
}
