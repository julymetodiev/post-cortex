// Copyright (c) 2025, 2026 Julius ML
// MIT License

//! Workspace lifecycle, message deletion and radar dismissals over gRPC
//! (`lifecycle.rs`); export and import: `transfer_tests`.

use super::*;
use pb::post_cortex_client::PostCortexClient;
use post_cortex_memory::Memory;
use post_cortex_storage::RealRocksDBStorage;
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;

pub(super) async fn system(storage: &RealRocksDBStorage) -> Arc<Memory> {
    Arc::new(
        crate::memory_tests::over_rocksdb(
            storage.clone(),
            post_cortex_memory::SystemConfig {
                enable_embeddings: false,
                ..Default::default()
            },
        )
        .await
        .unwrap(),
    )
}

/// A valid write of `text` to `session_id`.
pub(super) fn record(
    session_id: Uuid,
    text: &str,
) -> post_cortex_core::services::UpdateContextRequest {
    use post_cortex_core::core::context_update::{
        EntityData, EntityRelationship, EntityType, RelationType, UpdateContent, UpdateType,
    };
    let entity = |name: &str| EntityData {
        aliases: Vec::new(),
        name: name.to_string(),
        entity_type: EntityType::Concept,
        first_mentioned: chrono::Utc::now(),
        last_mentioned: chrono::Utc::now(),
        mention_count: 1,
        importance_score: 1.0,
        description: None,
    };
    post_cortex_core::services::UpdateContextRequest {
        session_id,
        interaction_type: UpdateType::ConceptDefined,
        content: UpdateContent {
            title: text.to_string(),
            description: text.to_string(),
            details: vec![],
            examples: vec![],
            implications: vec![],
        },
        entities: vec![entity("a"), entity("b")],
        relations: vec![EntityRelationship {
            from_entity: "a".into(),
            to_entity: "b".into(),
            relation_type: RelationType::RelatedTo,
            context: "test".into(),
        }],
        code_reference: None,
        author: None,
        derived_from: Vec::new(),
        meta: Default::default(),
    }
}

/// The gRPC service over `memory`.
pub(super) fn grpc_over(memory: Arc<Memory>) -> Served {
    served(memory)
}

/// Serve `memory` on a loopback port; returns a connected client.
pub(super) async fn serve(memory: Arc<Memory>) -> PostCortexClient<tonic::transport::Channel> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let incoming = futures::stream::unfold(listener, |listener| async move {
        let accepted = listener.accept().await.map(|(stream, _)| stream);
        Some((accepted, listener))
    });
    let kinds = post_cortex_memory::MemoryModule::new(Arc::clone(&memory))
        .unwrap()
        .kinds();
    tokio::spawn(
        tonic::transport::Server::builder()
            .add_service(PcxGrpcService::new(memory, BUILD, kinds).into_server())
            .serve_with_incoming(incoming),
    );
    PostCortexClient::connect(format!("http://{addr}"))
        .await
        .unwrap()
}

pub(super) async fn create_workspace(grpc: &PcxGrpcService, name: &str) -> String {
    grpc.create_workspace(Request::new(CreateWorkspaceRequest {
        name: name.into(),
        description: "d".into(),
    }))
    .await
    .unwrap()
    .into_inner()
    .workspace_id
}

async fn list(grpc: &PcxGrpcService, include_archived: bool) -> Vec<WorkspaceInfo> {
    grpc.list_workspaces(Request::new(ListWorkspacesRequest { include_archived }))
        .await
        .unwrap()
        .into_inner()
        .workspaces
}

/// UpdateWorkspace and (Un)ArchiveWorkspace change memory and storage; an
/// archived workspace leaves ListWorkspaces unless asked for; DeleteWorkspace
/// deletes it from storage too.
#[tokio::test]
async fn workspace_update_archive_and_delete() {
    let dir = tempfile::tempdir().unwrap();
    let storage = RealRocksDBStorage::new(dir.path()).await.unwrap();
    let grpc = grpc_over(system(&storage).await);
    let ws = create_workspace(&grpc, "ws").await;
    create_workspace(&grpc, "other").await;

    let updated = grpc
        .update_workspace(Request::new(UpdateWorkspaceRequest {
            workspace_id: ws.clone(),
            name: Some("renamed".into()),
            description: None,
            remote_rerank: None,
        }))
        .await
        .unwrap()
        .into_inner()
        .workspace
        .unwrap();
    assert_eq!(
        (updated.name.as_str(), updated.description.as_str()),
        ("renamed", "d")
    );
    // A new workspace forbids the remote reranker until it is allowed.
    assert!(!updated.remote_rerank);
    let allowed = grpc
        .update_workspace(Request::new(UpdateWorkspaceRequest {
            workspace_id: ws.clone(),
            name: None,
            description: None,
            remote_rerank: Some(true),
        }))
        .await
        .unwrap()
        .into_inner()
        .workspace
        .unwrap();
    assert!(allowed.remote_rerank);
    assert_eq!(allowed.name, "renamed");
    let taken = grpc
        .update_workspace(Request::new(UpdateWorkspaceRequest {
            workspace_id: ws.clone(),
            name: Some("other".into()),
            description: None,
            remote_rerank: None,
        }))
        .await
        .unwrap_err();
    assert_eq!(taken.code(), tonic::Code::AlreadyExists);
    let empty = grpc
        .update_workspace(Request::new(UpdateWorkspaceRequest {
            workspace_id: ws.clone(),
            name: None,
            description: None,
            remote_rerank: None,
        }))
        .await
        .unwrap_err();
    assert_eq!(empty.code(), tonic::Code::InvalidArgument);
    let missing = grpc
        .archive_workspace(Request::new(ArchiveWorkspaceRequest {
            workspace_id: Uuid::new_v4().to_string(),
        }))
        .await
        .unwrap_err();
    assert_eq!(missing.code(), tonic::Code::NotFound);

    let archive = || {
        Request::new(ArchiveWorkspaceRequest {
            workspace_id: ws.clone(),
        })
    };
    let first = grpc
        .archive_workspace(archive())
        .await
        .unwrap()
        .into_inner();
    let info = first.workspace.unwrap();
    assert!(first.changed && info.archived && info.archived_at_unix > 0);
    assert!(
        !grpc
            .archive_workspace(archive())
            .await
            .unwrap()
            .into_inner()
            .changed
    );
    assert_eq!(list(&grpc, false).await.len(), 1);
    assert_eq!(list(&grpc, true).await.len(), 2);
    drop(grpc);

    let grpc = grpc_over(system(&storage).await);
    let after = list(&grpc, true).await;
    let restored = after.iter().find(|w| w.workspace_id == ws).unwrap();
    assert_eq!(restored.name, "renamed");
    assert_eq!(restored.archived_at_unix, info.archived_at_unix);
    let back = grpc
        .unarchive_workspace(archive())
        .await
        .unwrap()
        .into_inner();
    assert!(back.changed && !back.workspace.unwrap().archived);
    assert_eq!(list(&grpc, false).await.len(), 2);

    assert!(
        grpc.delete_workspace(Request::new(DeleteWorkspaceRequest {
            workspace_id: ws.clone(),
        }))
        .await
        .unwrap()
        .into_inner()
        .success
    );
    drop(grpc);
    let grpc = grpc_over(system(&storage).await);
    assert!(list(&grpc, true).await.iter().all(|w| w.workspace_id != ws));
}

/// DeleteMessage and DeleteMessages report what they deleted and what is
/// left; an unknown session is NotFound.
#[tokio::test]
async fn messages_are_deleted_over_grpc() {
    use post_cortex_core::core::message::{MessageRole, NewMessage};

    let dir = tempfile::tempdir().unwrap();
    let storage = RealRocksDBStorage::new(dir.path()).await.unwrap();
    let grpc = grpc_over(system(&storage).await);
    let sid = grpc.memory.sessions.create(None, None).await.unwrap();
    let turns = ["a", "b", "c"]
        .map(|t| NewMessage {
            turn_id: t.into(),
            role: MessageRole::User,
            author: None,
            source_session: None,
            timestamp: chrono::Utc::now(),
            text: t.into(),
            tools: vec![],
            files: vec![],
            parent: None,
            blocks: Vec::new(),
        })
        .to_vec();
    grpc.memory
        .messages
        .append(post_cortex_core::services::AppendMessagesRequest {
            session_id: sid,
            messages: turns,
        })
        .await
        .unwrap();

    let one = |seq| {
        Request::new(DeleteMessageRequest {
            session_id: sid.to_string(),
            seq,
        })
    };
    let r = grpc.delete_message(one(0)).await.unwrap().into_inner();
    assert_eq!((r.deleted, r.remaining), (1, 2));
    let r = grpc.delete_message(one(0)).await.unwrap().into_inner();
    assert_eq!((r.deleted, r.remaining), (0, 2));
    let r = grpc
        .delete_messages(Request::new(DeleteMessagesRequest {
            session_id: sid.to_string(),
        }))
        .await
        .unwrap()
        .into_inner();
    assert_eq!((r.deleted, r.remaining), (2, 0));
    let unknown = grpc
        .delete_messages(Request::new(DeleteMessagesRequest {
            session_id: Uuid::new_v4().to_string(),
        }))
        .await
        .unwrap_err();
    assert_eq!(unknown.code(), tonic::Code::NotFound);
}

/// A dismissed radar item leaves the radar view of the workspace and of its
/// member session until it is restored; include_dismissed shows it.
#[tokio::test]
async fn radar_dismissals_over_grpc() {
    let dir = tempfile::tempdir().unwrap();
    let storage = RealRocksDBStorage::new(dir.path()).await.unwrap();
    let grpc = grpc_over(system(&storage).await);
    let ws = create_workspace(&grpc, "ws").await;
    let sid = grpc.memory.sessions.create(None, None).await.unwrap();
    grpc.add_session_to_workspace(Request::new(AddSessionToWorkspaceRequest {
        workspace_id: ws.clone(),
        session_id: sid.to_string(),
        role: "primary".into(),
    }))
    .await
    .unwrap();
    grpc.update_context(Request::new(UpdateContextRequest {
        session_id: sid.to_string(),
        interaction_type: "problem_solved".into(),
        content: Some(ContextContent {
            title: "cache stampede".into(),
            description: "still open".into(),
            ..Default::default()
        }),
        entities: vec![
            EntityInfo {
                name: "stampede".into(),
                entity_type: "problem".into(),
            },
            EntityInfo {
                name: "cache".into(),
                entity_type: "technology".into(),
            },
        ],
        relations: vec![RelationInfo {
            from_entity: "stampede".into(),
            to_entity: "cache".into(),
            relation_type: "related_to".into(),
            context: "in the cache".into(),
        }],
        ..Default::default()
    }))
    .await
    .unwrap();

    let radar = |session: bool, include: bool| {
        Request::new(GetGraphViewRequest {
            session_id: if session {
                sid.to_string()
            } else {
                String::new()
            },
            workspace_id: if session { String::new() } else { ws.clone() },
            view: Some(get_graph_view_request::View::Radar(RadarViewParams {
                limit: 0,
                include_dismissed: include,
            })),
        })
    };
    let radar_of = |view: GraphView| -> RadarView {
        match view.view {
            Some(graph_view::View::Radar(radar)) => radar,
            other => panic!("not the radar: {other:?}"),
        }
    };
    let unsolved = |radar: &RadarView| -> Vec<String> {
        radar
            .unsolved_problems
            .iter()
            .map(|n| n.name.clone())
            .collect()
    };
    // The graph is merged in the background.
    let mut seen = Vec::new();
    for _ in 0..100 {
        seen = unsolved(&radar_of(
            grpc.get_graph_view(radar(false, false))
                .await
                .unwrap()
                .into_inner(),
        ));
        if !seen.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(seen, ["stampede"]);

    let bad = grpc
        .dismiss_radar_item(Request::new(DismissRadarItemRequest {
            workspace_id: ws.clone(),
            kind: "nope".into(),
            key: "stampede".into(),
            author: None,
        }))
        .await
        .unwrap_err();
    assert_eq!(bad.code(), tonic::Code::InvalidArgument);
    let dismissed = grpc
        .dismiss_radar_item(Request::new(DismissRadarItemRequest {
            workspace_id: ws.clone(),
            kind: "unsolved_problem".into(),
            key: "stampede".into(),
            author: Some("ann".into()),
        }))
        .await
        .unwrap()
        .into_inner()
        .dismissal
        .unwrap();
    assert_eq!(dismissed.author.as_deref(), Some("ann"));
    for session in [false, true] {
        let shown = radar_of(
            grpc.get_graph_view(radar(session, false))
                .await
                .unwrap()
                .into_inner(),
        );
        assert!(unsolved(&shown).is_empty(), "session view: {session}");
        assert_eq!(shown.dismissed[0].key, "stampede");
        let all = radar_of(
            grpc.get_graph_view(radar(session, true))
                .await
                .unwrap()
                .into_inner(),
        );
        assert_eq!(unsolved(&all), ["stampede"]);
    }
    let listed = grpc
        .list_radar_dismissals(Request::new(ListRadarDismissalsRequest {
            workspace_id: ws.clone(),
        }))
        .await
        .unwrap()
        .into_inner()
        .dismissals;
    assert_eq!(listed.len(), 1);

    let restore = || {
        Request::new(RestoreRadarItemRequest {
            workspace_id: ws.clone(),
            kind: "unsolved_problem".into(),
            key: "stampede".into(),
        })
    };
    assert!(
        grpc.restore_radar_item(restore())
            .await
            .unwrap()
            .into_inner()
            .existed
    );
    assert!(
        !grpc
            .restore_radar_item(restore())
            .await
            .unwrap()
            .into_inner()
            .existed
    );
    let restored = radar_of(
        grpc.get_graph_view(radar(false, false))
            .await
            .unwrap()
            .into_inner(),
    );
    assert_eq!(unsolved(&restored), ["stampede"]);
}

/// Health names the build that answers: its Cargo version and the commit
/// of the checkout it was built in (this test runs in one).
#[tokio::test]
async fn health_names_the_version_and_the_commit_of_the_build() {
    let dir = tempfile::tempdir().unwrap();
    let storage = RealRocksDBStorage::new(dir.path()).await.unwrap();
    let grpc = grpc_over(system(&storage).await);
    let health = grpc
        .health(Request::new(HealthRequest {}))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(health.version, env!("CARGO_PKG_VERSION"));
    let head = std::process::Command::new("git")
        .args(["rev-parse", "--short=12", "HEAD"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .unwrap();
    assert_eq!(health.commit, String::from_utf8_lossy(&head.stdout).trim());
    assert_eq!(health.commit.len(), 12);
}
