// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use std::time::{Duration, Instant};

use pcx_module_api::model::{Block, ModelMessage, Role, StopReason, ToolChoice};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::*;

/// The headers, then the body its length names.
async fn read_request(socket: &mut TcpStream) -> String {
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

fn answer(status: &str, kind: &str, body: &str) -> String {
    format!(
        "HTTP/1.1 {status}\r\ncontent-type: {kind}\r\ncontent-length: {}\r\n\r\n{body}",
        body.len()
    )
}

/// A server that answers a listing with `listing` (its status and body)
/// and a message with `stream`; what each request sent, in order.
async fn api(
    listing: (&'static str, String),
    stream: String,
) -> (String, mpsc::UnboundedReceiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let (tx, rx) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        loop {
            let (mut socket, _) = listener.accept().await.unwrap();
            let sent = read_request(&mut socket).await;
            let response = if sent.starts_with("GET /v1/models") {
                answer(listing.0, "application/json", &listing.1)
            } else {
                answer("200 OK", "text/event-stream", &stream)
            };
            socket.write_all(response.as_bytes()).await.unwrap();
            socket.shutdown().await.unwrap();
            if tx.send(sent).is_err() {
                return;
            }
        }
    });
    (url, rx)
}

fn sse(events: &[Value]) -> String {
    events
        .iter()
        .map(|e| format!("event: {}\ndata: {e}\n\n", e["type"].as_str().unwrap()))
        .collect()
}

fn scope() -> Scope {
    Scope {
        tenant: String::new(),
        profile: String::new(),
        session_id: None,
        workspace_id: None,
        author: None,
        caller_module: None,
        call_depth: 0,
        turn: None,
        reach: None,
        role: None,
        read_only: false,
        person: None,
        deadline: Instant::now() + Duration::from_secs(10),
        cancel: CancellationToken::new(),
    }
}

fn request(model: &str) -> ModelRequest {
    ModelRequest {
        model: model.into(),
        system: String::new(),
        messages: vec![ModelMessage {
            role: Role::User,
            blocks: vec![Block::Text { text: "Hi".into() }],
        }],
        tools: Vec::new(),
        max_tokens: 50,
        tool_choice: ToolChoice::Auto,
    }
}

#[tokio::test]
async fn its_listing_at_start_then_an_answer_with_adaptive_thinking() {
    let listing = json!({
        "data": [
            {"id": "claude-opus-5-5", "display_name": "Claude Opus 5.5",
             "max_input_tokens": 1_000_000, "max_tokens": 128_000,
             "capabilities": {"thinking": {"supported": true, "types": {"adaptive": {"supported": true}}}}},
            {"id": "claude-haiku-4-5", "display_name": "Claude Haiku 4.5",
             "max_input_tokens": 200_000, "max_tokens": 64_000,
             "capabilities": {"thinking": {"supported": true, "types": {"adaptive": {"supported": false}}}}}
        ],
        "has_more": false, "last_id": "claude-haiku-4-5"
    })
    .to_string();
    let stream = sse(&[
        json!({"type": "message_start", "message": {"usage": {"input_tokens": 5, "output_tokens": 1}}}),
        json!({"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}}),
        json!({"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "Hello"}}),
        json!({"type": "message_delta", "delta": {"stop_reason": "end_turn"}, "usage": {"output_tokens": 2}}),
        json!({"type": "message_stop"}),
    ]);
    let (url, mut sent) = api(("200 OK", listing), stream).await;
    let provider = Anthropic::default();
    provider.start(&url, "key-1").await.unwrap();
    let listed = sent.recv().await.unwrap();
    assert!(listed.starts_with("GET /v1/models?limit=1000 "), "{listed}");
    assert!(listed.contains("x-api-key: key-1"), "{listed}");
    assert!(listed.contains("anthropic-version: 2026-10-21"), "{listed}");

    // Its table describes the model it knows; the other as listed.
    let models = provider.listed().await.unwrap();
    let ids: Vec<(&str, u64)> = models
        .iter()
        .map(|m| (m.id.as_str(), m.context_tokens))
        .collect();
    assert_eq!(
        ids,
        [
            ("claude-opus-5-5", 1_000_000),
            ("claude-haiku-4-5", 200_000)
        ]
    );
    assert!(models[0].price.is_some() && models[1].price.is_none());

    let (blocks, usage, stop) = provider
        .generate(&scope(), request("claude-opus-5-5"))
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    assert_eq!(
        blocks,
        [Block::Text {
            text: "Hello".into()
        }]
    );
    assert_eq!(
        (usage.input, usage.output, stop),
        (5, 2, StopReason::EndTurn)
    );
    let asked = sent.recv().await.unwrap();
    assert!(asked.starts_with("POST /v1/messages "), "{asked}");
    assert!(
        asked.contains("\"thinking\":{\"display\":\"summarized\",\"type\":\"adaptive\"}"),
        "{asked}"
    );

    // A model that does not think adaptively is not asked to.
    provider
        .generate(&scope(), request("claude-haiku-4-5"))
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    let asked = sent.recv().await.unwrap();
    assert!(!asked.contains("\"thinking\""), "{asked}");
}

#[tokio::test]
async fn a_refused_key_fails_its_listing_and_an_api_down_leaves_the_table() {
    let refused = json!({"type": "error", "error": {"type": "authentication_error", "message": "invalid x-api-key"}});
    let (url, _sent) = api(("401 Unauthorized", refused.to_string()), String::new()).await;
    let provider = Anthropic::default();
    // A start does not wait on the API: the refusal is its listing's.
    provider.start(&url, "bad").await.unwrap();
    let failed = provider.listed().await.unwrap_err();
    assert_eq!(failed.kind, ModuleErrorKind::InvalidArgument);
    assert_eq!(failed.field.as_deref(), Some("api_key"));

    let down = json!({"type": "error", "error": {"type": "api_error", "message": "down"}});
    let (url, _sent) = api(
        ("500 Internal Server Error", down.to_string()),
        String::new(),
    )
    .await;
    let provider = Anthropic::default();
    provider.start(&url, "key-1").await.unwrap();
    assert_eq!(
        provider.listed().await.unwrap().len(),
        catalog::table().known.len()
    );
}
