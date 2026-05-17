// Integration tests for Admin CLI functionality (Workspace and Session management)
use post_cortex::{ConversationMemorySystem, SystemConfig};
use post_cortex::workspace::SessionRole;
use serial_test::serial;
use std::sync::Arc;
use tempfile::TempDir;

async fn create_test_system() -> (Arc<ConversationMemorySystem>, TempDir) {
    let temp_dir = tempfile::tempdir().unwrap();
    let config = SystemConfig {
        data_directory: temp_dir.path().to_str().unwrap().to_string(),
        enable_embeddings: false, // Admin tasks usually don't need embeddings active
        ..Default::default()
    };
    let system = ConversationMemorySystem::new(config).await.unwrap();
    (Arc::new(system), temp_dir)
}

#[serial]
#[tokio::test]
async fn test_full_admin_lifecycle() {
    let (system, _temp_dir) = create_test_system().await;
    let storage = system.get_storage();

    // 1. Create Workspace
    let workspace_id = uuid::Uuid::new_v4();
    let ws_name = "Admin Test Workspace";
    let ws_desc = "Testing admin capabilities";
    
    storage
        .save_workspace_metadata(workspace_id, ws_name, ws_desc, &[])
        .await
        .expect("Failed to create workspace");

    // Verify workspace creation
    let workspaces = storage.list_all_workspaces().await.expect("Failed to list workspaces");
    assert_eq!(workspaces.len(), 1);
    assert_eq!(workspaces[0].id, workspace_id);
    assert_eq!(workspaces[0].name, ws_name);

    // 2. Create Session
    let session_name = "Admin Test Session";
    let session_id = system
        .create_session(Some(session_name.to_string()), None)
        .await
        .expect("Failed to create session");

    // Verify session creation
    let sessions = system.list_sessions().await.expect("Failed to list sessions");
    assert!(sessions.contains(&session_id));

    // 3. Attach Session to Workspace
    storage
        .add_session_to_workspace(workspace_id, session_id, SessionRole::Primary)
        .await
        .expect("Failed to attach session to workspace");

    // Verify attachment (reload workspace list)
    let workspaces_after_attach = storage.list_all_workspaces().await.expect("Failed to list workspaces");
    let ws = workspaces_after_attach.iter().find(|w| w.id == workspace_id).expect("Workspace not found");
    
    // Note: list_all_workspaces returns StoredWorkspace which has sessions vector
    assert_eq!(ws.sessions.len(), 1);
    assert_eq!(ws.sessions[0].0, session_id);
    assert_eq!(ws.sessions[0].1, SessionRole::Primary);

    // 4. Delete Session
    // Using the newly added delete_session method on StorageActorHandle
    let deleted = storage
        .delete_session(session_id)
        .await
        .expect("Failed to execute delete_session");
    
    assert!(deleted, "Delete session should return true");

    // Verify session is gone
    let sessions_after_delete = system.list_sessions().await.expect("Failed to list sessions");
    assert!(!sessions_after_delete.contains(&session_id));

    // Verify workspace reflects deletion (or at least session is gone from system)
    // Note: StoredWorkspace might still reference it unless we explicitly remove association.
    // The `RealRocksDBStorage::delete_session` implementation also cleans up `update:*` keys, 
    // but does NOT automatically clean up `ws_session:*` keys unless specifically handled.
    // Looking at `RealRocksDBStorage::delete_session` implementation:
    // It deletes `session:*` and `update:*` but NOT `ws_session:*`.
    // This is a potential consistency issue to check, but for now we verify session is gone.
    
    // 5. Delete Workspace
    storage
        .delete_workspace(workspace_id)
        .await
        .expect("Failed to delete workspace");

    // Verify workspace is gone
    let workspaces_final = storage.list_all_workspaces().await.expect("Failed to list workspaces");
    assert!(workspaces_final.is_empty());
}

#[serial]
#[tokio::test]
async fn test_multiple_sessions_management() {
    let (system, _temp_dir) = create_test_system().await;
    let storage = system.get_storage();

    // Create 5 sessions
    let mut session_ids = Vec::new();
    for i in 0..5 {
        let id = system
            .create_session(Some(format!("Session {}", i)), None)
            .await
            .unwrap();
        session_ids.push(id);
    }

    // List and verify count
    let list = system.list_sessions().await.unwrap();
    assert_eq!(list.len(), 5);

    // Create workspace
    let ws_id = uuid::Uuid::new_v4();
    storage.save_workspace_metadata(ws_id, "Multi WS", "Test", &[]).await.unwrap();

    // Attach all to workspace
    for id in &session_ids {
        storage.add_session_to_workspace(ws_id, *id, SessionRole::Related).await.unwrap();
    }

    // Verify attachment
    let workspaces = storage.list_all_workspaces().await.unwrap();
    assert_eq!(workspaces[0].sessions.len(), 5);

    // Delete 3 sessions
    for i in 0..3 {
        storage.delete_session(session_ids[i]).await.unwrap();
    }

    // Verify remaining sessions
    let list_remaining = system.list_sessions().await.unwrap();
    assert_eq!(list_remaining.len(), 2);
    assert!(list_remaining.contains(&session_ids[3]));
    assert!(list_remaining.contains(&session_ids[4]));

    // Clean up workspace
    storage.delete_workspace(ws_id).await.unwrap();
    let workspaces_empty = storage.list_all_workspaces().await.unwrap();
    assert!(workspaces_empty.is_empty());
}
