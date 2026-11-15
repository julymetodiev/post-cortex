// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The SMTP transport against a relay on the loopback that speaks just
//! enough SMTP: what it was sent, and a refused recipient.

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use tokio::sync::mpsc;

use super::*;

fn mail(html: Option<&str>) -> Outgoing {
    Outgoing {
        from: Mailbox {
            address: "reports@example.com".into(),
            name: "Reports".into(),
        },
        reply_to: Some("ops@example.com".into()),
        to: vec!["ana@example.com".into()],
        cc: Vec::new(),
        bcc: vec!["audit@example.com".into()],
        subject: "The weekly report".into(),
        text: "All is well.".into(),
        html: html.map(str::to_string),
    }
}

/// A relay that refuses `refuse` as a recipient (550) and answers every
/// other command; what a session sent up to each message's end, its
/// commands and data.
async fn relay(refuse: &'static str) -> (u16, mpsc::UnboundedReceiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let (tx, rx) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        loop {
            let (socket, _) = listener.accept().await.unwrap();
            let (read, mut write) = socket.into_split();
            let mut lines = BufReader::new(read).lines();
            let mut said = String::new();
            write.write_all(b"220 relay ready\r\n").await.unwrap();
            let mut data = false;
            while let Some(line) = lines.next_line().await.unwrap() {
                said.push_str(&line);
                said.push('\n');
                let answer: &[u8] = if data {
                    if line == "." {
                        data = false;
                        // The message is whole: what it was sent, now (the
                        // client's pool keeps the connection open).
                        if tx.send(said.clone()).is_err() {
                            return;
                        }
                        b"250 queued\r\n"
                    } else {
                        continue;
                    }
                } else if line.starts_with("EHLO") {
                    b"250 relay\r\n"
                } else if line.starts_with("RCPT") && line.contains(refuse) {
                    b"550 no such user\r\n"
                } else if line == "DATA" {
                    data = true;
                    b"354 go on\r\n"
                } else if line == "QUIT" {
                    write.write_all(b"221 bye\r\n").await.unwrap();
                    break;
                } else {
                    b"250 ok\r\n"
                };
                write.write_all(answer).await.unwrap();
            }
        }
    });
    (port, rx)
}

#[tokio::test]
async fn a_message_goes_to_its_recipients_with_its_alternatives() {
    let (port, mut said) = relay("nobody@").await;
    let smtp = Smtp::new("127.0.0.1", port, Secured::Plain, None).unwrap();
    let id = smtp.send(&mail(Some("<p>All is well.</p>"))).await.unwrap();
    assert!(id.starts_with('<') && id.ends_with('>'), "{id}");

    let session = said.recv().await.unwrap();
    assert!(
        session.contains("MAIL FROM:<reports@example.com>"),
        "{session}"
    );
    assert!(session.contains("RCPT TO:<ana@example.com>"), "{session}");
    assert!(session.contains("RCPT TO:<audit@example.com>"), "{session}");
    assert!(session.contains("Subject: The weekly report"), "{session}");
    assert!(session.contains("Reply-To: ops@example.com"), "{session}");
    assert!(session.contains("multipart/alternative"), "{session}");
    assert!(session.contains("text/html"), "{session}");
    assert!(session.contains(&format!("Message-ID: {id}")), "{session}");
    // A hidden copy is a recipient, never a header.
    assert!(!session.contains("Bcc:"), "{session}");
}

#[tokio::test]
async fn a_recipient_the_relay_refuses_fails_the_send_for_good() {
    let (port, _said) = relay("ana@").await;
    let smtp = Smtp::new("127.0.0.1", port, Secured::Plain, None).unwrap();
    let failed = smtp.send(&mail(None)).await.unwrap_err();
    assert_eq!(failed.kind, ModuleErrorKind::FailedPrecondition);
    assert!(
        failed.message.starts_with("the SMTP relay:"),
        "{}",
        failed.message
    );
}

#[tokio::test]
async fn a_relay_that_is_not_there_may_be_tried_again() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let smtp = Smtp::new("127.0.0.1", port, Secured::Plain, None).unwrap();
    let failed = smtp.send(&mail(None)).await.unwrap_err();
    assert_eq!(failed.kind, ModuleErrorKind::Unavailable);
}

/// A message through a real relay: `MAIL_LIVE_HOST` (TLS on 465), signed in
/// as `MAIL_LIVE_FROM` with `MAIL_LIVE_PASSWORD`, to `MAIL_LIVE_TO`. Run by
/// hand: `cargo test -p pcx-mail -- --ignored a_real_relay`.
#[tokio::test]
#[ignore = "sends real mail; needs MAIL_LIVE_* in the environment"]
async fn a_real_relay_takes_a_message() {
    let var = |name: &str| std::env::var(name).unwrap_or_else(|_| panic!("{name} is not set"));
    let from = var("MAIL_LIVE_FROM");
    let smtp = Smtp::new(
        &var("MAIL_LIVE_HOST"),
        465,
        Secured::Tls,
        Some((from.clone(), var("MAIL_LIVE_PASSWORD"))),
    )
    .unwrap();
    let mut live = mail(Some("<p>A message from the <b>pcx</b> mail module.</p>"));
    live.from = Mailbox {
        address: from,
        name: "pcx".into(),
    };
    live.reply_to = None;
    live.to = vec![var("MAIL_LIVE_TO")];
    live.bcc = Vec::new();
    live.subject = "pcx mail: a live test".into();
    live.text = "A message from the pcx mail module.".into();
    let id = smtp.send(&live).await.unwrap();
    assert!(id.starts_with('<'), "{id}");
}
