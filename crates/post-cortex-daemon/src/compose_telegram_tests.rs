// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Telegram as the daemon composes it (ARCHITECTURE section 8, operations,
//! W29), against a fake Bot API on localhost: a decision that waits for a
//! person is sent to the approvals chat; a press of the person the module
//! acts for approves it on the real board as that person (the host admits
//! the person-only call for the persons its settings name); a stranger's
//! press approves nothing.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::Router;
use axum::extract::{Form, Path, State};
use axum::routing::post;
use pcx_board::BoardModule;
use pcx_host::Host;
use pcx_module_api::{Module, Scope};
use pcx_telegram::TelegramModule;
use post_cortex_memory::capabilities::CreateWorkspace;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

/// The fake Bot API: what was sent, and the updates to answer next.
#[derive(Default)]
struct Fake {
    sent: parking_lot::Mutex<Vec<HashMap<String, String>>>,
    answered: parking_lot::Mutex<Vec<String>>,
    updates: parking_lot::Mutex<Vec<Value>>,
}

async fn bot(
    State(fake): State<Arc<Fake>>,
    Path((_token, method)): Path<(String, String)>,
    Form(fields): Form<HashMap<String, String>>,
) -> String {
    match method.as_str() {
        "getUpdates" => {
            let updates = std::mem::take(&mut *fake.updates.lock());
            if updates.is_empty() {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            json!({"ok": true, "result": updates}).to_string()
        }
        "sendMessage" => {
            fake.sent.lock().push(fields);
            json!({"ok": true, "result": {"message_id": 55, "chat": {"id": -100}}}).to_string()
        }
        "answerCallbackQuery" => {
            fake.answered.lock().push(fields["text"].clone());
            json!({"ok": true}).to_string()
        }
        _ => json!({"ok": true}).to_string(),
    }
}

fn scope(author: &str) -> Scope {
    Scope {
        tenant: String::new(),
        profile: String::new(),
        session_id: None,
        workspace_id: None,
        author: Some(author.to_string()),
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

async fn call(host: &Host, author: &str, tool: &str, input: Value) -> Value {
    host.call_for_agent(&scope(author), tool, input)
        .await
        .unwrap()
        .data
        .unwrap()
}

/// Wait until `done` holds, or fail after five seconds.
async fn until(what: &str, mut done: impl FnMut() -> bool) {
    for _ in 0..100 {
        if done() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("{what} did not happen");
}

fn press(update: i64, from: i64, data: &str) -> Value {
    json!({"update_id": update, "callback_query": {"id": format!("q{update}"),
        "from": {"id": from}, "message": {"message_id": 55, "chat": {"id": -100}}, "data": data}})
}

#[tokio::test]
async fn a_press_of_the_person_it_acts_for_approves_the_decision_as_them() {
    let fake = Arc::new(Fake::default());
    let app = Router::new()
        .route("/{token}/{method}", post(bot))
        .with_state(Arc::clone(&fake));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let api = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await });

    let data = tempfile::tempdir().unwrap();
    let memory = Arc::new(
        crate::memory_tests::rocksdb_memory(
            &data,
            post_cortex_memory::SystemConfig {
                enable_embeddings: false,
                ..Default::default()
            },
        )
        .await
        .unwrap(),
    );
    let ws = memory
        .workspaces
        .create(CreateWorkspace {
            name: "Equal".into(),
            description: String::new(),
        })
        .await
        .unwrap()
        .id
        .to_string();
    let modules: Vec<Arc<dyn Module>> = vec![
        crate::memory_tests::memory_module(&memory).0,
        Arc::new(BoardModule::new()),
        Arc::new(TelegramModule::new()),
    ];
    let config = HashMap::from([(
        TelegramModule::ID.to_string(),
        json!({"bot_token": "T0KEN", "api_url": api, "approvals": "ops",
               "chats": [{"name": "ops", "chat_id": -100}],
               "people": [{"telegram_id": 42, "name": "julius"}],
               "persons": {"persons": ["julius"]}}),
    )]);
    let host = Arc::new(
        Host::start(modules, &config, &HashMap::new())
            .await
            .unwrap(),
    );
    let stop = CancellationToken::new();
    let feed = tokio::spawn(crate::compose::feed_memory_writes(
        Arc::clone(&host),
        Arc::clone(&memory),
        stop.clone(),
    ));
    let board = call(
        &host,
        "julius",
        "board_create",
        json!({"workspace_id": ws, "name": "eq-ops", "columns": ["Triage", "Done"]}),
    )
    .await;
    let task = call(
        &host,
        "eq-medic",
        "task_add",
        json!({"workspace_id": ws, "board": board["id"], "title": "svc-music crash-loops"}),
    )
    .await;
    // eq-medic is in no team: a person approves.
    let decision = call(
        &host,
        "eq-medic",
        "decide",
        json!({"workspace_id": ws, "task": task["id"], "what": "Restart svc-music",
               "why": "it crash-loops", "risk": "RISK_CODE"}),
    )
    .await;
    assert_eq!(decision["pending"], json!(true));

    until("the prompt", || !fake.sent.lock().is_empty()).await;
    let sent = fake.sent.lock()[0].clone();
    assert_eq!(sent["chat_id"], "-100");
    assert!(sent["text"].starts_with("Waits for you: Restart svc-music"));
    let markup: Value = serde_json::from_str(&sent["reply_markup"]).unwrap();
    let approve = markup["inline_keyboard"][0][0]["callback_data"]
        .as_str()
        .unwrap()
        .to_string();

    // A stranger's press, then julius's.
    fake.updates.lock().push(press(1, 7, &approve));
    until("the stranger's answer", || !fake.answered.lock().is_empty()).await;
    assert!(fake.answered.lock()[0].starts_with("You do not decide here"));
    fake.updates.lock().push(press(2, 42, &approve));
    until("julius's answer", || fake.answered.lock().len() == 2).await;
    assert_eq!(
        fake.answered.lock()[1],
        "Approved by julius: Restart svc-music"
    );

    let thread = call(
        &host,
        "julius",
        "task_get",
        json!({"workspace_id": ws, "task": task["id"]}),
    )
    .await;
    let approved = &thread["decisions"][0];
    assert_eq!(approved["pending"], json!(false));
    assert_eq!(
        approved["decision"]["approved_by"], "julius",
        "as the person who pressed"
    );
    assert_eq!(
        approved["decision"]["approval_note"],
        "approved in Telegram"
    );
    stop.cancel();
    feed.await.unwrap();
}
