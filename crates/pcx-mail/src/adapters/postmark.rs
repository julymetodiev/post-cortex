// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Sending through Postmark's API: `POST /email` with the server's token
//! in `X-Postmark-Server-Token`, on the settings' message stream.

use async_trait::async_trait;
use pcx_module_api::ModuleError;
use post_cortex_proto::mail::{PostmarkAnswer, PostmarkEmail};

use crate::adapters::http::{Api, failure};
use crate::domain::mailbox_text;
use crate::ports::{Outgoing, Transport};

/// Postmark's API.
pub struct Postmark {
    api: Api,
    url: String,
    token: String,
    stream: String,
}

impl Postmark {
    /// The API at `url` with server token `token`, sending on `stream`.
    ///
    /// # Errors
    /// `Unavailable` when the HTTP client cannot be made.
    pub fn new(url: &str, token: &str, stream: &str) -> Result<Self, ModuleError> {
        Ok(Self {
            api: Api::new("Postmark")?,
            url: format!("{url}/email"),
            token: token.to_string(),
            stream: stream.to_string(),
        })
    }
}

#[async_trait]
impl Transport for Postmark {
    async fn send(&self, mail: &Outgoing) -> Result<String, ModuleError> {
        let email = PostmarkEmail {
            from: mailbox_text(&mail.from),
            to: mail.to.join(", "),
            cc: mail.cc.join(", "),
            bcc: mail.bcc.join(", "),
            reply_to: mail.reply_to.clone().unwrap_or_default(),
            subject: mail.subject.clone(),
            text_body: mail.text.clone(),
            html_body: mail.html.clone().unwrap_or_default(),
            message_stream: self.stream.clone(),
        };
        let (status, answer): (_, PostmarkAnswer) = self
            .api
            .post(
                &self.url,
                &[("X-Postmark-Server-Token", &self.token)],
                &email,
            )
            .await?;
        if status.is_success() && answer.error_code == 0 && !answer.message_id.is_empty() {
            return Ok(answer.message_id);
        }
        Err(self.api.failed(
            failure(status),
            format!(
                "refused it (HTTP {status}, code {}): {}",
                answer.error_code, answer.message
            ),
        ))
    }
}
