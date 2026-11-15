// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! What the mail module needs of the world (docs/modules/mail.md) besides
//! its documents (`pcx-documents`): the time, and a transport that sends a
//! message. The module's adapters fill them (SMTP, Resend, Postmark);
//! tests fill them in memory.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use pcx_module_api::ModuleError;
use post_cortex_proto::mail::Mailbox;

/// The time.
pub trait Clock: Send + Sync {
    /// Now.
    fn now(&self) -> DateTime<Utc>;
}

/// A message to send, checked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outgoing {
    /// Who it is from.
    pub from: Mailbox,
    /// Where answers go; none: to `from`.
    pub reply_to: Option<String>,
    /// Its recipients.
    pub to: Vec<String>,
    /// Its copies.
    pub cc: Vec<String>,
    /// Its hidden copies.
    pub bcc: Vec<String>,
    /// Its subject.
    pub subject: String,
    /// Its text body.
    pub text: String,
    /// Its HTML body; none: text only.
    pub html: Option<String>,
}

/// Sends mail.
#[async_trait]
pub trait Transport: Send + Sync {
    /// Send `mail`: the transport's id of the message.
    ///
    /// # Errors
    /// `Denied` when the provider refuses the credentials,
    /// `InvalidArgument` or `FailedPrecondition` when it refuses the
    /// message, `ResourceExhausted` when it limits the rate, `Unavailable`
    /// when it cannot be reached or fails.
    async fn send(&self, mail: &Outgoing) -> Result<String, ModuleError>;
}
