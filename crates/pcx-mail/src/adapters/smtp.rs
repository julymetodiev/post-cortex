// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Sending through an SMTP relay (`lettre`): STARTTLS that must succeed,
//! TLS from the start, or a plain connection to a loopback host. SES
//! through its SMTP interface is a relay like any other.

use std::time::Duration;

use async_trait::async_trait;
use lettre::message::header::ContentType;
use lettre::message::{Mailbox as Address, MultiPart, SinglePart};
use lettre::transport::smtp::authentication::Credentials;
use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};
use pcx_module_api::{ModuleError, ModuleErrorKind};
use post_cortex_proto::mail::Mailbox;

use crate::domain::Secured;
use crate::ports::{Outgoing, Transport};

/// How long a send may take, connecting included.
const TIMEOUT: Duration = Duration::from_secs(60);

fn refused(kind: ModuleErrorKind, what: impl std::fmt::Display) -> ModuleError {
    ModuleError::new(kind, format!("the SMTP relay: {what}"))
}

/// An SMTP relay.
pub struct Smtp {
    relay: AsyncSmtpTransport<Tokio1Executor>,
}

impl Smtp {
    /// The relay at `host`:`port`, secured as `secured`, signing in with
    /// `credentials` (none: no sign-in). Nothing connects until a send.
    ///
    /// # Errors
    /// `InvalidArgument` when the host cannot be a TLS name.
    pub fn new(
        host: &str,
        port: u16,
        secured: Secured,
        credentials: Option<(String, String)>,
    ) -> Result<Self, ModuleError> {
        let builder = match secured {
            Secured::Tls => AsyncSmtpTransport::<Tokio1Executor>::relay(host),
            Secured::StartTls => AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(host),
            Secured::Plain => Ok(AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(
                host,
            )),
        }
        .map_err(|e| refused(ModuleErrorKind::InvalidArgument, e))?
        .port(port)
        .timeout(Some(TIMEOUT));
        let builder = match credentials {
            Some((user, password)) => builder.credentials(Credentials::new(user, password)),
            None => builder,
        };
        Ok(Self {
            relay: builder.build(),
        })
    }
}

fn mailbox(field: &str, mailbox: &Mailbox) -> Result<Address, ModuleError> {
    let address = mailbox.address.parse().map_err(|e| {
        refused(
            ModuleErrorKind::InvalidArgument,
            format!("{field} {:?}: {e}", mailbox.address),
        )
    })?;
    let name = Some(mailbox.name.clone()).filter(|n| !n.is_empty());
    Ok(Address::new(name, address))
}

fn bare(field: &str, address: &str) -> Result<Address, ModuleError> {
    mailbox(
        field,
        &Mailbox {
            address: address.to_string(),
            name: String::new(),
        },
    )
}

/// `mail` as a MIME message: text, or text and HTML as alternatives.
fn message(mail: &Outgoing) -> Result<Message, ModuleError> {
    let mut builder = Message::builder()
        .from(mailbox("from", &mail.from)?)
        .subject(&mail.subject)
        // lettre makes the id from the host's name.
        .message_id(None);
    if let Some(reply_to) = &mail.reply_to {
        builder = builder.reply_to(bare("reply_to", reply_to)?);
    }
    for to in &mail.to {
        builder = builder.to(bare("to", to)?);
    }
    for cc in &mail.cc {
        builder = builder.cc(bare("cc", cc)?);
    }
    for bcc in &mail.bcc {
        builder = builder.bcc(bare("bcc", bcc)?);
    }
    let built = match &mail.html {
        Some(html) => builder.multipart(MultiPart::alternative_plain_html(
            mail.text.clone(),
            html.clone(),
        )),
        None => builder.singlepart(
            SinglePart::builder()
                .header(ContentType::TEXT_PLAIN)
                .body(mail.text.clone()),
        ),
    };
    built.map_err(|e| refused(ModuleErrorKind::InvalidArgument, e))
}

#[async_trait]
impl Transport for Smtp {
    async fn send(&self, mail: &Outgoing) -> Result<String, ModuleError> {
        let message = message(mail)?;
        let id = message
            .headers()
            .get_raw("Message-ID")
            .map(str::to_string)
            .ok_or_else(|| refused(ModuleErrorKind::Internal, "the message has no Message-ID"))?;
        self.relay.send(message).await.map_err(|e| {
            // A permanent answer (5xx) refuses the message or the sign-in;
            // anything else may pass on a later try.
            let kind = if e.is_permanent() {
                ModuleErrorKind::FailedPrecondition
            } else {
                ModuleErrorKind::Unavailable
            };
            refused(kind, e)
        })?;
        Ok(id)
    }
}

#[cfg(test)]
mod tests;
