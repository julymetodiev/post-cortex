// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The Bot API (ARCHITECTURE section 8, the outside world's JSON): each
//! call's input a mirror message, sent as form fields (the Bot API's form
//! encoding, whose values are text and whose `reply_markup` is its JSON);
//! each answer decoded into its mirror message by the canonical proto3
//! JSON mapping. The form fields are the input's proto3 JSON, field by
//! field.

use std::time::Duration;

use async_trait::async_trait;
use pcx_module_api::{ModuleError, ModuleErrorKind};
use post_cortex_proto::telegram::{
    Answer, AnswerCallbackQuery, Button, EditMessageText, GetUpdates, MessageAnswer, SendMessage,
    Update, UpdatesAnswer,
};
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::ports::Bot;

/// The Bot API's `reply_markup` of one row of inline buttons: an array of
/// rows, which a proto field cannot be, around the mirror buttons.
#[derive(Serialize)]
struct Keyboard<'a> {
    inline_keyboard: [&'a [Button]; 1],
}

fn unavailable(what: impl std::fmt::Display) -> ModuleError {
    ModuleError::new(ModuleErrorKind::Unavailable, format!("the Bot API: {what}"))
}

/// The Bot API of one bot.
pub struct HttpBot {
    http: reqwest::Client,
    base: String,
}

impl HttpBot {
    /// The bot of `token` at `api` (e.g. `https://api.telegram.org`).
    ///
    /// # Errors
    /// The HTTP client could not be made.
    pub fn new(api: &str, token: &str) -> Result<Self, ModuleError> {
        let http = reqwest::Client::builder()
            // A long poll waits up to its timeout; a margin on top.
            .timeout(Duration::from_secs(60))
            .build()
            .map_err(unavailable)?;
        Ok(Self {
            http,
            base: format!("{}/bot{token}", api.trim_end_matches('/')),
        })
    }

    /// Call `method` with `input`: its answer.
    async fn call<I: Serialize, O: DeserializeOwned>(
        &self,
        method: &str,
        input: &I,
    ) -> Result<O, ModuleError> {
        let fields = form(input)?;
        let response = self
            .http
            .post(format!("{}/{method}", self.base))
            .form(&fields)
            .send()
            .await
            // Its URL holds the token: the error says only what failed.
            .map_err(|e| unavailable(format!("{method} could not be sent ({})", kind(&e))))?;
        let body = response
            .bytes()
            .await
            .map_err(|e| unavailable(format!("{method}'s answer did not come ({})", kind(&e))))?;
        serde_json::from_slice(&body).map_err(|e| unavailable(format!("{method}'s answer: {e}")))
    }
}

/// What kind of failure `e` is, without its URL.
fn kind(e: &reqwest::Error) -> &'static str {
    if e.is_timeout() {
        "timed out"
    } else if e.is_connect() {
        "no connection"
    } else {
        "failed"
    }
}

/// `input`'s proto3 JSON as form fields: each field's text (a string as it
/// is, anything else as its JSON).
fn form<I: Serialize>(input: &I) -> Result<Vec<(String, String)>, ModuleError> {
    let serde_json::Value::Object(fields) = serde_json::to_value(input).map_err(unavailable)?
    else {
        return Err(unavailable("an input is not a message"));
    };
    Ok(fields
        .into_iter()
        .map(|(name, value)| match value {
            serde_json::Value::String(text) => (name, text),
            other => (name, other.to_string()),
        })
        .collect())
}

/// `buttons` (one row) as the Bot API's `reply_markup`; none: empty.
fn markup(buttons: &[Button]) -> Result<String, ModuleError> {
    if buttons.is_empty() {
        return Ok(String::new());
    }
    serde_json::to_string(&Keyboard {
        inline_keyboard: [buttons],
    })
    .map_err(unavailable)
}

/// `ok` with `description`, as a result.
fn ok(ok: bool, description: &str, method: &str) -> Result<(), ModuleError> {
    if ok {
        Ok(())
    } else {
        Err(ModuleError::new(
            ModuleErrorKind::FailedPrecondition,
            format!("the Bot API refused {method}: {description}"),
        ))
    }
}

#[async_trait]
impl Bot for HttpBot {
    async fn updates(&self, offset: i64, timeout: u32) -> Result<Vec<Update>, ModuleError> {
        let input = GetUpdates {
            offset,
            timeout,
            allowed_updates: vec!["message".to_string(), "callback_query".to_string()],
        };
        let answer: UpdatesAnswer = self.call("getUpdates", &input).await?;
        ok(answer.ok, &answer.description, "getUpdates")?;
        Ok(answer.result)
    }

    async fn send(&self, chat: i64, text: &str, buttons: &[Button]) -> Result<i64, ModuleError> {
        let input = SendMessage {
            chat_id: chat,
            text: text.to_string(),
            reply_markup: markup(buttons)?,
        };
        let answer: MessageAnswer = self.call("sendMessage", &input).await?;
        ok(answer.ok, &answer.description, "sendMessage")?;
        Ok(answer.result.map_or(0, |m| m.message_id))
    }

    async fn edit(
        &self,
        chat: i64,
        message: i64,
        text: &str,
        buttons: &[Button],
    ) -> Result<(), ModuleError> {
        let input = EditMessageText {
            chat_id: chat,
            message_id: message,
            text: text.to_string(),
            reply_markup: markup(buttons)?,
        };
        let answer: Answer = self.call("editMessageText", &input).await?;
        ok(answer.ok, &answer.description, "editMessageText")
    }

    async fn answer(&self, query: &str, text: &str) -> Result<(), ModuleError> {
        let input = AnswerCallbackQuery {
            callback_query_id: query.to_string(),
            text: text.to_string(),
        };
        let answer: Answer = self.call("answerCallbackQuery", &input).await?;
        ok(answer.ok, &answer.description, "answerCallbackQuery")
    }
}

#[cfg(test)]
mod tests;
