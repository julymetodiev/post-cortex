// Integration tests for workspace functionality with real RocksDB
use post_cortex::capabilities::CreateWorkspace;
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
        ..Default::default()
    };
    let system = open(temp_dir.path().to_str().unwrap(), config)
        .await
        .unwrap();
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
            ..Default::default()
        };
        let system = open(&data_dir, config).await.unwrap();

        // Create workspace
        let workspace_id = system
            .workspaces
            .create(CreateWorkspace {
                name: "Test Workspace".to_string(),
                description: "Real test".to_string(),
            })
            .await
            .unwrap()
            .id;

        // Verify in memory
        let workspace = system.workspaces.get(workspace_id).ok();
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
            ..Default::default()
        };
        let system2 = open(&data_dir, config2).await.unwrap();
        let workspace2 = system2.workspaces.get(workspace_id).ok();
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
                let ws_id = sys
                    .workspaces
                    .create(CreateWorkspace {
                        name: format!("Workspace {}", i),
                        description: format!("Test workspace {}", i),
                    })
                    .await
                    .unwrap()
                    .id;

                // Create 3 sessions
                let session1 = sys
                    .sessions
                    .create(Some(format!("service-{}-auth", i)), None)
                    .await
                    .unwrap();
                let session2 = sys
                    .sessions
                    .create(Some(format!("service-{}-payment", i)), None)
                    .await
                    .unwrap();
                let session3 = sys
                    .sessions
                    .create(Some(format!("service-{}-products", i)), None)
                    .await
                    .unwrap();

                // Add sessions to workspace
                sys.workspaces
                    .add_session(
                        ws_id,
                        session1,
                        post_cortex::workspace::SessionRole::Primary,
                    )
                    .await
                    .unwrap();
                sys.workspaces
                    .add_session(
                        ws_id,
                        session2,
                        post_cortex::workspace::SessionRole::Related,
                    )
                    .await
                    .unwrap();
                sys.workspaces
                    .add_session(
                        ws_id,
                        session3,
                        post_cortex::workspace::SessionRole::Related,
                    )
                    .await
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
    assert_eq!(system.workspaces.count(), 20);

    // Verify each workspace has 3 sessions
    let workspaces = system.workspaces.list(true);
    for workspace in workspaces {
        assert_eq!(workspace.session_count(), 3);
    }
}

#[serial]
#[tokio::test]
async fn test_rocksdb_single_instance_enforcement() {
    let temp_dir = tempfile::tempdir().unwrap();
    let data_dir = temp_dir.path().to_str().unwrap().to_string();

    // Open first instance
    let config1 = SystemConfig {
        ..Default::default()
    };
    let system1 = open(&data_dir, config1).await.unwrap();

    // Try to open second instance - should fail with lock error
    let config2 = SystemConfig {
        ..Default::default()
    };
    let result = open(&data_dir, config2).await;

    assert!(result.is_err());
    if let Err(error_msg) = result {
        let error_msg_lower = error_msg.to_string().to_lowercase();
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
        .workspaces
        .create(CreateWorkspace {
            name: "Microservices".to_string(),
            description: "E-commerce platform".to_string(),
        })
        .await
        .unwrap()
        .id;

    // Create sessions
    let auth_session = system
        .sessions
        .create(Some("auth-service".to_string()), None)
        .await
        .unwrap();
    let payment_session = system
        .sessions
        .create(Some("payment-service".to_string()), None)
        .await
        .unwrap();
    let shared_lib = system
        .sessions
        .create(Some("shared-utils".to_string()), None)
        .await
        .unwrap();

    // Add with different roles
    system
        .workspaces
        .add_session(
            ws_id,
            auth_session,
            post_cortex::workspace::SessionRole::Primary,
        )
        .await
        .unwrap();
    system
        .workspaces
        .add_session(
            ws_id,
            payment_session,
            post_cortex::workspace::SessionRole::Related,
        )
        .await
        .unwrap();
    system
        .workspaces
        .add_session(
            ws_id,
            shared_lib,
            post_cortex::workspace::SessionRole::Shared,
        )
        .await
        .unwrap();

    // Verify roles
    let workspace = system.workspaces.get(ws_id).ok().unwrap();
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
        .workspaces
        .remove_session(ws_id, shared_lib)
        .await
        .unwrap();
    assert_eq!(removed, post_cortex::workspace::SessionRole::Shared);

    // Verify removal
    let workspace = system.workspaces.get(ws_id).ok().unwrap();
    assert_eq!(workspace.get_session_role(&shared_lib), None);
    assert_eq!(workspace.session_count(), 2);
}

#[serial]
#[tokio::test]
async fn test_stress_concurrent_operations_no_deadlock() {
    let (system, _temp_dir) = create_test_system().await;

    // Create one workspace
    let ws_id = system
        .workspaces
        .create(CreateWorkspace {
            name: "Stress Test".to_string(),
            description: "High concurrency test".to_string(),
        })
        .await
        .unwrap()
        .id;

    // Spawn 100 concurrent tasks that add/remove sessions
    let tasks: Vec<_> = (0..100)
        .map(|i| {
            let sys = system.clone();
            let workspace_id = ws_id;
            tokio::spawn(async move {
                // Create session
                let session_id = sys
                    .sessions
                    .create(Some(format!("session-{}", i)), None)
                    .await
                    .unwrap();

                // Add to workspace
                sys.workspaces
                    .add_session(
                        workspace_id,
                        session_id,
                        post_cortex::workspace::SessionRole::Related,
                    )
                    .await
                    .unwrap();

                // Read workspace multiple times (concurrent read stress)
                for _ in 0..10 {
                    let ws = sys.workspaces.get(workspace_id).ok().unwrap();
                    assert!(ws.get_session_role(&session_id).is_some());
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
    let workspace = system.workspaces.get(ws_id).ok().unwrap();
    assert_eq!(workspace.session_count(), 100);
}
