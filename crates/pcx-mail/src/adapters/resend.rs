// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Sending through Resend's API: `POST /emails` with the API key as its
//! bearer.

use async_trait::async_trait;
use pcx_module_api::ModuleError;
use post_cortex_proto::mail::{ResendAnswer, ResendEmail};

use crate::adapters::http::{Api, failure};
use crate::domain::mailbox_text;
use crate::ports::{Outgoing, Transport};

/// Resend's API.
pub struct Resend {
    api: Api,
    url: String,
    bearer: String,
}

impl Resend {
    /// The API at `url` with API key `key`.
    ///
    /// # Errors
    /// `Unavailable` when the HTTP client cannot be made.
    pub fn new(url: &str, key: &str) -> Result<Self, ModuleError> {
        Ok(Self {
            api: Api::new("Resend")?,
            url: format!("{url}/emails"),
            bearer: format!("Bearer {key}"),
        })
    }
}

#[async_trait]
impl Transport for Resend {
    async fn send(&self, mail: &Outgoing) -> Result<String, ModuleError> {
        let email = ResendEmail {
            from: mailbox_text(&mail.from),
            to: mail.to.clone(),
            cc: mail.cc.clone(),
            bcc: mail.bcc.clone(),
            reply_to: mail.reply_to.clone().unwrap_or_default(),
            subject: mail.subject.clone(),
            text: mail.text.clone(),
            html: mail.html.clone().unwrap_or_default(),
        };
        let (status, answer): (_, ResendAnswer) = self
            .api
            .post(&self.url, &[("Authorization", &self.bearer)], &email)
            .await?;
        if status.is_success() && !answer.id.is_empty() {
            return Ok(answer.id);
        }
        Err(self.api.failed(
            failure(status),
            format!(
                "refused it (HTTP {status}, {}): {}",
                answer.name, answer.message
            ),
        ))
    }
}
