// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The Telegram channel's service (ARCHITECTURE section 8, operations,
//! W29): a message sent to a destination; a decision that waits for a
//! person sent to the approvals chat with its buttons, and its message
//! settled when someone approves it; a person's press approving or
//! declining it as that person; a person's message a question to the
//! chief. Between the handlers (and the module's poller and events) and
//! the ports.

use pcx_module_api::{ModuleError, ModuleErrorKind};
use post_cortex_proto::telegram::{TelegramConfig, Update};

use crate::domain::{self, Choice, Refused};
use crate::ports::{Board, Bot, Chief, Waiting};
use crate::prompts::{Kept, Prompt};

fn refused(rule: Refused) -> ModuleError {
    let kind = match rule {
        Refused::UnknownChat(_) => ModuleErrorKind::NotFound,
        Refused::Setting { .. } | Refused::Empty => ModuleErrorKind::InvalidArgument,
    };
    let field = match &rule {
        Refused::Setting { field, .. } => Some(field.clone()),
        Refused::UnknownChat(_) | Refused::Empty => None,
    };
    let error = ModuleError::new(kind, rule.to_string());
    match field {
        Some(field) => error.with_field(field),
        None => error,
    }
}

/// `config` checked before the module starts.
///
/// # Errors
/// `InvalidArgument` naming the first wrong setting.
pub fn checked(config: TelegramConfig) -> Result<TelegramConfig, ModuleError> {
    domain::check(&config).map_err(refused)?;
    Ok(config)
}

/// The channel over its ports, as `config` sets it, with what it keeps.
pub struct Service<'a> {
    config: &'a TelegramConfig,
    kept: &'a Kept,
    bot: &'a dyn Bot,
    board: &'a dyn Board,
    chief: &'a dyn Chief,
}

impl<'a> Service<'a> {
    /// The service.
    pub fn new(
        config: &'a TelegramConfig,
        kept: &'a Kept,
        bot: &'a dyn Bot,
        board: &'a dyn Board,
        chief: &'a dyn Chief,
    ) -> Self {
        Self {
            config,
            kept,
            bot,
            board,
            chief,
        }
    }

    /// Send `text` to the chat named `destination`: the message's id.
    ///
    /// # Errors
    /// `NotFound` for a chat it does not have; `InvalidArgument` for an
    /// empty text; the Bot API failed.
    pub async fn send(&self, destination: &str, text: &str) -> Result<String, ModuleError> {
        let chat = domain::chat(self.config, destination).map_err(refused)?;
        let text = domain::said(text).map_err(refused)?;
        Ok(self.bot.send(chat, text, &[]).await?.to_string())
    }

    /// Decision `id` of `workspace` was written: sent to the approvals
    /// chat while it waits for a person, one message a decision (written
    /// again while it waits, its message follows it: what it says, the
    /// version an approval names); its message settled once it waits no
    /// longer.
    ///
    /// # Errors
    /// The board or the Bot API failed.
    pub async fn decision_written(&self, workspace: &str, id: &str) -> Result<(), ModuleError> {
        if self.config.approvals.is_empty() {
            return Ok(());
        }
        let _one_write = self.kept.one_write().await;
        let Some(Waiting { decision, task }) = self.board.decision(workspace, id).await? else {
            return Ok(());
        };
        if !domain::waits(&decision) {
            // However it ended (approved, declined, withdrawn): its message
            // says it and its buttons go.
            if let Some(prompt) = self.kept.settle(id) {
                let text = domain::over(&decision.decision.unwrap_or_default());
                self.bot
                    .edit(prompt.chat, prompt.message, &text, &[])
                    .await?;
            }
            return Ok(());
        }
        if let Some((number, prompt)) = self.kept.of(id) {
            if prompt.version == decision.version {
                return Ok(());
            }
            // Written again while it waits: its one message as it is now.
            let (text, buttons) = domain::prompt(&decision, &task, number);
            if text != prompt.text {
                self.bot
                    .edit(prompt.chat, prompt.message, &text, &buttons)
                    .await?;
            }
            self.kept.keep(
                number,
                Prompt {
                    version: decision.version,
                    text,
                    ..prompt
                },
            );
            return Ok(());
        }
        let chat = domain::chat(self.config, &self.config.approvals).map_err(refused)?;
        let number = self.kept.number();
        let (text, buttons) = domain::prompt(&decision, &task, number);
        let message = self.bot.send(chat, &text, &buttons).await?;
        let data = decision.decision.unwrap_or_default();
        self.kept.keep(
            number,
            Prompt {
                workspace: workspace.to_string(),
                decision: id.to_string(),
                version: decision.version,
                text,
                what: data.what,
                chat,
                message,
            },
        );
        Ok(())
    }

    /// Read the updates since the last read (waiting up to `timeout`
    /// seconds for one) and act on each.
    ///
    /// # Errors
    /// The Bot API failed reading them; acting on one fails only it (its
    /// person is told).
    pub async fn poll(&self, timeout: u32) -> Result<(), ModuleError> {
        for update in self.bot.updates(self.kept.offset(), timeout).await? {
            self.kept.read(update.update_id);
            self.update(update).await?;
        }
        Ok(())
    }

    async fn update(&self, update: Update) -> Result<(), ModuleError> {
        if let Some(query) = update.callback_query {
            let from = query.from.as_ref().map_or(0, |u| u.id);
            let said = self.press(from, &query.data).await;
            return self.bot.answer(&query.id, &said).await;
        }
        let Some(message) = update.message else {
            return Ok(());
        };
        let chat = message.chat.as_ref().map_or(0, |c| c.id);
        let from = message.from.as_ref().map_or(0, |u| u.id);
        // Only its chats, only the people it acts for, only text.
        let (true, Some(person), Ok(question)) = (
            domain::serves(self.config, chat),
            domain::person(self.config, from),
            domain::said(&message.text),
        ) else {
            return Ok(());
        };
        let answer = match self
            .chief
            .ask(person, question, &self.kept.talk(chat))
            .await
        {
            Ok((answer, conversation)) => {
                self.kept.talking(chat, conversation);
                answer
            }
            Err(e) => format!("The chief could not answer: {}", e.message),
        };
        let answer = if answer.trim().is_empty() {
            "The chief gave no answer.".to_string()
        } else {
            answer
        };
        self.bot.send(chat, &answer, &[]).await?;
        Ok(())
    }

    /// What a press of Telegram user `from` with `data` does, and what it
    /// tells them.
    async fn press(&self, from: i64, data: &str) -> String {
        let Some((choice, number)) = domain::pressed(data) else {
            return "This button is not the channel's.".to_string();
        };
        let Some(person) = domain::person(self.config, from) else {
            return "You do not decide here: the channel acts for no one of that account."
                .to_string();
        };
        let Some(prompt) = self.kept.prompt(number) else {
            return "This prompt is gone; approve it in the app.".to_string();
        };
        let done = match choice {
            Choice::Approve => {
                self.board
                    .approve(&prompt.workspace, person, &prompt.decision, prompt.version)
                    .await
            }
            Choice::Decline => {
                self.board
                    .decline(&prompt.workspace, person, &prompt.decision, prompt.version)
                    .await
            }
        };
        if let Err(e) = done {
            return format!("Not done: {}", e.message);
        }
        let text = domain::settled(&prompt.what, choice, person);
        // The decision's own write may have settled its message already.
        if self.kept.settle(&prompt.decision).is_none() {
            return text;
        }
        match self.bot.edit(prompt.chat, prompt.message, &text, &[]).await {
            Ok(()) => text,
            Err(e) => format!("{text} (the message was not updated: {})", e.message),
        }
    }
}

#[cfg(test)]
mod tests;
