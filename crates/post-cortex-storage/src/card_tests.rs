// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! [`CardStorage`] behaviour, run against every backend.

use std::sync::Arc;

use uuid::Uuid;

use crate::traits::{CardStorage, CardWrite, Storage};
use post_cortex_core::core::card::{Card, CardScope};

fn written(write: CardWrite) -> Card {
    match write {
        CardWrite::Written(card) => card,
        CardWrite::Conflict(latest) => panic!("unexpected conflict, latest {latest:?}"),
    }
}

async fn versions_and_conflicts(storage: &impl CardStorage) {
    let scope = CardScope::Workspace(Uuid::new_v4());
    assert_eq!(storage.get_card(scope).await.unwrap(), None);

    let v1 = written(
        storage
            .put_card(scope, "stack: rust".into(), Some("claude".into()), 0)
            .await
            .unwrap(),
    );
    assert_eq!((v1.version, v1.author.as_deref()), (1, Some("claude")));

    // A writer that read no card is too late now.
    match storage
        .put_card(scope, "other".into(), None, 0)
        .await
        .unwrap()
    {
        CardWrite::Conflict(Some(latest)) => assert_eq!(latest, v1),
        other => panic!("expected a conflict, got {other:?}"),
    }

    let v2 = written(
        storage
            .put_card(scope, "stack: rust, surrealdb".into(), None, 1)
            .await
            .unwrap(),
    );
    assert_eq!(v2.version, 2);
    assert_eq!(storage.get_card(scope).await.unwrap(), Some(v2.clone()));

    let history = storage.card_history(scope, 10).await.unwrap();
    assert_eq!(history, vec![v2.clone(), v1], "newest first, all kept");
    assert_eq!(storage.card_history(scope, 1).await.unwrap(), vec![v2]);

    // The user card is its own.
    assert_eq!(storage.get_card(CardScope::User).await.unwrap(), None);
    let user = written(
        storage
            .put_card(CardScope::User, "answers in Bulgarian".into(), None, 0)
            .await
            .unwrap(),
    );
    assert_eq!(user.version, 1);
}

async fn one_of_two_racing_writers_wins<S: CardStorage + 'static>(storage: Arc<S>) {
    let scope = CardScope::Workspace(Uuid::new_v4());
    written(storage.put_card(scope, "v1".into(), None, 0).await.unwrap());
    let a = tokio::spawn({
        let storage = storage.clone();
        async move { storage.put_card(scope, "a".into(), None, 1).await.unwrap() }
    });
    let b = tokio::spawn({
        let storage = storage.clone();
        async move { storage.put_card(scope, "b".into(), None, 1).await.unwrap() }
    });
    let results = [a.await.unwrap(), b.await.unwrap()];
    let wins = results
        .iter()
        .filter(|w| matches!(w, CardWrite::Written(_)))
        .count();
    assert_eq!(wins, 1, "{results:?}");
    assert_eq!(storage.get_card(scope).await.unwrap().unwrap().version, 2);
}

async fn deleting_the_workspace_deletes_its_card(storage: &impl Storage) {
    let workspace = Uuid::new_v4();
    storage
        .save_workspace_metadata(workspace, "ws", "", &[])
        .await
        .unwrap();
    let scope = CardScope::Workspace(workspace);
    written(
        storage
            .put_card(scope, "goals".into(), None, 0)
            .await
            .unwrap(),
    );
    storage.delete_workspace(workspace).await.unwrap();
    assert_eq!(storage.get_card(scope).await.unwrap(), None);
    assert!(storage.card_history(scope, 10).await.unwrap().is_empty());
}

#[tokio::test]
async fn rocksdb_cards() {
    let dir = tempfile::tempdir().unwrap();
    let storage = Arc::new(crate::RealRocksDBStorage::new(dir.path()).await.unwrap());
    versions_and_conflicts(storage.as_ref()).await;
    one_of_two_racing_writers_wins(storage.clone()).await;
    deleting_the_workspace_deletes_its_card(storage.as_ref()).await;
}

#[cfg(feature = "surrealdb-storage")]
#[tokio::test]
async fn surrealdb_cards() {
    let storage = Arc::new(
        crate::SurrealDBStorage::new("mem://", None, None, None, None)
            .await
            .unwrap(),
    );
    versions_and_conflicts(storage.as_ref()).await;
    one_of_two_racing_writers_wins(storage.clone()).await;
    deleting_the_workspace_deletes_its_card(storage.as_ref()).await;
}
