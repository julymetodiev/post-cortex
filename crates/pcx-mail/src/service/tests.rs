// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The mail service over documents held in memory, a clock the test sets
//! and a transport that records what it was given (or fails).

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use parking_lot::Mutex as Held;
use pcx_documents::InMemoryDocuments;
use post_cortex_proto::mail::{MailConfig, Mailbox, Smtp, Transport as Via};

use super::*;

struct Fixed(Held<DateTime<Utc>>);

impl Fixed {
    fn at(text: &str) -> Self {
        Self(Held::new(at(text)))
    }

    fn set(&self, text: &str) {
        *self.0.lock() = at(text);
    }
}

fn at(text: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(text)
        .unwrap()
        .with_timezone(&Utc)
}

impl Clock for Fixed {
    fn now(&self) -> DateTime<Utc> {
        *self.0.lock()
    }
}

/// A transport that records each message, answering `id-<n>`, or fails
/// with `fail`.
#[derive(Default)]
struct Recorded {
    sent: Held<Vec<Outgoing>>,
    fail: Option<ModuleErrorKind>,
}

#[async_trait]
impl Transport for Recorded {
    async fn send(&self, mail: &Outgoing) -> Result<String, ModuleError> {
        if let Some(kind) = self.fail {
            return Err(ModuleError::new(kind, "the SMTP relay: 451 try later"));
        }
        let mut sent = self.sent.lock();
        sent.push(mail.clone());
        Ok(format!("id-{}", sent.len()))
    }
}

fn settings(allow: &[&str], daily_limit: u32) -> Settings {
    domain::settings(MailConfig {
        workspace_id: "ws-1".into(),
        from: Some(Mailbox {
            address: "reports@example.com".into(),
            name: "Reports".into(),
        }),
        reply_to: "ops@example.com".into(),
        transport: Via::Smtp.into(),
        smtp: Some(Smtp {
            host: "smtp.example.com".into(),
            ..Smtp::default()
        }),
        allow: allow.iter().map(|a| (*a).to_string()).collect(),
        daily_limit,
        ..MailConfig::default()
    })
    .unwrap()
}

fn message(to: &str) -> SendRequest {
    SendRequest {
        to: vec![to.to_string()],
        subject: "The weekly report".into(),
        text: "All is well.".into(),
        ..SendRequest::default()
    }
}

fn docs() -> InMemoryDocuments {
    InMemoryDocuments::new(Some("mail"), post_cortex_proto::MAIL_DESCRIPTOR_SET).unwrap()
}

fn outcome(record: &SentRecord) -> Outcome {
    record.sent.as_ref().unwrap().outcome()
}

#[tokio::test]
async fn a_send_goes_through_the_transport_and_is_logged() {
    let (docs, clock, transport, gate) = (
        docs(),
        Fixed::at("2026-10-31T09:30:00Z"),
        Recorded::default(),
        Mutex::new(()),
    );
    let settings = settings(&[], 0);
    let service = Service::new(&docs, &clock, &transport, &settings, &gate);
    let mut asked = message("ana@example.com");
    asked.html = "<p>All is well.</p>".into();
    let record = service.send(asked, Some("claude")).await.unwrap();

    let sent = transport.sent.lock().clone();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].from.address, "reports@example.com");
    assert_eq!(sent[0].reply_to.as_deref(), Some("ops@example.com"));
    assert_eq!(sent[0].html.as_deref(), Some("<p>All is well.</p>"));
    let entry = record.sent.as_ref().unwrap();
    assert_eq!(entry.outcome(), Outcome::Sent);
    assert_eq!(
        (
            entry.message_id.as_str(),
            entry.day.as_str(),
            entry.author.as_str()
        ),
        ("id-1", "2026-10-31", "claude")
    );
    let page = service.list_log(ListLogRequest::default()).await.unwrap();
    assert_eq!(page.entries, vec![record]);
}

#[tokio::test]
async fn a_message_the_rules_refuse_is_neither_sent_nor_logged() {
    let (docs, clock, transport, gate) = (
        docs(),
        Fixed::at("2026-10-31T09:30:00Z"),
        Recorded::default(),
        Mutex::new(()),
    );
    let settings = settings(&[], 0);
    let service = Service::new(&docs, &clock, &transport, &settings, &gate);
    let failed = service.send(message("ana"), None).await.unwrap_err();
    assert_eq!(
        (failed.kind, failed.field.as_deref()),
        (ModuleErrorKind::InvalidArgument, Some("to"))
    );
    assert!(transport.sent.lock().is_empty());
    assert!(
        service
            .list_log(ListLogRequest::default())
            .await
            .unwrap()
            .entries
            .is_empty()
    );
}

#[tokio::test]
async fn a_recipient_outside_allow_is_refused_and_logged() {
    let (docs, clock, transport, gate) = (
        docs(),
        Fixed::at("2026-10-31T09:30:00Z"),
        Recorded::default(),
        Mutex::new(()),
    );
    let settings = settings(&["@example.com"], 0);
    let service = Service::new(&docs, &clock, &transport, &settings, &gate);
    let mut asked = message("ana@example.com");
    asked.bcc = vec!["leak@elsewhere.example".into()];
    let failed = service.send(asked, None).await.unwrap_err();
    assert_eq!(failed.kind, ModuleErrorKind::Denied);
    assert!(
        failed.message.contains("leak@elsewhere.example"),
        "{}",
        failed.message
    );
    assert!(transport.sent.lock().is_empty());
    let log = service.list_log(ListLogRequest::default()).await.unwrap();
    assert_eq!(outcome(&log.entries[0]), Outcome::Refused);
}

#[tokio::test]
async fn the_daily_limit_counts_the_day_s_sends() {
    let (docs, clock, transport, gate) = (
        docs(),
        Fixed::at("2026-10-31T09:30:00Z"),
        Recorded::default(),
        Mutex::new(()),
    );
    let settings = settings(&[], 2);
    let service = Service::new(&docs, &clock, &transport, &settings, &gate);
    service.send(message("a@example.com"), None).await.unwrap();
    service.send(message("b@example.com"), None).await.unwrap();
    let failed = service
        .send(message("c@example.com"), None)
        .await
        .unwrap_err();
    assert_eq!(failed.kind, ModuleErrorKind::ResourceExhausted);
    // A refusal is no send: the next day counts afresh.
    clock.set("2026-11-01T00:00:01Z");
    service.send(message("c@example.com"), None).await.unwrap();
    assert_eq!(transport.sent.lock().len(), 3);
}

#[tokio::test]
async fn a_send_the_transport_fails_is_logged_with_why() {
    let (docs, clock, gate) = (docs(), Fixed::at("2026-10-31T09:30:00Z"), Mutex::new(()));
    let transport = Recorded {
        fail: Some(ModuleErrorKind::Unavailable),
        ..Recorded::default()
    };
    let settings = settings(&[], 0);
    let service = Service::new(&docs, &clock, &transport, &settings, &gate);
    let failed = service
        .send(message("ana@example.com"), None)
        .await
        .unwrap_err();
    assert_eq!(failed.kind, ModuleErrorKind::Unavailable);
    let log = service.list_log(ListLogRequest::default()).await.unwrap();
    let entry = log.entries[0].sent.as_ref().unwrap();
    assert_eq!(entry.outcome(), Outcome::Failed);
    assert_eq!(entry.error, "the SMTP relay: 451 try later");
    assert!(
        failed.message.contains(&log.entries[0].id),
        "{}",
        failed.message
    );
}

#[tokio::test]
async fn a_test_message_says_its_transport_and_is_marked() {
    let (docs, clock, transport, gate) = (
        docs(),
        Fixed::at("2026-10-31T09:30:00Z"),
        Recorded::default(),
        Mutex::new(()),
    );
    let settings = settings(&[], 0);
    let service = Service::new(&docs, &clock, &transport, &settings, &gate);
    let record = service.test("ana@example.com", Some("ana")).await.unwrap();
    assert!(record.sent.as_ref().unwrap().test);
    let sent = transport.sent.lock().clone();
    assert_eq!(sent[0].subject, TEST_SUBJECT);
    assert!(sent[0].text.contains("through SMTP"), "{}", sent[0].text);
}

#[tokio::test]
async fn the_log_reads_a_page_at_a_time_and_by_recipient() {
    let (docs, clock, transport, gate) = (
        docs(),
        Fixed::at("2026-10-31T09:30:00Z"),
        Recorded::default(),
        Mutex::new(()),
    );
    let settings = settings(&[], 0);
    let service = Service::new(&docs, &clock, &transport, &settings, &gate);
    for to in ["a@example.com", "b@example.com", "a@example.com"] {
        service.send(message(to), None).await.unwrap();
    }
    let first = service
        .list_log(ListLogRequest {
            limit: 2,
            ..ListLogRequest::default()
        })
        .await
        .unwrap();
    assert_eq!(first.entries.len(), 2);
    assert!(first.more);
    let rest = service
        .list_log(ListLogRequest {
            limit: 2,
            before: first.entries[1].id.clone(),
            ..ListLogRequest::default()
        })
        .await
        .unwrap();
    assert_eq!(rest.entries.len(), 1);
    assert!(!rest.more);
    let to_a = service
        .list_log(ListLogRequest {
            recipient: "A@example.com".into(),
            ..ListLogRequest::default()
        })
        .await
        .unwrap();
    assert_eq!(to_a.entries.len(), 2);
    let gone = service
        .list_log(ListLogRequest {
            before: "no-such-entry".into(),
            ..ListLogRequest::default()
        })
        .await
        .unwrap_err();
    assert_eq!(gone.kind, ModuleErrorKind::NotFound);
}

#[tokio::test]
async fn pruning_removes_what_the_log_no_longer_keeps() {
    let (docs, clock, transport, gate) = (
        docs(),
        Fixed::at("2026-08-01T09:00:00Z"),
        Recorded::default(),
        Mutex::new(()),
    );
    let settings = settings(&[], 0);
    let service = Service::new(&docs, &clock, &transport, &settings, &gate);
    service
        .send(message("old@example.com"), None)
        .await
        .unwrap();
    clock.set("2026-08-02T09:00:00Z");
    service
        .send(message("kept@example.com"), None)
        .await
        .unwrap();
    clock.set("2026-10-31T12:00:00Z");
    assert_eq!(service.prune().await.unwrap(), 1);
    let log = service.list_log(ListLogRequest::default()).await.unwrap();
    assert_eq!(log.entries.len(), 1);
    assert_eq!(
        log.entries[0].sent.as_ref().unwrap().to,
        vec!["kept@example.com"]
    );
}

mod templates;
