// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Workspaces: the creation time is kept by every save, members are the
//! same on every backend, a membership needs its workspace, and a delete
//! racing an update never resurrects the workspace; an update changes only
//! what it names.

use crate::storage::Stores;
use post_cortex_core::workspace::{SessionRole, WorkspaceProject};
use uuid::Uuid;

use crate::storage::StoredWorkspace;
use crate::storage::WorkspaceUpdate;

async fn stored(stores: &Stores, id: Uuid) -> Option<StoredWorkspace> {
    stores
        .workspaces
        .list_workspaces()
        .await
        .unwrap()
        .into_iter()
        .find(|w| w.id == id)
}

fn sorted(mut members: Vec<(Uuid, SessionRole)>) -> Vec<(Uuid, SessionRole)> {
    members.sort_by_key(|(id, _)| *id);
    members
}

/// A restored workspace is stored as given; renaming it keeps its slug,
/// creation time, projects, archive state, remote reranking and members.
pub async fn saves_keep_the_creation_time(stores: &Stores) {
    let (id, a, b) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
    let workspace = StoredWorkspace {
        id,
        name: "ws".into(),
        slug: "kept".into(),
        description: "d".into(),
        sessions: vec![(a, SessionRole::Related), (b, SessionRole::Dependency)],
        created_at: 1_000,
        projects: vec![WorkspaceProject {
            key: "github.com/o/r".into(),
            session_id: a,
        }],
        archived_at: Some(2_000),
        remote_rerank: true,
    };
    stores
        .workspaces
        .restore_workspace(&workspace)
        .await
        .unwrap();
    let got = stored(stores, id).await.unwrap();
    assert_eq!(got.created_at, 1_000);
    assert_eq!(got.slug, "kept");
    assert_eq!(got.archived_at, Some(2_000));
    assert!(got.remote_rerank);
    assert_eq!(got.projects, workspace.projects);
    assert_eq!(sorted(got.sessions), sorted(workspace.sessions.clone()));

    stores
        .workspaces
        .save_workspace_metadata(id, "renamed", "not-this", "d2", &[])
        .await
        .unwrap();
    let got = stored(stores, id).await.unwrap();
    assert_eq!((got.name.as_str(), got.created_at), ("renamed", 1_000));
    assert_eq!(got.slug, "kept");
    assert_eq!(got.archived_at, Some(2_000));
    assert!(got.remote_rerank);
    assert_eq!(sorted(got.sessions), sorted(workspace.sessions.clone()));

    // Restoring again replaces the members.
    let only_b = StoredWorkspace {
        sessions: vec![(b, SessionRole::Shared)],
        ..workspace
    };
    stores.workspaces.restore_workspace(&only_b).await.unwrap();
    let got = stored(stores, id).await.unwrap();
    assert_eq!(got.sessions, [(b, SessionRole::Shared)]);
}

/// Sessions named on save become `Primary` members; existing members
/// keep their role.
pub async fn save_adds_members(stores: &Stores) {
    let (id, a, b) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
    stores
        .workspaces
        .save_workspace_metadata(id, "ws", &id.to_string(), "", &[a])
        .await
        .unwrap();
    assert_eq!(
        stored(stores, id).await.unwrap().sessions,
        [(a, SessionRole::Primary)]
    );
    stores
        .workspaces
        .add_session_to_workspace(id, a, SessionRole::Related)
        .await
        .unwrap();
    stores
        .workspaces
        .save_workspace_metadata(id, "ws", &id.to_string(), "", &[a, b])
        .await
        .unwrap();
    assert_eq!(
        sorted(stored(stores, id).await.unwrap().sessions),
        sorted(vec![(a, SessionRole::Related), (b, SessionRole::Primary)])
    );
}

/// A session cannot join a workspace that does not exist.
pub async fn membership_needs_the_workspace(stores: &Stores) {
    let err = stores
        .workspaces
        .add_session_to_workspace(Uuid::new_v4(), Uuid::new_v4(), SessionRole::Primary)
        .await
        .unwrap_err();
    assert!(err.is_not_found(), "{err:#}");
}

/// Delete racing an update and a membership (the adapters run them on
/// separate threads): once all are done the
/// workspace is gone and leaves no member behind.
pub async fn delete_racing_updates_leaves_nothing(stores: &Stores) {
    for _ in 0..40 {
        let (id, member) = (Uuid::new_v4(), Uuid::new_v4());
        stores
            .workspaces
            .save_workspace_metadata(id, "ws", &id.to_string(), "", &[])
            .await
            .unwrap();
        let rename = WorkspaceUpdate {
            name: Some("renamed".into()),
            ..Default::default()
        };
        let (deleted, renamed, joined) = futures::join!(
            stores.workspaces.delete_workspace(id),
            stores.workspaces.update_workspace(id, &rename),
            stores
                .workspaces
                .add_session_to_workspace(id, member, SessionRole::Primary),
        );
        // SurrealDB may refuse a write that conflicts with a concurrent
        // one (retryable); RocksDB serialises them. Either way nothing
        // half-applied remains.
        if let Err(e) = deleted {
            assert!(retryable(&e), "{e:#}");
            stores.workspaces.delete_workspace(id).await.unwrap();
        }
        for result in [renamed, joined] {
            if let Err(e) = result {
                assert!(e.is_not_found() || retryable(&e), "{e:#}");
            }
        }
        assert!(stored(stores, id).await.is_none(), "resurrected");
        // A membership written after the delete would show up here.
        stores
            .workspaces
            .save_workspace_metadata(id, "again", &id.to_string(), "", &[])
            .await
            .unwrap();
        assert!(stored(stores, id).await.unwrap().sessions.is_empty());
    }
}

fn retryable(err: &crate::PortError) -> bool {
    format!("{err:#}").contains("can be retried")
}

/// An update changes only what it names; the archive state, remote
/// reranking (off for a new workspace), projects, members and creation
/// time stay through renames and metadata saves.
pub async fn updates_keep_the_rest_of_the_row(stores: &Stores) {
    let (workspace, session) = (Uuid::new_v4(), Uuid::new_v4());
    let missing = stores
        .workspaces
        .update_workspace(workspace, &WorkspaceUpdate::default())
        .await
        .unwrap_err();
    assert!(missing.is_not_found(), "{missing}");

    stores
        .workspaces
        .save_workspace_metadata(workspace, "ws", &workspace.to_string(), "d", &[])
        .await
        .unwrap();
    stores
        .workspaces
        .add_session_to_workspace(workspace, session, SessionRole::Related)
        .await
        .unwrap();
    let project = WorkspaceProject {
        key: "github.com/a/b".to_string(),
        session_id: session,
    };
    stores
        .workspaces
        .save_workspace_projects(workspace, std::slice::from_ref(&project))
        .await
        .unwrap();
    let stored = |all: Vec<StoredWorkspace>| all.into_iter().find(|w| w.id == workspace).unwrap();
    let created = stored(stores.workspaces.list_workspaces().await.unwrap());
    assert!(!created.remote_rerank);
    let created_at = created.created_at;

    stores
        .workspaces
        .update_workspace(
            workspace,
            &WorkspaceUpdate {
                name: Some("renamed".to_string()),
                archived_at: Some(Some(42)),
                remote_rerank: Some(true),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let ws = stored(stores.workspaces.list_workspaces().await.unwrap());
    assert_eq!(
        (ws.name.as_str(), ws.description.as_str()),
        ("renamed", "d")
    );
    assert_eq!(ws.archived_at, Some(42));
    assert!(ws.remote_rerank);
    assert_eq!(ws.projects, std::slice::from_ref(&project));
    assert_eq!(ws.sessions, [(session, SessionRole::Related)]);
    assert_eq!(ws.created_at, created_at);

    // A metadata save (e.g. on creation elsewhere) keeps the archive state
    // and remote reranking.
    stores
        .workspaces
        .save_workspace_metadata(workspace, "renamed", &workspace.to_string(), "d2", &[])
        .await
        .unwrap();
    let ws = stored(stores.workspaces.list_workspaces().await.unwrap());
    assert_eq!((ws.archived_at, ws.remote_rerank), (Some(42), true));

    stores
        .workspaces
        .update_workspace(
            workspace,
            &WorkspaceUpdate {
                description: Some("new".to_string()),
                archived_at: Some(None),
                remote_rerank: Some(false),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let ws = stored(stores.workspaces.list_workspaces().await.unwrap());
    assert_eq!(
        (
            ws.name.as_str(),
            ws.description.as_str(),
            ws.archived_at,
            ws.remote_rerank
        ),
        ("renamed", "new", None, false)
    );
}
