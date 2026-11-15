// Integration tests for Admin CLI functionality (Workspace and Session management)
use post_cortex::capabilities::CreateWorkspace;
use post_cortex::workspace::SessionRole;
use post_cortex::{Memory, SystemConfig};
use serial_test::serial;
use std::sync::Arc;
use tempfile::TempDir;

/// Memory with `config` over RocksDB in `data_directory`, composed as the
/// daemon composes it.
async fn open(data_directory: &str, config: SystemConfig) -> Result<Memory, String> {
    let storage = post_cortex::compose::StorageConfig::RocksDb {
        data_directory: data_directory.into(),
    };
    let models = post_cortex::compose::Models::new(&post_cortex::compose::ModelConfig::default());
    post_cortex::compose::open_memory(
        &storage,
        &models,
        None,
        config,
        post_cortex::PipelineConfig::default(),
    )
    .await
}

async fn create_test_system() -> (Arc<Memory>, TempDir) {
    let temp_dir = tempfile::tempdir().unwrap();
    let config = SystemConfig {
        enable_embeddings: false, // Admin tasks usually don't need embeddings active
        ..Default::default()
    };
    let system = open(temp_dir.path().to_str().unwrap(), config)
        .await
        .unwrap();
    (Arc::new(system), temp_dir)
}

#[serial]
#[tokio::test]
async fn test_full_admin_lifecycle() {
    let (system, _temp_dir) = create_test_system().await;

    // 1. Create Workspace
    let ws_name = "Admin Test Workspace";
    let ws_desc = "Testing admin capabilities";

    let workspace_id = system
        .workspaces
        .create(CreateWorkspace {
            name: ws_name.into(),
            description: ws_desc.into(),
        })
        .await
        .expect("Failed to create workspace")
        .id;

    // Verify workspace creation
    let workspaces = system.workspaces.list(true);
    assert_eq!(workspaces.len(), 1);
    assert_eq!(workspaces[0].id, workspace_id);
    assert_eq!(workspaces[0].name, ws_name);

    // 2. Create Session
    let session_name = "Admin Test Session";
    let session_id = system
        .sessions
        .create(Some(session_name.to_string()), None)
        .await
        .expect("Failed to create session");

    // Verify session creation
    let sessions = system
        .sessions
        .list()
        .await
        .expect("Failed to list sessions");
    assert!(sessions.contains(&session_id));

    // 3. Attach Session to Workspace
    system
        .workspaces
        .add_session(workspace_id, session_id, SessionRole::Primary)
        .await
        .expect("Failed to attach session to workspace");

    // Verify attachment
    let ws = system
        .workspaces
        .get(workspace_id)
        .expect("Workspace not found");
    assert_eq!(
        ws.get_all_sessions(),
        vec![(session_id, SessionRole::Primary)]
    );

    // 4. Delete Session
    let deleted = system
        .sessions
        .delete(session_id)
        .await
        .expect("Failed to execute delete_session");

    assert!(deleted, "Delete session should return true");

    // Verify session is gone
    let sessions_after_delete = system
        .sessions
        .list()
        .await
        .expect("Failed to list sessions");
    assert!(!sessions_after_delete.contains(&session_id));

    // Verify workspace reflects deletion (or at least session is gone from system)
    // Note: StoredWorkspace might still reference it unless we explicitly remove association.
    // The `RealRocksDBStorage::delete_session` implementation also cleans up `update:*` keys,
    // but does NOT automatically clean up `ws_session:*` keys unless specifically handled.
    // Looking at `RealRocksDBStorage::delete_session` implementation:
    // It deletes `session:*` and `update:*` but NOT `ws_session:*`.
    // This is a potential consistency issue to check, but for now we verify session is gone.

    // 5. Delete Workspace
    assert!(
        system
            .workspaces
            .delete(workspace_id)
            .await
            .expect("Failed to delete workspace")
    );

    // Verify workspace is gone
    assert!(system.workspaces.list(true).is_empty());
}

#[serial]
#[tokio::test]
async fn test_multiple_sessions_management() {
    let (system, _temp_dir) = create_test_system().await;

    // Create 5 sessions
    let mut session_ids = Vec::new();
    for i in 0..5 {
        let id = system
            .sessions
            .create(Some(format!("Session {}", i)), None)
            .await
            .unwrap();
        session_ids.push(id);
    }

    // List and verify count
    let list = system.sessions.list().await.unwrap();
    assert_eq!(list.len(), 5);

    // Create workspace
    let ws_id = system
        .workspaces
        .create(CreateWorkspace {
            name: "Multi WS".into(),
            description: "Test".into(),
        })
        .await
        .unwrap()
        .id;

    // Attach all to workspace
    for id in &session_ids {
        system
            .workspaces
            .add_session(ws_id, *id, SessionRole::Related)
            .await
            .unwrap();
    }

    // Verify attachment
    assert_eq!(system.workspaces.get(ws_id).unwrap().session_count(), 5);

    // Delete 3 sessions
    for id in &session_ids[..3] {
        system.sessions.delete(*id).await.unwrap();
    }

    // Verify remaining sessions
    let list_remaining = system.sessions.list().await.unwrap();
    assert_eq!(list_remaining.len(), 2);
    assert!(list_remaining.contains(&session_ids[3]));
    assert!(list_remaining.contains(&session_ids[4]));

    // Clean up workspace
    assert!(system.workspaces.delete(ws_id).await.unwrap());
    assert!(system.workspaces.list(true).is_empty());
}
