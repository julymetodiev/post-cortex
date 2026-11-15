// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Cards: versions with optimistic concurrency, history, and the
//! workspace card going with its workspace.

use uuid::Uuid;

use post_cortex_core::core::card::{Card, CardScope};

use crate::storage::{CardWrite, Stores};

fn written(write: CardWrite) -> Card {
    match write {
        CardWrite::Written(card) => card,
        CardWrite::Conflict(latest) => panic!("unexpected conflict, latest {latest:?}"),
    }
}

/// Every write is a new version; a write from a stale version is a conflict
/// carrying the latest; the user card is its own.
pub async fn versions_and_conflicts(stores: &Stores) {
    let scope = CardScope::Workspace(Uuid::new_v4());
    assert_eq!(stores.cards.get_card(scope).await.unwrap(), None);

    let v1 = written(
        stores
            .cards
            .put_card(scope, "stack: rust".into(), Some("claude".into()), 0)
            .await
            .unwrap(),
    );
    assert_eq!((v1.version, v1.author.as_deref()), (1, Some("claude")));

    // A writer that read no card is too late now.
    match stores
        .cards
        .put_card(scope, "other".into(), None, 0)
        .await
        .unwrap()
    {
        CardWrite::Conflict(Some(latest)) => assert_eq!(latest, v1),
        other => panic!("expected a conflict, got {other:?}"),
    }

    let v2 = written(
        stores
            .cards
            .put_card(scope, "stack: rust, surrealdb".into(), None, 1)
            .await
            .unwrap(),
    );
    assert_eq!(v2.version, 2);
    assert_eq!(
        stores.cards.get_card(scope).await.unwrap(),
        Some(v2.clone())
    );

    let history = stores.cards.card_history(scope, 10).await.unwrap();
    assert_eq!(history, vec![v2.clone(), v1], "newest first, all kept");
    assert_eq!(stores.cards.card_history(scope, 1).await.unwrap(), vec![v2]);

    // The user card is its own.
    assert_eq!(stores.cards.get_card(CardScope::User).await.unwrap(), None);
    let user = written(
        stores
            .cards
            .put_card(CardScope::User, "answers in Bulgarian".into(), None, 0)
            .await
            .unwrap(),
    );
    assert_eq!(user.version, 1);

    // A person's user card is neither the host's nor another person's, and
    // the store names the persons who have one.
    assert_eq!(
        stores.cards.card_people().await.unwrap(),
        Vec::<Uuid>::new()
    );
    let (ana, bob) = (Uuid::new_v4(), Uuid::new_v4());
    for (person, text) in [(ana, "short answers"), (bob, "long answers")] {
        let card = written(
            stores
                .cards
                .put_card(CardScope::Person(person), text.into(), None, 0)
                .await
                .unwrap(),
        );
        assert_eq!(card.version, 1);
    }
    let ana_card = stores.cards.get_card(CardScope::Person(ana)).await.unwrap();
    assert_eq!(ana_card.map(|c| c.text), Some("short answers".to_string()));
    assert_eq!(
        stores
            .cards
            .card_history(CardScope::User, 10)
            .await
            .unwrap()
            .len(),
        1
    );
    let mut people = stores.cards.card_people().await.unwrap();
    people.sort();
    let mut expected = vec![ana, bob];
    expected.sort();
    assert_eq!(people, expected);
}

/// Deleting a workspace deletes its card and history.
pub async fn deleting_the_workspace_deletes_its_card(stores: &Stores) {
    let workspace = Uuid::new_v4();
    stores
        .workspaces
        .save_workspace_metadata(workspace, "ws", &workspace.to_string(), "", &[])
        .await
        .unwrap();
    let scope = CardScope::Workspace(workspace);
    written(
        stores
            .cards
            .put_card(scope, "goals".into(), None, 0)
            .await
            .unwrap(),
    );
    stores.workspaces.delete_workspace(workspace).await.unwrap();
    assert_eq!(stores.cards.get_card(scope).await.unwrap(), None);
    assert!(
        stores
            .cards
            .card_history(scope, 10)
            .await
            .unwrap()
            .is_empty()
    );
}

/// Of two writers racing from the same version, one wins and one gets the
/// conflict.
pub async fn one_of_two_racing_writers_wins(stores: &Stores) {
    let scope = CardScope::Workspace(Uuid::new_v4());
    written(
        stores
            .cards
            .put_card(scope, "v1".into(), None, 0)
            .await
            .unwrap(),
    );
    let (a, b) = futures::join!(
        stores.cards.put_card(scope, "a".into(), None, 1),
        stores.cards.put_card(scope, "b".into(), None, 1),
    );
    let results = [a.unwrap(), b.unwrap()];
    let wins = results
        .iter()
        .filter(|w| matches!(w, CardWrite::Written(_)))
        .count();
    assert_eq!(wins, 1, "{results:?}");
    assert_eq!(
        stores.cards.get_card(scope).await.unwrap().unwrap().version,
        2
    );
}
