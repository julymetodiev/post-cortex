// Integration tests for workspace functionality with real RocksDB
use post_cortex::{ConversationMemorySystem, SystemConfig};
use serial_test::serial;
use std::sync::Arc;
use tempfile::TempDir;

async fn create_test_system() -> (Arc<ConversationMemorySystem>, TempDir) {
    let temp_dir = tempfile::tempdir().unwrap();
    let config = SystemConfig {
        data_directory: temp_dir.path().to_str().unwrap().to_string(),
        ..Default::default()
    };
    let system = ConversationMemorySystem::new(config).await.unwrap();
    (Arc::new(system), temp_dir)
}

#[serial]
#[tokio::test]
// #[ignore] // TODO: Implement workspace persistence in RocksDB
async fn test_workspace_create_and_persist() {
    let temp_dir = tempfile::tempdir().unwrap();
    let data_dir = temp_dir.path().to_str().unwrap().to_string();

    let workspace_id = {
        // Scope 1: Create system and workspace
        let config = SystemConfig {
            data_directory: data_dir.clone(),
            ..Default::default()
        };
        let system = ConversationMemorySystem::new(config).await.unwrap();

        // Create workspace
        let workspace_id = system
            .workspace_manager
            .create_workspace("Test Workspace".to_string(), "Real test".to_string());

        // Persist workspace
        system.save_workspace_metadata(
            workspace_id,
            "Test Workspace",
            "Real test",
            &[],
        ).await.unwrap();

        // Verify in memory
        let workspace = system.workspace_manager.get_workspace(&workspace_id);
        assert!(workspace.is_some());
        assert_eq!(workspace.unwrap().name, "Test Workspace");

        workspace_id
        // system drops here, closing RocksDB
    };

    // Small delay to ensure RocksDB fully closes
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    // Scope 2: Reopen and verify persistence
    {
        let config2 = SystemConfig {
            data_directory: data_dir,
            ..Default::default()
        };
        let system2 = ConversationMemorySystem::new(config2).await.unwrap();
        let workspace2 = system2.workspace_manager.get_workspace(&workspace_id);
        assert!(workspace2.is_some());
        assert_eq!(workspace2.unwrap().name, "Test Workspace");
    }
}

#[serial]
#[tokio::test]
async fn test_concurrent_workspace_and_session_operations() {
    let (system, _temp_dir) = create_test_system().await;

    // Spawn 20 concurrent tasks
    let tasks: Vec<_> = (0..20)
        .map(|i| {
            let sys = system.clone();
            tokio::spawn(async move {
                // Create workspace
                let ws_id = sys.workspace_manager.create_workspace(
                    format!("Workspace {}", i),
                    format!("Test workspace {}", i),
                );

                // Create 3 sessions
                let session1 = sys
                    .create_session(Some(format!("service-{}-auth", i)), None)
                    .await
                    .unwrap();
                let session2 = sys
                    .create_session(Some(format!("service-{}-payment", i)), None)
                    .await
                    .unwrap();
                let session3 = sys
                    .create_session(Some(format!("service-{}-products", i)), None)
                    .await
                    .unwrap();

                // Add sessions to workspace
                sys.workspace_manager
                    .add_session_to_workspace(
                        &ws_id,
                        session1,
                        post_cortex::workspace::SessionRole::Primary,
                    )
                    .unwrap();
                sys.workspace_manager
                    .add_session_to_workspace(
                        &ws_id,
                        session2,
                        post_cortex::workspace::SessionRole::Related,
                    )
                    .unwrap();
                sys.workspace_manager
                    .add_session_to_workspace(
                        &ws_id,
                        session3,
                        post_cortex::workspace::SessionRole::Related,
                    )
                    .unwrap();

                (ws_id, session1, session2, session3)
            })
        })
        .collect();

    // Wait for all tasks - if deadlock occurs, this will timeout
    let results = futures::future::join_all(tasks).await;

    // Verify all completed successfully
    assert_eq!(results.len(), 20);
    for result in results {
        assert!(result.is_ok());
    }

    // Verify total counts
    assert_eq!(system.workspace_manager.total_workspaces(), 20);

    // Verify each workspace has 3 sessions
    let workspaces = system.workspace_manager.list_workspaces();
    for workspace in workspaces {
        assert_eq!(workspace.session_ids.len(), 3);
    }
}

#[serial]
#[tokio::test]
async fn test_rocksdb_single_instance_enforcement() {
    let temp_dir = tempfile::tempdir().unwrap();
    let data_dir = temp_dir.path().to_str().unwrap().to_string();

    // Open first instance
    let config1 = SystemConfig {
        data_directory: data_dir.clone(),
        ..Default::default()
    };
    let system1 = ConversationMemorySystem::new(config1).await.unwrap();

    // Try to open second instance - should fail with lock error
    let config2 = SystemConfig {
        data_directory: data_dir.clone(),
        ..Default::default()
    };
    let result = ConversationMemorySystem::new(config2).await;

    assert!(result.is_err());
    if let Err(error_msg) = result {
        let error_msg_lower = error_msg.to_lowercase();
        // RocksDB lock error contains "lock" or "io error"
        assert!(
            error_msg_lower.contains("lock") || error_msg_lower.contains("io error"),
            "Expected lock error, got: {}",
            error_msg
        );
    }

    drop(system1);
}

#[serial]
#[tokio::test]
async fn test_workspace_session_relationships() {
    let (system, _temp_dir) = create_test_system().await;

    // Create workspace
    let ws_id = system
        .workspace_manager
        .create_workspace("Microservices".to_string(), "E-commerce platform".to_string());

    // Create sessions
    let auth_session = system
        .create_session(Some("auth-service".to_string()), None)
        .await
        .unwrap();
    let payment_session = system
        .create_session(Some("payment-service".to_string()), None)
        .await
        .unwrap();
    let shared_lib = system
        .create_session(Some("shared-utils".to_string()), None)
        .await
        .unwrap();

    // Add with different roles
    system
        .workspace_manager
        .add_session_to_workspace(
            &ws_id,
            auth_session,
            post_cortex::workspace::SessionRole::Primary,
        )
        .unwrap();
    system
        .workspace_manager
        .add_session_to_workspace(
            &ws_id,
            payment_session,
            post_cortex::workspace::SessionRole::Related,
        )
        .unwrap();
    system
        .workspace_manager
        .add_session_to_workspace(
            &ws_id,
            shared_lib,
            post_cortex::workspace::SessionRole::Shared,
        )
        .unwrap();

    // Verify roles
    let workspace = system.workspace_manager.get_workspace(&ws_id).unwrap();
    assert_eq!(
        workspace.get_session_role(&auth_session),
        Some(post_cortex::workspace::SessionRole::Primary)
    );
    assert_eq!(
        workspace.get_session_role(&payment_session),
        Some(post_cortex::workspace::SessionRole::Related)
    );
    assert_eq!(
        workspace.get_session_role(&shared_lib),
        Some(post_cortex::workspace::SessionRole::Shared)
    );

    // Remove session
    let removed = system
        .workspace_manager
        .remove_session_from_workspace(&ws_id, &shared_lib)
        .unwrap();
    assert_eq!(removed, Some(post_cortex::workspace::SessionRole::Shared));

    // Verify removal
    let workspace = system.workspace_manager.get_workspace(&ws_id).unwrap();
    assert_eq!(workspace.get_session_role(&shared_lib), None);
    assert_eq!(workspace.session_ids.len(), 2);
}

#[serial]
#[tokio::test]
async fn test_stress_concurrent_operations_no_deadlock() {
    let (system, _temp_dir) = create_test_system().await;

    // Create one workspace
    let ws_id = system
        .workspace_manager
        .create_workspace("Stress Test".to_string(), "High concurrency test".to_string());

    // Spawn 100 concurrent tasks that add/remove sessions
    let tasks: Vec<_> = (0..100)
        .map(|i| {
            let sys = system.clone();
            let workspace_id = ws_id;
            tokio::spawn(async move {
                // Create session
                let session_id = sys
                    .create_session(Some(format!("session-{}", i)), None)
                    .await
                    .unwrap();

                // Add to workspace
                sys.workspace_manager
                    .add_session_to_workspace(
                        &workspace_id,
                        session_id,
                        post_cortex::workspace::SessionRole::Related,
                    )
                    .unwrap();

                // Read workspace multiple times (concurrent read stress)
                for _ in 0..10 {
                    let ws = sys.workspace_manager.get_workspace(&workspace_id).unwrap();
                    assert!(ws.session_ids.contains_key(&session_id));
                }

                session_id
            })
        })
        .collect();

    // If there's a deadlock, this will timeout
    let session_ids: Vec<_> = futures::future::join_all(tasks)
        .await
        .into_iter()
        .map(|r| r.unwrap())
        .collect();

    assert_eq!(session_ids.len(), 100);

    // Verify all sessions in workspace
    let workspace = system.workspace_manager.get_workspace(&ws_id).unwrap();
    assert_eq!(workspace.session_ids.len(), 100);
}
