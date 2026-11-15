// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use super::*;
use post_cortex_core::workspace::SessionRole;
use post_cortex_storage::RealRocksDBStorage;
use std::sync::Arc;

async fn start(storage: &RealRocksDBStorage) -> Served {
    let memory = crate::memory_tests::over_rocksdb(
        storage.clone(),
        post_cortex_memory::SystemConfig {
            enable_embeddings: false,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    served(Arc::new(memory))
}

/// The gRPC service serves the capability services it is given: one
/// write pipeline and redactor for every transport of the daemon, so a
/// session another transport creates is the one gRPC lists.
#[tokio::test]
async fn grpc_serves_the_shared_service() {
    let dir = tempfile::tempdir().unwrap();
    let storage = RealRocksDBStorage::new(dir.path()).await.unwrap();
    let grpc = start(&storage).await;
    let sid = grpc.memory.sessions.create(None, None).await.unwrap();
    let listed = grpc
        .list_sessions(Request::new(ListSessionsRequest::default()))
        .await
        .unwrap()
        .into_inner();
    assert!(
        listed
            .sessions
            .iter()
            .any(|s| s.session_id == sid.to_string()),
        "{listed:?}"
    );
}

/// AddSessionToWorkspace / RemoveSessionFromWorkspace are what a restart
/// finds, roles included (both backends: memory_system::workspaces).
#[tokio::test]
async fn membership_over_grpc_survives_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let storage = RealRocksDBStorage::new(dir.path()).await.unwrap();
    let grpc = start(&storage).await;
    let ws = grpc
        .create_workspace(Request::new(CreateWorkspaceRequest {
            name: "ws".into(),
            description: "d".into(),
        }))
        .await
        .unwrap()
        .into_inner()
        .workspace_id;
    let kept = grpc.memory.sessions.create(None, None).await.unwrap();
    let removed = grpc.memory.sessions.create(None, None).await.unwrap();
    for (sid, role) in [(kept, "dependency"), (removed, "primary")] {
        grpc.add_session_to_workspace(Request::new(AddSessionToWorkspaceRequest {
            workspace_id: ws.clone(),
            session_id: sid.to_string(),
            role: role.into(),
        }))
        .await
        .unwrap();
    }
    let remove = || {
        Request::new(RemoveSessionFromWorkspaceRequest {
            workspace_id: ws.clone(),
            session_id: removed.to_string(),
        })
    };
    grpc.remove_session_from_workspace(remove()).await.unwrap();
    let again = grpc.remove_session_from_workspace(remove()).await;
    assert_eq!(again.unwrap_err().code(), tonic::Code::NotFound);
    drop(grpc);

    let restarted = start(&storage).await;
    let sessions = restarted
        .memory
        .workspaces
        .get(ws.parse().unwrap())
        .unwrap()
        .get_all_sessions();
    assert_eq!(sessions, vec![(kept, SessionRole::Dependency)]);
}

/// Every failure kind has its one gRPC code, internal included (the
/// parity test drives not found, invalid and conflict end to end).
#[test]
fn every_failure_kind_has_its_grpc_code() {
    use post_cortex_memory::tools::{Failure, Kind};
    use tonic::Code;
    for (kind, code) in [
        (Kind::InvalidArgument, Code::InvalidArgument),
        (Kind::NotFound, Code::NotFound),
        (Kind::Conflict, Code::AlreadyExists),
        (Kind::Denied, Code::PermissionDenied),
        (Kind::FailedPrecondition, Code::FailedPrecondition),
        (Kind::Unavailable, Code::Unavailable),
        (Kind::Internal, Code::Internal),
    ] {
        let status = post_cortex_transport::grpc::status(Failure::new(kind, "x"));
        assert_eq!(status.code(), code, "{kind:?}");
    }
}
