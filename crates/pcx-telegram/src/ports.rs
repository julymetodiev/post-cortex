// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! What the Telegram channel needs of the world (ARCHITECTURE section 8,
//! operations): the Bot API, the board (a decision, approving it as a
//! person, a person's comment), and the chief, to talk with. The module's
//! adapters fill them; tests fill them in memory.

use async_trait::async_trait;
use pcx_module_api::ModuleError;
use post_cortex_proto::board::DecisionRecord;
use post_cortex_proto::telegram::{Button, Update};

/// The Bot API.
#[async_trait]
pub trait Bot: Send + Sync {
    /// The updates from `offset` on, waiting up to `timeout` seconds.
    ///
    /// # Errors
    /// The API refused or could not be reached.
    async fn updates(&self, offset: i64, timeout: u32) -> Result<Vec<Update>, ModuleError>;

    /// Send `text` to chat `chat` with `buttons` (one row): its id.
    ///
    /// # Errors
    /// As [`Bot::updates`].
    async fn send(&self, chat: i64, text: &str, buttons: &[Button]) -> Result<i64, ModuleError>;

    /// Replace the text of message `message` of chat `chat`, with
    /// `buttons` (one row; none: those it had go).
    ///
    /// # Errors
    /// As [`Bot::updates`].
    async fn edit(
        &self,
        chat: i64,
        message: i64,
        text: &str,
        buttons: &[Button],
    ) -> Result<(), ModuleError>;

    /// Answer button press `query` with `text`, shown to who pressed it.
    ///
    /// # Errors
    /// As [`Bot::updates`].
    async fn answer(&self, query: &str, text: &str) -> Result<(), ModuleError>;
}

/// A decision as the board answers it, with its task's title.
#[derive(Debug, Clone, PartialEq)]
pub struct Waiting {
    /// The decision.
    pub decision: DecisionRecord,
    /// Its task's title.
    pub task: String,
}

/// The board, for the channel.
#[async_trait]
pub trait Board: Send + Sync {
    /// Decision `id` of `workspace` at its latest version, with its task;
    /// none when it is gone.
    ///
    /// # Errors
    /// The board or memory failed.
    async fn decision(&self, workspace: &str, id: &str) -> Result<Option<Waiting>, ModuleError>;

    /// Approve decision `id` of `workspace` from its version `version`, as
    /// `person`.
    ///
    /// # Errors
    /// The board refused (it waits no longer, it changed) or failed.
    async fn approve(
        &self,
        workspace: &str,
        person: &str,
        id: &str,
        version: u64,
    ) -> Result<(), ModuleError>;

    /// Decline decision `id` of `workspace` from its version `version`, as
    /// `person`.
    ///
    /// # Errors
    /// The board refused (it waits no longer, it changed) or failed.
    async fn decline(
        &self,
        workspace: &str,
        person: &str,
        id: &str,
        version: u64,
    ) -> Result<(), ModuleError>;
}

/// The chief, for a person who writes to the bot.
#[async_trait]
pub trait Chief: Send + Sync {
    /// Ask the chief `question` as `person`, going on with `conversation`
    /// (empty: a new one): its answer and the conversation.
    ///
    /// # Errors
    /// The chief refused or failed.
    async fn ask(
        &self,
        person: &str,
        question: &str,
        conversation: &str,
    ) -> Result<(String, String), ModuleError>;
}
