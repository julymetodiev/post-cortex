// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The mail service (docs/modules/mail.md): a send checked (made from a
//! template when it names one), held to `allow` and the daily limit, sent
//! through the transport and logged whatever its end; a test message; the
//! log read a page at a time and kept for 90 days; the templates
//! (`templates`). Between the handlers (and the module's daily pruning)
//! and the ports.

use pcx_documents::{Documents, Item, Typed, days};
use pcx_module_api::{ModuleError, ModuleErrorKind};
use post_cortex_proto::mail::{ListLogRequest, LogPage, Outcome, SendRequest, Sent, SentRecord};
use prost_types::Timestamp;
use tokio::sync::Mutex;

use crate::domain::{self, Broken, LOG_PAGE, MAX_LOG_PAGE, Settings};
use crate::ports::{Clock, Outgoing, Transport};

/// The subject of a test message.
const TEST_SUBJECT: &str = "A test message from pcx mail";

mod templates;

fn broken(rule: Broken) -> ModuleError {
    let field = rule.field.clone();
    ModuleError::new(ModuleErrorKind::InvalidArgument, rule.to_string()).with_field(field)
}

fn record(item: Item<Sent>) -> SentRecord {
    SentRecord {
        id: item.document.id,
        sent: Some(item.data),
    }
}

/// The mail module's sends and its log in its workspace.
pub struct Service<'a> {
    docs: &'a dyn Documents,
    clock: &'a dyn Clock,
    transport: &'a dyn Transport,
    settings: &'a Settings,
    /// Held while a send is counted, sent and logged, so two sends never
    /// both pass the daily limit's last place.
    gate: &'a Mutex<()>,
}

impl<'a> Service<'a> {
    /// The service over its documents, the clock, the transport and the
    /// settings, its sends one at a time through `gate`.
    pub fn new(
        docs: &'a dyn Documents,
        clock: &'a dyn Clock,
        transport: &'a dyn Transport,
        settings: &'a Settings,
        gate: &'a Mutex<()>,
    ) -> Self {
        Self {
            docs,
            clock,
            transport,
            settings,
            gate,
        }
    }

    fn log(&self) -> Typed<'a> {
        Typed::new(self.docs, domain::sent_kind())
    }

    /// Send `message` asked by `author`, made from its template when it
    /// names one: its log entry.
    ///
    /// # Errors
    /// `NotFound` for a template there is none of; `InvalidArgument` for
    /// values the template refuses or a message the rules refuse (not
    /// logged);
    /// `Denied` for a recipient outside `allow` and
    /// `ResourceExhausted` past the daily limit (both logged as refused);
    /// the transport's failure (logged as failed); memory failed.
    pub async fn send(
        &self,
        message: SendRequest,
        author: Option<&str>,
    ) -> Result<SentRecord, ModuleError> {
        let (message, template) = self.composed(message).await?;
        self.deliver(message, author, false, template).await
    }

    /// A test message to `to` asked by `author`, with the current
    /// settings: its log entry.
    ///
    /// # Errors
    /// As [`Service::send`].
    pub async fn test(&self, to: &str, author: Option<&str>) -> Result<SentRecord, ModuleError> {
        let message = SendRequest {
            to: vec![to.to_string()],
            subject: TEST_SUBJECT.to_string(),
            text: format!(
                "This message was sent by the mail module of pcx through {} to check its settings. Nothing needs to be done.",
                self.settings.route.name()
            ),
            ..SendRequest::default()
        };
        self.deliver(message, author, true, String::new()).await
    }

    /// Send `message` (made from `template` when it names one).
    async fn deliver(
        &self,
        message: SendRequest,
        author: Option<&str>,
        test: bool,
        template: String,
    ) -> Result<SentRecord, ModuleError> {
        domain::check(&message).map_err(broken)?;
        let _turn = self.gate.lock().await;
        let at = self.clock.now();
        let mut entry = Sent {
            subject: message.subject.clone(),
            at: Some(Timestamp {
                seconds: at.timestamp(),
                nanos: 0,
            }),
            day: days::day(at),
            author: author.unwrap_or_default().to_string(),
            to: message.to.clone(),
            cc: message.cc.clone(),
            bcc: message.bcc.clone(),
            outcome: Outcome::Sent.into(),
            message_id: String::new(),
            error: String::new(),
            test,
            template,
        };
        if let Some(outside) = domain::recipients(&message.to, &message.cc, &message.bcc)
            .find(|r| !domain::allowed(&self.settings.allow, r))
        {
            let why = format!("{outside} is not among the recipients the settings allow");
            return self.refused(entry, ModuleErrorKind::Denied, why).await;
        }
        if self.sent_on(&entry.day).await? >= self.settings.daily_limit as usize {
            let why = format!(
                "the {} messages a day the settings allow are sent",
                self.settings.daily_limit
            );
            return self
                .refused(entry, ModuleErrorKind::ResourceExhausted, why)
                .await;
        }
        let outgoing = Outgoing {
            from: self.settings.from.clone(),
            reply_to: self.settings.reply_to.clone(),
            to: message.to,
            cc: message.cc,
            bcc: message.bcc,
            subject: message.subject,
            text: message.text,
            html: Some(message.html).filter(|h| !h.is_empty()),
        };
        match self.transport.send(&outgoing).await {
            Ok(id) => {
                entry.message_id = id;
                Ok(record(self.log().create(&entry, "").await?))
            }
            Err(failed) => {
                entry.outcome = Outcome::Failed.into();
                entry.error.clone_from(&failed.message);
                let logged = self.log().create(&entry, "").await?;
                Err(ModuleError::new(
                    failed.kind,
                    format!("{} (log entry {})", failed.message, logged.document.id),
                ))
            }
        }
    }

    /// Log `entry` as refused for `why`, and fail with `kind`.
    async fn refused(
        &self,
        mut entry: Sent,
        kind: ModuleErrorKind,
        why: String,
    ) -> Result<SentRecord, ModuleError> {
        entry.outcome = Outcome::Refused.into();
        entry.error.clone_from(&why);
        let logged = self.log().create(&entry, "").await?;
        Err(ModuleError::new(
            kind,
            format!("{why} (log entry {})", logged.document.id),
        ))
    }

    /// The messages sent on `day`.
    async fn sent_on(&self, day: &str) -> Result<usize, ModuleError> {
        let example = Sent {
            day: day.to_string(),
            outcome: Outcome::Sent.into(),
            ..Sent::default()
        };
        Ok(self
            .log()
            .all(Some((&example, &["day", "outcome"][..])))
            .await?
            .len())
    }

    /// A page of the log, the newest first: after entry `before` when the
    /// request names one, to `recipient` when it names one.
    ///
    /// # Errors
    /// `NotFound` when `before` is no entry of the log; memory failed.
    pub async fn list_log(&self, request: ListLogRequest) -> Result<LogPage, ModuleError> {
        let limit = match request.limit {
            0 => LOG_PAGE,
            n => usize::try_from(n).map_or(MAX_LOG_PAGE, |n| n.min(MAX_LOG_PAGE)),
        };
        let recipient = request.recipient.to_ascii_lowercase();
        let of_template = Sent {
            template: request.template.clone(),
            ..Sent::default()
        };
        let matching = (!request.template.is_empty()).then_some((&of_template, &["template"][..]));
        let before = Some(request.before.as_str()).filter(|b| !b.is_empty());
        let (entries, more) = self
            .log()
            .after(matching, before, limit, |e: &Item<Sent>| {
                recipient.is_empty()
                    || domain::recipients(&e.data.to, &e.data.cc, &e.data.bcc)
                        .any(|r| r.to_ascii_lowercase() == recipient)
            })
            .await?;
        Ok(LogPage {
            entries: entries.into_iter().map(record).collect::<Vec<SentRecord>>(),
            more,
        })
    }

    /// Remove the entries the log no longer keeps: how many.
    ///
    /// # Errors
    /// Memory failed. An entry whose day does not read is left, and said.
    pub async fn prune(&self) -> Result<usize, ModuleError> {
        let today = self.clock.now();
        let mut removed = 0;
        for entry in self.log().all::<Sent>(None).await? {
            match days::expired(&entry.data.day, today, domain::KEEP_DAYS) {
                Ok(true) => {
                    self.log().delete(&entry.document.id).await?;
                    removed += 1;
                }
                Ok(false) => {}
                Err(rule) => {
                    tracing::warn!(entry = %entry.document.id, %rule, "a log entry is kept: its day does not read");
                }
            }
        }
        Ok(removed)
    }
}

#[cfg(test)]
mod tests;
