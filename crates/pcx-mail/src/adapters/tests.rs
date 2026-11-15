// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Resend and Postmark against a server on the loopback: the JSON each
//! API names, its credentials' header, and its answers read.

use post_cortex_proto::mail::Mailbox;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;

use super::*;
use crate::ports::Outgoing;
use pcx_module_api::ModuleErrorKind;

fn mail() -> Outgoing {
    Outgoing {
        from: Mailbox {
            address: "reports@example.com".into(),
            name: "Reports".into(),
        },
        reply_to: None,
        to: vec!["ana@example.com".into(), "bob@example.com".into()],
        cc: Vec::new(),
        bcc: Vec::new(),
        subject: "The weekly report".into(),
        text: "All is well.".into(),
        html: Some("<p>All is well.</p>".into()),
    }
}

/// The headers, then the body its length names.
async fn request(socket: &mut TcpStream) -> String {
    let mut read = Vec::new();
    let mut buf = [0u8; 4096];
    loop {
        let n = socket.read(&mut buf).await.unwrap();
        read.extend_from_slice(&buf[..n]);
        let text = String::from_utf8_lossy(&read).to_string();
        if let Some(end) = text.find("\r\n\r\n") {
            let length: usize = text
                .lines()
                .find_map(|l| {
                    l.to_ascii_lowercase()
                        .strip_prefix("content-length:")
                        .map(|v| v.trim().parse().unwrap())
                })
                .unwrap_or(0);
            if read.len() >= end + 4 + length {
                return text;
            }
        }
    }
}

/// A server that answers each request `status` with `body`; what each
/// request sent.
async fn api(
    status: &'static str,
    body: &'static str,
) -> (String, mpsc::UnboundedReceiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let (tx, rx) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        loop {
            let (mut socket, _) = listener.accept().await.unwrap();
            let sent = request(&mut socket).await;
            let answer = format!(
                "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{body}",
                body.len()
            );
            socket.write_all(answer.as_bytes()).await.unwrap();
            socket.shutdown().await.unwrap();
            if tx.send(sent).is_err() {
                return;
            }
        }
    });
    (url, rx)
}

/// The JSON body of a request.
fn body(sent: &str) -> serde_json::Value {
    let (_, body) = sent.split_once("\r\n\r\n").unwrap();
    serde_json::from_str(body).unwrap()
}

#[tokio::test]
async fn resend_gets_its_json_and_answers_the_id() {
    let (url, mut sent) = api("200 OK", r#"{"id":"re-1"}"#).await;
    let resend = transport(&Route::Resend {
        url,
        key: "re_key".into(),
    })
    .unwrap();
    assert_eq!(resend.send(&mail()).await.unwrap(), "re-1");
    let sent = sent.recv().await.unwrap();
    assert!(sent.starts_with("POST /emails "), "{sent}");
    assert!(sent.contains("authorization: Bearer re_key"), "{sent}");
    assert_eq!(
        body(&sent),
        serde_json::json!({
            "from": "\"Reports\" <reports@example.com>",
            "to": ["ana@example.com", "bob@example.com"],
            "subject": "The weekly report",
            "text": "All is well.",
            "html": "<p>All is well.</p>",
        })
    );
}

#[tokio::test]
async fn resend_refusing_the_key_is_denied() {
    let (url, _sent) = api(
        "401 Unauthorized",
        r#"{"statusCode":401,"name":"missing_api_key","message":"Missing API key"}"#,
    )
    .await;
    let resend = transport(&Route::Resend {
        url,
        key: "re_key".into(),
    })
    .unwrap();
    let failed = resend.send(&mail()).await.unwrap_err();
    assert_eq!(failed.kind, ModuleErrorKind::Denied);
    assert!(
        failed.message.contains("Missing API key"),
        "{}",
        failed.message
    );
    assert!(!failed.message.contains("re_key"), "{}", failed.message);
}

#[tokio::test]
async fn postmark_gets_its_json_on_its_stream_and_answers_the_id() {
    let (url, mut sent) = api(
        "200 OK",
        r#"{"To":"ana@example.com","SubmittedAt":"2026-10-31T12:00:00Z","MessageID":"pm-1","ErrorCode":0,"Message":"OK"}"#,
    )
    .await;
    let postmark = transport(&Route::Postmark {
        url,
        token: "pm_token".into(),
        stream: "outbound".into(),
    })
    .unwrap();
    assert_eq!(postmark.send(&mail()).await.unwrap(), "pm-1");
    let sent = sent.recv().await.unwrap();
    assert!(sent.starts_with("POST /email "), "{sent}");
    assert!(sent.contains("x-postmark-server-token: pm_token"), "{sent}");
    assert_eq!(
        body(&sent),
        serde_json::json!({
            "From": "\"Reports\" <reports@example.com>",
            "To": "ana@example.com, bob@example.com",
            "Subject": "The weekly report",
            "TextBody": "All is well.",
            "HtmlBody": "<p>All is well.</p>",
            "MessageStream": "outbound",
        })
    );
}

#[tokio::test]
async fn postmark_refusing_the_message_is_an_invalid_argument() {
    let (url, _sent) = api(
        "422 Unprocessable Entity",
        r#"{"ErrorCode":300,"Message":"Invalid 'To' address"}"#,
    )
    .await;
    let postmark = transport(&Route::Postmark {
        url,
        token: "pm_token".into(),
        stream: "outbound".into(),
    })
    .unwrap();
    let failed = postmark.send(&mail()).await.unwrap_err();
    assert_eq!(failed.kind, ModuleErrorKind::InvalidArgument);
    assert!(failed.message.contains("code 300"), "{}", failed.message);
}

#[tokio::test]
async fn an_answer_that_is_not_the_api_s_json_may_be_tried_again() {
    let (url, _sent) = api("502 Bad Gateway", "<html>bad gateway</html>").await;
    let resend = transport(&Route::Resend {
        url,
        key: "re_key".into(),
    })
    .unwrap();
    let failed = resend.send(&mail()).await.unwrap_err();
    assert_eq!(failed.kind, ModuleErrorKind::Unavailable);
}
