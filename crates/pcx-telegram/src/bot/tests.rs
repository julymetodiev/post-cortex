// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The Bot API adapter against a fake Bot API on localhost: what it sends
//! as form fields and how it reads the answers.

use std::collections::HashMap;
use std::sync::Arc;

use axum::Router;
use axum::extract::{Form, Path, State};
use axum::routing::post;
use parking_lot::Mutex;

use super::*;

/// Each call the fake got: its method and form fields.
type Calls = Arc<Mutex<Vec<(String, HashMap<String, String>)>>>;

async fn fake(
    State(calls): State<Calls>,
    Path((_token, method)): Path<(String, String)>,
    Form(fields): Form<HashMap<String, String>>,
) -> String {
    calls.lock().push((method.clone(), fields));
    match method.as_str() {
        "getUpdates" => r#"{"ok": true, "result": [
            {"update_id": 7, "message": {"message_id": 3, "from": {"id": 42, "is_bot": false},
              "chat": {"id": -100, "type": "group"}, "text": "status?", "date": 1}},
            {"update_id": 8, "callback_query": {"id": "q1", "from": {"id": 42},
              "message": {"message_id": 9, "chat": {"id": -100}}, "data": "a:1", "chat_instance": "x"}}
        ]}"#
        .to_string(),
        "sendMessage" => r#"{"ok": true, "result": {"message_id": 11, "chat": {"id": -100}}}"#.to_string(),
        _ => r#"{"ok": false, "description": "Bad Request: message is not modified"}"#.to_string(),
    }
}

async fn api() -> (String, Calls) {
    let calls = Calls::default();
    let app = Router::new()
        .route("/{token}/{method}", post(fake))
        .with_state(Arc::clone(&calls));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await });
    (url, calls)
}

#[tokio::test]
async fn updates_are_read_from_the_bot_apis_json() {
    let (url, calls) = api().await;
    let bot = HttpBot::new(&url, "T0KEN").unwrap();
    let updates = bot.updates(7, 30).await.unwrap();
    assert_eq!(updates.len(), 2);
    let message = updates[0].message.as_ref().unwrap();
    assert_eq!(message.text, "status?");
    assert_eq!(message.chat.as_ref().unwrap().id, -100);
    let press = updates[1].callback_query.as_ref().unwrap();
    assert_eq!((press.id.as_str(), press.data.as_str()), ("q1", "a:1"));
    assert_eq!(press.from.as_ref().unwrap().id, 42);
    let (method, fields) = calls.lock()[0].clone();
    assert_eq!(method, "getUpdates");
    assert_eq!(fields["offset"], "7");
    assert_eq!(fields["timeout"], "30");
    assert_eq!(fields["allowed_updates"], r#"["message","callback_query"]"#);
}

#[tokio::test]
async fn a_message_goes_with_its_buttons_as_the_bot_api_reads_them() {
    let (url, calls) = api().await;
    let bot = HttpBot::new(&url, "T0KEN").unwrap();
    let buttons = [
        Button {
            text: "Approve".into(),
            callback_data: "a:1".into(),
        },
        Button {
            text: "Decline".into(),
            callback_data: "d:1".into(),
        },
    ];
    assert_eq!(bot.send(-100, "Waits for you", &buttons).await.unwrap(), 11);
    assert_eq!(bot.send(-100, "plain", &[]).await.unwrap(), 11);
    let calls = calls.lock().clone();
    let fields = &calls[0].1;
    assert_eq!(fields["chat_id"], "-100");
    assert_eq!(fields["text"], "Waits for you");
    assert_eq!(
        fields["reply_markup"],
        r#"{"inline_keyboard":[[{"text":"Approve","callback_data":"a:1"},{"text":"Decline","callback_data":"d:1"}]]}"#
    );
    assert!(
        !calls[1].1.contains_key("reply_markup"),
        "no buttons, no markup"
    );
}

#[tokio::test]
async fn a_call_the_bot_api_refuses_says_why() {
    let (url, _) = api().await;
    let bot = HttpBot::new(&url, "T0KEN").unwrap();
    let refused = bot.edit(-100, 9, "same", &[]).await.unwrap_err();
    assert_eq!(refused.kind, ModuleErrorKind::FailedPrecondition);
    assert!(refused.message.contains("message is not modified"));
    let gone = HttpBot::new("http://127.0.0.1:9", "T0KEN").unwrap();
    let unreachable = gone.answer("q", "x").await.unwrap_err();
    assert_eq!(unreachable.kind, ModuleErrorKind::Unavailable);
    assert!(
        !unreachable.message.contains("T0KEN"),
        "the token stays out of errors"
    );
}
