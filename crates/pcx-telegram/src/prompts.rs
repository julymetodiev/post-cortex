// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! What the channel keeps while it runs: the prompts of decisions it sent
//! (a button names a prompt by number; the decision behind it is kept
//! here, one prompt a decision), the next update to read, and each chat's
//! conversation with the chief. Lost when the module stops: a button of an
//! older run answers that its prompt is gone (the app approves it still),
//! a chat starts a new conversation.

use std::collections::HashMap;

use parking_lot::Mutex;

/// A decision's prompt in a chat.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prompt {
    /// The decision's workspace.
    pub workspace: String,
    /// The decision.
    pub decision: String,
    /// Its version as its message shows it: an approval names it.
    pub version: u64,
    /// What its message says while it waits.
    pub text: String,
    /// What it decides.
    pub what: String,
    /// The chat it was sent to.
    pub chat: i64,
    /// Its message there.
    pub message: i64,
}

/// The channel's state.
#[derive(Default)]
pub struct Kept {
    prompts: Mutex<(u64, HashMap<u64, Prompt>)>,
    writes: tokio::sync::Mutex<()>,
    offset: Mutex<i64>,
    talks: Mutex<HashMap<i64, String>>,
}

impl Kept {
    /// A number for a new prompt.
    pub fn number(&self) -> u64 {
        let mut prompts = self.prompts.lock();
        prompts.0 += 1;
        prompts.0
    }

    /// Held while one write of a decision is acted on: the board writes a
    /// decision more than once as it asks, the writes are heard together,
    /// and each would read no prompt of it yet and send one.
    pub async fn one_write(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.writes.lock().await
    }

    /// Keep `prompt` as number `number` (again: as it is now).
    pub fn keep(&self, number: u64, prompt: Prompt) {
        self.prompts.lock().1.insert(number, prompt);
    }

    /// Prompt `number`; none when it is gone.
    pub fn prompt(&self, number: u64) -> Option<Prompt> {
        self.prompts.lock().1.get(&number).cloned()
    }

    /// Take the prompt of decision `decision` (it was settled).
    pub fn settle(&self, decision: &str) -> Option<Prompt> {
        let mut prompts = self.prompts.lock();
        let number = prompts
            .1
            .iter()
            .find(|(_, p)| p.decision == decision)
            .map(|(n, _)| *n)?;
        prompts.1.remove(&number)
    }

    /// The prompt of decision `decision` with its number; none when it has
    /// none (never sent, or settled).
    pub fn of(&self, decision: &str) -> Option<(u64, Prompt)> {
        self.prompts
            .lock()
            .1
            .iter()
            .find(|(_, p)| p.decision == decision)
            .map(|(n, p)| (*n, p.clone()))
    }

    /// The next update to read.
    pub fn offset(&self) -> i64 {
        *self.offset.lock()
    }

    /// Read from after update `id` next.
    pub fn read(&self, id: i64) {
        let mut offset = self.offset.lock();
        *offset = (*offset).max(id + 1);
    }

    /// Chat `chat`'s conversation with the chief; empty: none yet.
    pub fn talk(&self, chat: i64) -> String {
        self.talks.lock().get(&chat).cloned().unwrap_or_default()
    }

    /// Chat `chat` goes on with `conversation`.
    pub fn talking(&self, chat: i64, conversation: String) {
        self.talks.lock().insert(chat, conversation);
    }
}
