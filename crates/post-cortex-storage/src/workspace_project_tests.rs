// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Projects stored on workspace rows, run against every backend.

use crate::test_support::Backend;
use uuid::Uuid;

use post_cortex_core::workspace::{SessionRole, WorkspaceProject};

/// Projects round-trip, survive a metadata save and go with the workspace.
async fn projects_live_on_the_workspace_row(storage: &impl Backend) {
    let (workspace, session) = (Uuid::new_v4(), Uuid::new_v4());
    let missing = storage
        .save_workspace_projects(workspace, &[])
        .await
        .unwrap_err();
    assert!(missing.to_string().contains("not found"), "{missing}");

    storage
        .save_workspace_metadata(workspace, "ws", &workspace.to_string(), "d", &[])
        .await
        .unwrap();
    storage
        .add_session_to_workspace(workspace, session, SessionRole::Primary)
        .await
        .unwrap();
    let projects = vec![
        WorkspaceProject {
            key: "github.com/a/b".to_string(),
            session_id: session,
        },
        WorkspaceProject {
            key: "local:0123456789abcdef".to_string(),
            session_id: Uuid::new_v4(),
        },
    ];
    storage
        .save_workspace_projects(workspace, &projects)
        .await
        .unwrap();

    // A metadata save (rename, membership change) keeps the projects.
    storage
        .save_workspace_metadata(
            workspace,
            "renamed",
            &workspace.to_string(),
            "d",
            &[session],
        )
        .await
        .unwrap();
    let stored = storage.list_workspaces().await.unwrap();
    let stored = stored.iter().find(|w| w.id == workspace).unwrap();
    assert_eq!(stored.name, "renamed");
    assert_eq!(stored.projects, projects);
    assert_eq!(stored.sessions.len(), 1);

    storage.delete_workspace(workspace).await.unwrap();
    assert!(
        storage
            .list_workspaces()
            .await
            .unwrap()
            .iter()
            .all(|w| w.id != workspace)
    );
}

#[tokio::test]
async fn rocksdb_workspace_projects() {
    let dir = tempfile::tempdir().unwrap();
    let storage = crate::RealRocksDBStorage::new(dir.path()).await.unwrap();
    projects_live_on_the_workspace_row(&storage).await;
}

#[cfg(feature = "surrealdb-storage")]
#[tokio::test]
async fn surrealdb_workspace_projects() {
    let storage = crate::SurrealDBStorage::new("mem://", None, None, None, None)
        .await
        .unwrap();
    projects_live_on_the_workspace_row(&storage).await;
}
