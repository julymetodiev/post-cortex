// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use super::*;
use post_cortex_core::core::card::CardScope;
use post_cortex_core::services::GetCardRequest;
use std::sync::Arc;

/// update_card stores the caller's author, else the default one.
#[tokio::test]
async fn card_author_defaults_to_the_caller() {
    let (memory, _dir) = crate::capabilities::test_support::rocksdb_memory().await;
    let memory = Arc::new(memory);
    let service = memory;
    let write = |version: u64, author: Option<&str>| {
        let mut body = serde_json::json!({
            "action": "update_card", "card": "user",
            "content": format!("v{}", version + 1), "expected_version": version,
        });
        if let Some(author) = author {
            body["author"] = serde_json::Value::String(author.to_string());
        }
        body
    };
    handle(
        &service,
        write(0, None),
        Some("claude-code".to_string()),
        None,
    )
    .await
    .unwrap();
    handle(
        &service,
        write(1, Some("julius")),
        Some("claude-code".to_string()),
        None,
    )
    .await
    .unwrap();

    let response = service
        .cards
        .get(GetCardRequest {
            scope: CardScope::User,
            history: 1,
        })
        .await
        .unwrap();
    let card = response.card.unwrap();
    assert_eq!(card.author.as_deref(), Some("julius"));
    assert_eq!(response.history[0].author.as_deref(), Some("claude-code"));
}

/// The user card of a call for a person is that person's: two persons and
/// the host each read and write their own (docs/memory.md, cards).
#[tokio::test]
async fn each_person_has_their_own_user_card() {
    let (memory, _dir) = crate::capabilities::test_support::rocksdb_memory().await;
    let (ana, bob) = (uuid::Uuid::new_v4(), uuid::Uuid::new_v4());
    for (person, text) in [
        (Some(ana), "ana's"),
        (Some(bob), "bob's"),
        (None, "the host's"),
    ] {
        let body = serde_json::json!({
            "action": "update_card", "card": "user", "content": text, "expected_version": 0,
        });
        handle(&memory, body, None, person).await.unwrap();
    }
    for (person, text) in [
        (Some(ana), "ana's"),
        (Some(bob), "bob's"),
        (None, "the host's"),
    ] {
        let body = serde_json::json!({ "action": "get_card", "card": "user" });
        handle(&memory, body, None, person).await.unwrap();
        let card = memory
            .cards
            .get(GetCardRequest {
                scope: CardScope::user_of(person),
                history: 0,
            })
            .await
            .unwrap()
            .card
            .unwrap();
        assert_eq!(card.text, text);
    }
}
