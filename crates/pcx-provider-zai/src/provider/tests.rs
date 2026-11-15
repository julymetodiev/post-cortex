// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The provider against a local endpoint that answers as Z.AI does.

use std::time::{Duration, Instant};

use pcx_module_api::model::{Block, ModelMessage, Role, StopReason, ToolChoice};
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

use pcx_module_api::Scope;
use pcx_module_api::model::KeyedProvider;
use pcx_module_api::model::{ModelFailureKind, ModelProvider, ModelRequest};
use pcx_openai_compat::{Endpoint, served};

use super::*;

/// A local endpoint that answers one request with `response`; the request
/// it read.
/// A server on a `/chat/completions` URL that answers its first request
/// `response`, after a 404 to a listing (`GET`) when `listing` is off.
async fn serve(response: String, listing: bool) -> (String, tokio::task::JoinHandle<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/chat/completions", listener.local_addr().unwrap());
    let served = tokio::spawn(async move {
        loop {
            let (mut socket, _) = listener.accept().await.unwrap();
            let text = read_request(&mut socket).await;
            if !listing && text.starts_with("GET ") {
                let none = "HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\n\r\n";
                socket.write_all(none.as_bytes()).await.unwrap();
                socket.shutdown().await.unwrap();
                continue;
            }
            socket.write_all(response.as_bytes()).await.unwrap();
            socket.shutdown().await.unwrap();
            return text;
        }
    });
    (url, served)
}

/// The headers, then the body its length names.
async fn read_request(socket: &mut tokio::net::TcpStream) -> String {
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

async fn endpoint(response: String) -> (String, tokio::task::JoinHandle<String>) {
    serve(response, false).await
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

fn request() -> ModelRequest {
    ModelRequest {
        model: "glm-5.3".into(),
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

/// The provider started on `url`, whose listing (the first request) the
/// test's server does not answer: the table's models.
async fn provider(url: &str) -> Provider {
    let provider = super::provider();
    provider.start(url, "key-1").await.unwrap();
    provider
}

#[tokio::test]
async fn an_answer_streams_from_the_endpoint() {
    let events = [
        json!({ "choices": [{ "delta": { "reasoning_content": "Greet." } }] }),
        json!({ "choices": [{ "delta": { "content": "Hello" } }] }),
        json!({ "choices": [{ "delta": {}, "finish_reason": "stop" }],
                "usage": { "prompt_tokens": 5, "completion_tokens": 2 } }),
    ];
    let mut sse: String = events.iter().map(|e| format!("data: {e}\n\n")).collect();
    sse.push_str("data: [DONE]\n\n");
    let response = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\n\r\n{sse}",
        sse.len()
    );
    let (url, served) = endpoint(response).await;
    let (blocks, usage, stop) = provider(&url)
        .await
        .generate(&scope(), request())
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    assert_eq!(
        blocks,
        [
            Block::Thinking {
                text: "Greet.".into(),
                signature: None
            },
            Block::Text {
                text: "Hello".into()
            }
        ]
    );
    assert_eq!(
        (usage.input, usage.output, stop),
        (5, 2, StopReason::EndTurn)
    );
    let sent = served.await.unwrap();
    assert!(sent.contains("authorization: Bearer key-1"), "{sent}");
    assert!(sent.contains("\"enable_thinking\":true"), "{sent}");
}

#[tokio::test]
async fn a_used_up_plan_is_not_a_rate_limit() {
    let body = json!({ "error": { "code": "1308", "message": "Usage limit reached" } }).to_string();
    let response = format!(
        "HTTP/1.1 429 Too Many Requests\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{body}",
        body.len()
    );
    let (url, _served) = endpoint(response).await;
    let failed = provider(&url)
        .await
        .generate(&scope(), request())
        .await
        .err()
        .unwrap();
    assert_eq!(failed.kind, ModelFailureKind::QuotaExhausted);

    let busy = json!({ "error": { "code": "1302", "message": "Rate limit" } }).to_string();
    let response = format!(
        "HTTP/1.1 429 Too Many Requests\r\nretry-after: 7\r\ncontent-length: {}\r\n\r\n{busy}",
        busy.len()
    );
    let (url, _served) = endpoint(response).await;
    let limited = provider(&url)
        .await
        .generate(&scope(), request())
        .await
        .err()
        .unwrap();
    assert_eq!(limited.kind, ModelFailureKind::RateLimited);
    assert_eq!(limited.retry_after, Some(Duration::from_secs(7)));
}

fn answer(status: &str, body: &str) -> String {
    format!(
        "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{body}",
        body.len()
    )
}

#[tokio::test]
async fn its_listing_names_the_models_and_the_table_describes_them() {
    let listing = json!({"object": "list", "data": [
        {"id": "glm-5.3", "object": "model"}, {"id": "glm-5.3-flash", "object": "model"}
    ]})
    .to_string();
    let (url, sent) = serve(answer("200 OK", &listing), true).await;
    let endpoint = Endpoint::new(&url, "key-1", catalog::codes()).unwrap();
    let served = endpoint.models().await.map(|l| served(&l));
    let models = catalog::table().listed(ID, served).unwrap();
    let sent = sent.await.unwrap();
    assert!(sent.starts_with("GET /models "), "{sent}");
    let listed: Vec<(&str, u64)> = models
        .iter()
        .map(|m| (m.id.as_str(), m.context_tokens))
        .collect();
    assert_eq!(listed, [("glm-5.3", 1_048_576), ("glm-5.3-flash", 0)]);
}

#[tokio::test]
async fn a_refused_key_fails_its_listing() {
    let refused = json!({ "error": { "code": "1000", "message": "bad key" } }).to_string();
    let (url, _sent) = serve(answer("401 Unauthorized", &refused), true).await;
    let endpoint = Endpoint::new(&url, "key-1", catalog::codes()).unwrap();
    let served = endpoint.models().await.map(|l| served(&l));
    let failed = catalog::table().listed(ID, served).unwrap_err();
    assert_eq!(failed.kind, ModelFailureKind::Unauthorized);
}
