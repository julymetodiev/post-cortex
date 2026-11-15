// Copyright (c) 2026 Julius ML
// MIT License

//! A failure is the same kind on every transport: not found, invalid,
//! conflict (internal: see the per-transport mappings over every kind).

use super::*;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use post_cortex_proto::pb;
use post_cortex_proto::pb::post_cortex_server::PostCortex;
use tonic::{Code, Request as GrpcRequest};
use tower::ServiceExt;

/// The kind an MCP tool error states.
async fn mcp_kind(t: &mut Transports, tool: &str, args: Value) -> String {
    let message = t.mcp.call_raw(tool, args).await;
    assert_eq!(message["result"]["isError"], json!(true), "{message}");
    let data = mcp_data(&message).unwrap_or_else(|| panic!("no details: {message}"));
    data["kind"].as_str().unwrap().to_string()
}

async fn rest_status(t: &Transports, request: Request<Body>) -> StatusCode {
    t.router.clone().oneshot(request).await.unwrap().status()
}

fn post(path: &str, body: Value) -> Request<Body> {
    Request::post(path)
        .header("host", "localhost:3737")
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

#[tokio::test]
async fn not_found_is_not_found_everywhere() {
    let mut t = transports().await;
    let ghost = uuid::Uuid::new_v4().to_string();
    let kind = mcp_kind(
        &mut t,
        "get_structured_summary",
        json!({"session_id": ghost}),
    )
    .await;
    assert_eq!(kind, "not_found");
    let status = t
        .grpc
        .get_structured_summary(GrpcRequest::new(pb::GetStructuredSummaryRequest {
            session_id: ghost.clone(),
            ..Default::default()
        }))
        .await
        .unwrap_err();
    assert_eq!(status.code(), Code::NotFound, "{status:?}");
    let file = Request::get(format!("/api/file?session_id={ghost}&path=a.rs"))
        .header("host", "localhost:3737")
        .body(Body::empty())
        .unwrap();
    assert_eq!(rest_status(&t, file).await, StatusCode::NOT_FOUND);
    let append = post(
        "/api/messages",
        json!({"session_id": ghost, "messages": []}),
    );
    assert_eq!(rest_status(&t, append).await, StatusCode::NOT_FOUND);
    t.ct.cancel();
}

#[tokio::test]
async fn invalid_is_invalid_everywhere() {
    let mut t = transports().await;
    let args = json!({"action": "resolve_project", "key": "a b", "name": "x"});
    assert_eq!(mcp_kind(&mut t, "session", args).await, "invalid_argument");
    let status = t
        .grpc
        .resolve_project(GrpcRequest::new(pb::ResolveProjectRequest {
            key: "a b".into(),
            name: "x".into(),
            ..Default::default()
        }))
        .await
        .unwrap_err();
    assert_eq!(status.code(), Code::InvalidArgument, "{status:?}");
    let resolve = post("/api/projects/resolve", json!({"key": "a b", "name": "x"}));
    assert_eq!(rest_status(&t, resolve).await, StatusCode::BAD_REQUEST);

    // A strict name: an unknown role is invalid, never a default.
    let sid = t.memory.sessions.create(None, None).await.unwrap();
    let ws = t
        .memory
        .workspaces
        .create(post_cortex_memory::capabilities::CreateWorkspace {
            name: "w".into(),
            description: String::new(),
        })
        .await
        .unwrap();
    let args = json!({"action": "add_session", "workspace_id": ws.id.to_string(),
                      "session_id": sid.to_string(), "role": "boss"});
    assert_eq!(
        mcp_kind(&mut t, "manage_workspace", args).await,
        "invalid_argument"
    );
    let status = t
        .grpc
        .add_session_to_workspace(GrpcRequest::new(pb::AddSessionToWorkspaceRequest {
            workspace_id: ws.id.to_string(),
            session_id: sid.to_string(),
            role: "boss".into(),
        }))
        .await
        .unwrap_err();
    assert_eq!(status.code(), Code::InvalidArgument, "{status:?}");
    let attach = post(
        &format!("/api/workspaces/{}/sessions/{sid}", ws.id),
        json!({"role": "boss"}),
    );
    assert_eq!(rest_status(&t, attach).await, StatusCode::BAD_REQUEST);
    t.ct.cancel();
}

#[tokio::test]
async fn conflict_is_conflict_everywhere() {
    let mut t = transports().await;
    let create = |name: &str| post_cortex_memory::capabilities::CreateWorkspace {
        name: name.into(),
        description: String::new(),
    };
    t.memory.workspaces.create(create("taken")).await.unwrap();
    let other = t.memory.workspaces.create(create("other")).await.unwrap();
    let args = json!({"action": "update", "workspace_id": other.id.to_string(), "name": "taken"});
    assert_eq!(mcp_kind(&mut t, "manage_workspace", args).await, "conflict");
    let status = t
        .grpc
        .update_workspace(GrpcRequest::new(pb::UpdateWorkspaceRequest {
            workspace_id: other.id.to_string(),
            name: Some("taken".into()),
            description: None,
            remote_rerank: None,
        }))
        .await
        .unwrap_err();
    assert_eq!(status.code(), Code::AlreadyExists, "{status:?}");
    t.ct.cancel();
}

/// gRPC names are parsed by the shared strict parsers: a misspelt
/// interaction or entity type is invalid, never stored as a default
/// (`ConceptDefined`, `Concept`).
#[tokio::test]
async fn grpc_names_are_strict() {
    let t = transports().await;
    let sid = t
        .memory
        .sessions
        .create(None, None)
        .await
        .unwrap()
        .to_string();
    let entity = |name: &str, entity_type: &str| pb::EntityInfo {
        name: name.into(),
        entity_type: entity_type.into(),
    };
    for (interaction_type, entity_type) in
        [("decison_made", "concept"), ("decision_made", "conceptt")]
    {
        let status = t
            .grpc
            .update_context(GrpcRequest::new(pb::UpdateContextRequest {
                session_id: sid.clone(),
                interaction_type: interaction_type.into(),
                content: Some(pb::ContextContent {
                    title: "t".into(),
                    description: "d".into(),
                    ..Default::default()
                }),
                entities: vec![entity("a", entity_type), entity("b", "concept")],
                relations: vec![pb::RelationInfo {
                    from_entity: "a".into(),
                    to_entity: "b".into(),
                    relation_type: "related_to".into(),
                    context: "c".into(),
                }],
                ..Default::default()
            }))
            .await
            .unwrap_err();
        assert_eq!(status.code(), Code::InvalidArgument, "{status:?}");
    }
    let session = t.memory.sessions.get(sid.parse().unwrap()).await.unwrap();
    assert!(session.records().is_empty(), "nothing was stored");
    t.ct.cancel();
}

/// With embeddings off, a search is refused alike in every scope on MCP
/// and gRPC: the check is in `Search::search`, not in one transport.
#[tokio::test]
async fn search_without_embeddings_is_refused_alike() {
    let mut t = transports().await;
    let session = t.memory.sessions.create(None, None).await.unwrap();
    let workspace = t
        .memory
        .workspaces
        .create(post_cortex_memory::capabilities::CreateWorkspace {
            name: "w".into(),
            description: String::new(),
        })
        .await
        .unwrap()
        .id;
    t.memory
        .workspaces
        .add_session(
            workspace,
            session,
            post_cortex_core::workspace::SessionRole::Primary,
        )
        .await
        .unwrap();
    for (scope, id) in [("session", session), ("workspace", workspace)] {
        let args = json!({"query": "q", "scope": scope, "scope_id": id.to_string()});
        let kind = mcp_kind(&mut t, "semantic_search", args).await;
        assert_eq!(kind, "failed_precondition", "{scope}");
    }
    let status = t
        .grpc
        .semantic_search(GrpcRequest::new(pb::SemanticSearchRequest {
            query: "q".into(),
            workspace_id: Some(workspace.to_string()),
            max_results: 10,
            ..Default::default()
        }))
        .await
        .unwrap_err();
    assert_eq!(status.code(), Code::FailedPrecondition, "{status:?}");
    t.ct.cancel();
}
