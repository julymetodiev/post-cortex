// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use super::*;
use crate::capabilities::UpdateWorkspace;
use crate::capabilities::{Memory, Target};
use crate::graph_views::ViewRequest;
use crate::memory_system::backends_tests::backends;

fn sorted(mut sessions: Vec<(Uuid, SessionRole)>) -> Vec<(Uuid, SessionRole)> {
    sessions.sort_by_key(|(id, _)| *id);
    sessions
}

async fn create(memory: &Memory, name: &str) -> Uuid {
    memory
        .workspaces
        .create(CreateWorkspace {
            name: name.into(),
            description: "d".into(),
        })
        .await
        .unwrap()
        .id
}

/// Adds with their roles, a role change and a removal are what a restart
/// finds.
#[tokio::test]
async fn membership_survives_a_restart() {
    for backend in backends().await {
        let memory = backend.memory().await;
        let ws = create(&memory, "ws").await;
        let related = memory.sessions.create(None, None).await.unwrap();
        let shared = memory.sessions.create(None, None).await.unwrap();
        let removed = memory.sessions.create(None, None).await.unwrap();
        for (sid, role) in [
            (related, SessionRole::Related),
            (shared, SessionRole::Primary),
            (removed, SessionRole::Primary),
        ] {
            memory.workspaces.add_session(ws, sid, role).await.unwrap();
        }
        memory
            .workspaces
            .add_session(ws, shared, SessionRole::Shared)
            .await
            .unwrap();
        assert_eq!(
            memory.workspaces.remove_session(ws, removed).await.unwrap(),
            SessionRole::Primary
        );
        let expected = sorted(vec![
            (related, SessionRole::Related),
            (shared, SessionRole::Shared),
        ]);
        let before = memory.workspaces.get(ws).unwrap();
        assert_eq!(sorted(before.get_all_sessions()), expected);
        drop(memory);

        let restarted = backend.memory().await;
        let after = restarted.workspaces.get(ws).unwrap();
        assert_eq!(
            sorted(after.get_all_sessions()),
            expected,
            "{}",
            backend.name
        );
    }
}

/// Unknown workspace or non-member: NotFound, and nothing is stored.
#[tokio::test]
async fn missing_workspace_or_member_is_not_found() {
    for backend in backends().await {
        let memory = backend.memory().await;
        let ghost = Uuid::new_v4();
        let sid = memory.sessions.create(None, None).await.unwrap();
        assert!(matches!(
            memory
                .workspaces
                .add_session(ghost, sid, SessionRole::Related)
                .await,
            Err(WorkspaceError::NotFound(_))
        ));
        let ws = create(&memory, "ws").await;
        assert!(matches!(
            memory.workspaces.remove_session(ws, sid).await,
            Err(WorkspaceError::NotFound(_))
        ));
        drop(memory);

        let restarted = backend.memory().await;
        assert!(restarted.workspaces.get(ghost).is_err());
        let ws = restarted.workspaces.get(ws).unwrap();
        assert!(ws.get_all_sessions().is_empty(), "{}", backend.name);
        assert!(restarted.workspaces.locks.is_empty());
    }
}

/// The merged graph of a workspace is dropped when its membership changes
/// and when it is deleted (review W4 item 9: the cache was process-global,
/// never evicted).
#[tokio::test]
async fn the_merged_graph_follows_membership_and_delete() {
    let memory = backends().await.remove(0).memory().await;
    let ws = create(&memory, "ws").await;
    let sid = memory.sessions.create(None, None).await.unwrap();
    memory
        .workspaces
        .add_session(ws, sid, SessionRole::Primary)
        .await
        .unwrap();
    let topics = || ViewRequest::parse("topics", &Default::default()).unwrap();
    memory
        .views
        .view(Target::Workspace(ws), topics())
        .await
        .unwrap();
    assert_eq!(memory.workspaces.graphs.len(), 1, "the merge is cached");

    let other = memory.sessions.create(None, None).await.unwrap();
    memory
        .workspaces
        .add_session(ws, other, SessionRole::Related)
        .await
        .unwrap();
    assert_eq!(memory.workspaces.graphs.len(), 0, "membership changed");

    memory
        .views
        .view(Target::Workspace(ws), topics())
        .await
        .unwrap();
    assert!(memory.workspaces.delete(ws).await.unwrap());
    assert_eq!(memory.workspaces.graphs.len(), 0, "deleted");
}

/// A slug is made once: a second workspace of one name gets the name's
/// slug with its id, a rename keeps it, a restart reads it, and a deleted
/// workspace's slug is free for the next one.
#[tokio::test]
async fn a_slug_is_made_once_and_kept() {
    for backend in backends().await {
        let memory = backend.memory().await;
        let first = create(&memory, "Checkout Service").await;
        let second = create(&memory, "checkout-service").await;
        let slug = |memory: &Memory, id| memory.workspaces.get(id).unwrap().slug.clone();
        assert_eq!(slug(&memory, first), "checkout-service");
        let hex = second.simple().to_string();
        assert_eq!(
            slug(&memory, second),
            format!("checkout-service-{}", &hex[..6])
        );
        memory
            .workspaces
            .update(
                first,
                UpdateWorkspace {
                    name: Some("Payments".into()),
                    description: None,
                    ..UpdateWorkspace::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(slug(&memory, first), "checkout-service");
        drop(memory);

        let restarted = backend.memory().await;
        assert_eq!(slug(&restarted, first), "checkout-service");
        assert_eq!(
            slug(&restarted, second),
            format!("checkout-service-{}", &hex[..6])
        );
        assert!(restarted.workspaces.delete(first).await.unwrap());
        let third = create(&restarted, "Checkout service").await;
        assert_eq!(slug(&restarted, third), "checkout-service");
        assert_eq!(
            slug(&restarted, second),
            format!("checkout-service-{}", &hex[..6])
        );
    }
}
