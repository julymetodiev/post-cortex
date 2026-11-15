// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use super::*;
use crate::capabilities::CreateWorkspace;
use crate::capabilities::test_support::rocksdb_memory;

/// An unknown workspace is refused, versions advance, a stale version
/// conflicts with the latest returned, secrets are redacted and the size
/// limit holds.
#[tokio::test]
async fn cards_are_versioned_redacted_and_bounded() {
    let (memory, _dir) = rocksdb_memory().await;
    let cards = &memory.cards;
    let update = |scope, text: &str, expected_version| UpdateCardRequest {
        scope,
        text: text.to_string(),
        expected_version,
        author: Some("claude".to_string()),
    };

    let unknown = CardScope::Workspace(Uuid::new_v4());
    assert!(matches!(
        cards.update(update(unknown, "x", 0)).await,
        Err(CardError::WorkspaceNotFound(_))
    ));

    let ws = memory
        .workspaces
        .create(CreateWorkspace {
            name: "cards".into(),
            description: String::new(),
        })
        .await
        .unwrap()
        .id;
    let scope = CardScope::Workspace(ws);
    let first = cards
        .update(update(
            scope,
            "## Stack\nrust; key AKIAABCDEFGHIJKLMNOP\n\n",
            0,
        ))
        .await
        .unwrap();
    assert!(first.written);
    let card = first.card.unwrap();
    assert_eq!(card.version, 1);
    assert!(card.text.contains("[REDACTED:"), "{}", card.text);
    assert!(
        !card.text.ends_with('\n'),
        "trailing blank lines are trimmed"
    );

    let stale = cards.update(update(scope, "other", 0)).await.unwrap();
    assert!(!stale.written);
    assert_eq!(stale.card.unwrap().version, 1, "the latest comes back");

    assert!(matches!(
        cards
            .update(update(scope, &"x".repeat(MAX_CARD_BYTES + 1), 1))
            .await,
        Err(CardError::InvalidArgument(_))
    ));
    assert!(
        cards
            .update(update(scope, "## Stack\nrust", 1))
            .await
            .unwrap()
            .written
    );

    let read = cards
        .get(GetCardRequest { scope, history: 5 })
        .await
        .unwrap();
    assert_eq!(read.card.unwrap().version, 2);
    assert_eq!(read.history.len(), 1);
    let user = cards
        .get(GetCardRequest {
            scope: CardScope::User,
            history: 0,
        })
        .await
        .unwrap();
    assert!(user.card.is_none());
}
