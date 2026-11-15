// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use super::*;
use tonic::{Response, Status};

async fn resolve_project(
    svc: &Served,
    request: ResolveProjectRequest,
) -> Result<ResolveProjectResponse, Status> {
    svc.resolve_project(Request::new(request))
        .await
        .map(Response::into_inner)
}

async fn find_project(svc: &Served, request: FindProjectRequest) -> FindProjectResponse {
    svc.find_project(Request::new(request))
        .await
        .unwrap()
        .into_inner()
}

#[tokio::test]
async fn resolve_project_over_grpc() {
    let dir = tempfile::tempdir().unwrap();
    let memory = crate::memory_tests::rocksdb_memory(
        &dir,
        post_cortex_memory::SystemConfig {
            enable_embeddings: false,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let svc = served(Arc::new(memory));
    let memory = Arc::clone(&svc.memory);
    let request = |key: &str| pb::ResolveProjectRequest {
        key: key.to_string(),
        name: "post-cortex".to_string(),
        client: Some("claude-code".to_string()),
        workspace: None,
    };

    let bad = resolve_project(&svc, request("has space")).await;
    assert_eq!(bad.unwrap_err().code(), tonic::Code::InvalidArgument);

    let first = resolve_project(&svc, request("github.com/a/post-cortex"))
        .await
        .unwrap();
    assert!(first.created);
    let again = resolve_project(&svc, request("github.com/a/post-cortex"))
        .await
        .unwrap();
    assert!(!again.created);
    assert_eq!(
        (&again.workspace_id, &again.session_id),
        (&first.workspace_id, &first.session_id)
    );
    let ws = memory
        .workspaces
        .get(first.workspace_id.parse().unwrap())
        .unwrap();
    assert_eq!(ws.name, "post-cortex");

    let find = |key: &str, workspace: Option<&str>| {
        find_project(
            &svc,
            pb::FindProjectRequest {
                key: key.to_string(),
                workspace: workspace.map(String::from),
            },
        )
    };
    let bound = find("github.com/a/post-cortex", Some("elsewhere")).await;
    assert!(bound.bound);
    assert_eq!(
        (&bound.workspace_id, &bound.session_id),
        (&first.workspace_id, &first.session_id)
    );
    let joins = find("github.com/a/new", Some("post-cortex")).await;
    assert!(!joins.bound);
    assert_eq!(joins.workspace_id, first.workspace_id);
    assert_eq!(
        find("github.com/a/new", Some("missing")).await,
        pb::FindProjectResponse::default()
    );
    assert_eq!(memory.workspaces.count(), 1);
}
