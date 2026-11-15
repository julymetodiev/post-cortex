// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The Telegram channel's domain (ARCHITECTURE section 8, operations,
//! W29): what its settings may be, who a Telegram user is, where a message
//! goes, what a decision waiting for a person reads as and what its
//! buttons send back. Pure: no host, no network.

use std::fmt;

use post_cortex_proto::board::{Decision, DecisionRecord, Risk};
use post_cortex_proto::telegram::{Button, TelegramConfig};

/// The module's id.
pub const MODULE_ID: &str = "telegram";

/// The Bot API's URL when the settings name none.
pub const API_URL: &str = "https://api.telegram.org";

/// A setting or a request the channel refuses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refused {
    /// A setting is wrong.
    Setting {
        /// The setting, as a path of field names.
        field: String,
        /// What is wrong.
        reason: String,
    },
    /// No chat of that name.
    UnknownChat(String),
    /// Nothing to send.
    Empty,
}

impl fmt::Display for Refused {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Setting { field, reason } => write!(f, "{field}: {reason}"),
            Self::UnknownChat(name) => write!(f, "no chat of the channel is named {name}"),
            Self::Empty => f.write_str("a message says something"),
        }
    }
}

impl std::error::Error for Refused {}

fn setting(field: String, reason: impl Into<String>) -> Refused {
    Refused::Setting {
        field,
        reason: reason.into(),
    }
}

/// Whether `config` may run: a token; chats named once; people each once,
/// each a person the module acts for; the approvals chat one of its chats.
///
/// # Errors
/// The first setting that is wrong.
pub fn check(config: &TelegramConfig) -> Result<(), Refused> {
    if config.bot_token.trim().is_empty() {
        return Err(setting("bot_token".to_string(), "is empty"));
    }
    let mut names: Vec<&str> = Vec::new();
    for (i, chat) in config.chats.iter().enumerate() {
        let name = chat.name.as_str();
        if !pcx_module_api::name::is_slug(name) {
            return Err(setting(
                format!("chats.{i}.name"),
                "is 1 to 64 of a-z, 0-9 and '-'",
            ));
        }
        if names.contains(&name) {
            return Err(setting(
                format!("chats.{i}.name"),
                format!("names {name} twice"),
            ));
        }
        names.push(name);
    }
    let persons = config
        .persons
        .as_ref()
        .map(|p| p.persons.as_slice())
        .unwrap_or_default();
    let mut ids: Vec<i64> = Vec::new();
    for (i, person) in config.people.iter().enumerate() {
        if ids.contains(&person.telegram_id) {
            return Err(setting(
                format!("people.{i}.telegram_id"),
                format!("names user {} twice", person.telegram_id),
            ));
        }
        ids.push(person.telegram_id);
        if !persons.contains(&person.name) {
            return Err(setting(
                format!("people.{i}.name"),
                format!("{} is not one of the persons it acts for", person.name),
            ));
        }
    }
    if !config.approvals.is_empty() && !names.contains(&config.approvals.as_str()) {
        return Err(setting(
            "approvals".to_string(),
            format!("{} is not one of its chats", config.approvals),
        ));
    }
    Ok(())
}

/// The id of chat `name`.
///
/// # Errors
/// No chat of that name.
pub fn chat(config: &TelegramConfig, name: &str) -> Result<i64, Refused> {
    config
        .chats
        .iter()
        .find(|c| c.name == name)
        .map(|c| c.chat_id)
        .ok_or_else(|| Refused::UnknownChat(name.to_string()))
}

/// Whether the bot serves chat `id` (it reads only its chats).
#[must_use]
pub fn serves(config: &TelegramConfig, id: i64) -> bool {
    config.chats.iter().any(|c| c.chat_id == id)
}

/// The person Telegram user `id` is, when the module acts for them.
#[must_use]
pub fn person(config: &TelegramConfig, id: i64) -> Option<&str> {
    config
        .people
        .iter()
        .find(|p| p.telegram_id == id)
        .map(|p| p.name.as_str())
}

/// `text` to send, trimmed.
///
/// # Errors
/// It says nothing.
pub fn said(text: &str) -> Result<&str, Refused> {
    match text.trim() {
        "" => Err(Refused::Empty),
        text => Ok(text),
    }
}

/// What a person does with a decision that waits for them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Choice {
    /// Approve it: its task moves again.
    Approve,
    /// Decline it: it waits no more, and who asked it gives up.
    Decline,
}

impl Choice {
    fn letter(self) -> char {
        match self {
            Self::Approve => 'a',
            Self::Decline => 'd',
        }
    }
}

/// What a press of a decision's button sends back: the choice and the
/// prompt's number (the decision behind it the module keeps; a button
/// carries at most 64 bytes).
#[must_use]
pub fn callback(choice: Choice, prompt: u64) -> String {
    format!("{}:{prompt}", choice.letter())
}

/// The choice and the prompt a button's `data` names; none for data the
/// module did not write.
#[must_use]
pub fn pressed(data: &str) -> Option<(Choice, u64)> {
    let (letter, prompt) = data.split_once(':')?;
    let choice = match letter {
        "a" => Choice::Approve,
        "d" => Choice::Decline,
        _ => return None,
    };
    match prompt.parse() {
        Ok(prompt) => Some((choice, prompt)),
        Err(_) => None,
    }
}

/// A decision's risk for a person.
fn risk(risk: Risk) -> &'static str {
    match risk {
        Risk::Code => "code",
        Risk::Money => "money",
        Risk::Data => "data",
        Risk::Operations => "operations",
        Risk::None | Risk::Unspecified => "nothing",
    }
}

/// Whether `decision` waits for a person (the board says it in `pending`).
#[must_use]
pub fn waits(decision: &DecisionRecord) -> bool {
    decision.pending
}

/// The message of a decision that waits for a person, on task `task`, and
/// its buttons for prompt number `prompt`.
#[must_use]
pub fn prompt(decision: &DecisionRecord, task: &str, prompt: u64) -> (String, Vec<Button>) {
    let data = decision.decision.clone().unwrap_or_default();
    let mut text = vec![
        format!("Waits for you: {}", data.what),
        format!("Why: {}", data.why),
        format!("Task: {task}"),
        format!("By {}, risking {}", data.decided_by, risk(data.risk())),
    ];
    if let Some(held) = &data.held_because {
        text.push(format!("Not approved by the policy: {held}"));
    }
    let buttons = vec![
        Button {
            text: "Approve".to_string(),
            callback_data: callback(Choice::Approve, prompt),
        },
        Button {
            text: "Decline".to_string(),
            callback_data: callback(Choice::Decline, prompt),
        },
    ];
    (text.join("\n"), buttons)
}

/// The prompt's text once `person` chose `choice` (or someone approved it
/// elsewhere: `person` is the approver).
#[must_use]
pub fn settled(what: &str, choice: Choice, person: &str) -> String {
    match choice {
        Choice::Approve => format!("Approved by {person}: {what}"),
        Choice::Decline => format!("Declined by {person}: {what}"),
    }
}

/// What the message of `decision` says once it waits no more, however it
/// ended: approved or declined (here or elsewhere), or withdrawn by who
/// asked it.
#[must_use]
pub fn over(decision: &Decision) -> String {
    let what = &decision.what;
    if let Some(person) = &decision.approved_by {
        settled(what, Choice::Approve, person)
    } else if let Some(person) = &decision.declined_by {
        settled(what, Choice::Decline, person)
    } else if let Some(why) = &decision.withdrawn {
        format!("Withdrawn by {}: {what} ({why})", decision.decided_by)
    } else {
        format!("Waits no more: {what}")
    }
}

#[cfg(test)]
pub mod tests;
