// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Tests of the workspace registry.

use std::sync::Arc;

use post_cortex_core::workspace::{SessionRole, WorkspaceProject};
use uuid::Uuid;

use crate::memory_system::workspace_registry::{RestoredWorkspace, WorkspaceManager};

/// A new workspace in `manager`, as the Workspaces capability adds one.
fn create(
    manager: &WorkspaceManager,
    name: impl Into<String>,
    description: impl Into<String>,
) -> Uuid {
    let id = Uuid::new_v4();
    manager.restore_workspace(RestoredWorkspace {
        id,
        name: name.into(),
        slug: id.to_string(),
        description: description.into(),
        sessions: Vec::new(),
        created_at: std::time::SystemTime::now(),
        projects: Vec::new(),
        archived_at: None,
        remote_rerank: false,
    });
    id
}

#[test]
fn test_workspace_creation() {
    let manager = WorkspaceManager::default();

    let ws_id = create(
        &manager,
        "Test Workspace".to_string(),
        "Testing workspace".to_string(),
    );

    let workspace = manager.get_workspace(&ws_id).unwrap();
    assert_eq!(workspace.name, "Test Workspace");
    assert_eq!(workspace.description, "Testing workspace");
    assert_eq!(manager.total_workspaces(), 1);
}

#[test]
fn test_session_management() {
    let manager = WorkspaceManager::default();
    let ws_id = create(&manager, "Test".to_string(), "".to_string());

    let session1 = Uuid::new_v4();
    let session2 = Uuid::new_v4();

    manager
        .add_session_to_workspace(&ws_id, session1, SessionRole::Primary)
        .unwrap();
    manager
        .add_session_to_workspace(&ws_id, session2, SessionRole::Related)
        .unwrap();

    let workspace = manager.get_workspace(&ws_id).unwrap();
    assert_eq!(workspace.session_count(), 2);
    assert_eq!(
        workspace.get_session_role(&session1),
        Some(SessionRole::Primary)
    );
    assert_eq!(
        workspace.get_session_role(&session2),
        Some(SessionRole::Related)
    );
}

#[test]
fn test_workspace_lookup_by_name() {
    let manager = WorkspaceManager::default();
    let ws_id = create(&manager, "MyWorkspace".to_string(), "".to_string());

    let workspace = manager.get_workspace_by_name("MyWorkspace").unwrap();
    assert_eq!(workspace.id, ws_id);
}

#[test]
fn test_project_binding() {
    let manager = WorkspaceManager::default();
    let ws_id = create(&manager, "p".to_string(), "".to_string());
    assert!(manager.get_workspace_by_project("github.com/a/b").is_none());

    let (first, second) = (Uuid::new_v4(), Uuid::new_v4());
    for session_id in [first, second] {
        manager
            .set_workspace_project(
                &ws_id,
                WorkspaceProject {
                    key: "github.com/a/b".to_string(),
                    session_id,
                },
            )
            .unwrap();
    }
    let workspace = manager.get_workspace_by_project("github.com/a/b").unwrap();
    assert_eq!(workspace.id, ws_id);
    // Rebinding a key replaces its session.
    assert_eq!(workspace.projects().len(), 1);
    assert_eq!(
        workspace.project("github.com/a/b").unwrap().session_id,
        second
    );
    assert!(
        manager
            .set_workspace_project(
                &Uuid::new_v4(),
                WorkspaceProject {
                    key: "x".to_string(),
                    session_id: first,
                },
            )
            .is_err()
    );

    manager.delete_workspace(&ws_id);
    assert!(manager.get_workspace_by_project("github.com/a/b").is_none());
}

#[test]
fn test_update_and_archive() {
    let manager = WorkspaceManager::default();
    let ws_id = create(&manager, "a".to_string(), "d".to_string());
    let other = create(&manager, "b".to_string(), "".to_string());
    let session = Uuid::new_v4();
    manager
        .add_session_to_workspace(&ws_id, session, SessionRole::Primary)
        .unwrap();

    let ws = manager
        .update_workspace(&ws_id, Some("renamed".to_string()), None, None)
        .unwrap();
    assert_eq!(
        (ws.name.as_str(), ws.description.as_str()),
        ("renamed", "d")
    );
    assert!(manager.get_workspace_by_name("a").is_none());
    assert_eq!(manager.get_workspace_by_name("renamed").unwrap().id, ws_id);
    // The swapped-in copy keeps the members.
    assert_eq!(ws.get_session_role(&session), Some(SessionRole::Primary));
    assert!(
        manager
            .update_workspace(&ws_id, Some("b".to_string()), None, None)
            .is_err(),
        "the name of another workspace"
    );
    assert_eq!(manager.get_workspace_by_name("b").unwrap().id, other);
    assert!(
        manager
            .update_workspace(&Uuid::new_v4(), None, None, None)
            .is_err()
    );

    let now = chrono::Utc::now();
    let (_, before) = manager.set_workspace_archived(&ws_id, Some(now)).unwrap();
    assert_eq!(before, None);
    assert_eq!(
        manager.get_workspace(&ws_id).unwrap().archived_at(),
        Some(now)
    );
    assert_eq!(manager.list_workspaces_filtered(false).len(), 1);
    assert_eq!(manager.list_workspaces_filtered(true).len(), 2);
    let (ws, before) = manager.set_workspace_archived(&ws_id, None).unwrap();
    assert_eq!(before, Some(now));
    assert!(!ws.is_archived());
    assert_eq!(manager.list_workspaces_filtered(false).len(), 2);
}

#[tokio::test]
async fn test_concurrent_workspace_operations() {
    let manager = Arc::new(WorkspaceManager::default());

    // Spawn 50 concurrent tasks creating workspaces
    let tasks: Vec<_> = (0..50)
        .map(|i| {
            let mgr = manager.clone();
            tokio::spawn(async move {
                let ws_id = create(&mgr, format!("Workspace {}", i), format!("Test {}", i));

                // Add 5 sessions to each workspace
                for _ in 0..5 {
                    let session_id = Uuid::new_v4();
                    mgr.add_session_to_workspace(&ws_id, session_id, SessionRole::Related)
                        .unwrap();
                }

                ws_id
            })
        })
        .collect();

    // Wait for all tasks
    let workspace_ids: Vec<_> = futures::future::join_all(tasks)
        .await
        .into_iter()
        .map(|r| r.unwrap())
        .collect();

    // Verify all workspaces created
    assert_eq!(manager.total_workspaces(), 50);

    // Verify each workspace has 5 sessions
    for ws_id in workspace_ids {
        let workspace = manager.get_workspace(&ws_id).unwrap();
        assert_eq!(workspace.session_count(), 5);
    }
}

/// A snapshot does not change when the workspace does (snapshots shared
/// their members and metadata, so a change showed through every copy and
/// rolling one back meant mutating a shared value).
#[test]
fn snapshots_do_not_change_afterwards() {
    let manager = WorkspaceManager::default();
    let ws_id = create(&manager, "w".to_string(), String::new());
    let before = manager.get_workspace(&ws_id).unwrap();
    let session = Uuid::new_v4();
    assert_eq!(
        manager
            .add_session_to_workspace(&ws_id, session, SessionRole::Primary)
            .unwrap(),
        None
    );
    assert_eq!(
        manager
            .add_session_to_workspace(&ws_id, session, SessionRole::Shared)
            .unwrap(),
        Some(SessionRole::Primary)
    );
    manager
        .set_workspace_archived(&ws_id, Some(chrono::Utc::now()))
        .unwrap();
    assert_eq!(before.session_count(), 0);
    assert!(!before.is_archived());
    let after = manager.get_workspace(&ws_id).unwrap();
    assert_eq!(after.get_session_role(&session), Some(SessionRole::Shared));
    assert!(after.is_archived());
    assert!(
        manager
            .remove_session_from_workspace(&ws_id, &Uuid::new_v4())
            .is_err()
    );
}
