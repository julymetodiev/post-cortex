// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use super::*;
use post_cortex_core::core::message::{MessageCursor, MessageOrder};
use post_cortex_memory::capabilities::CreateWorkspace;
use tonic::{Response, Status};

async fn append_messages(
    svc: &Served,
    request: AppendMessagesRequest,
) -> Result<AppendMessagesResponse, Status> {
    svc.append_messages(Request::new(request))
        .await
        .map(Response::into_inner)
}

async fn get_card(svc: &Served, request: GetCardRequest) -> Result<GetCardResponse, Status> {
    svc.get_card(Request::new(request))
        .await
        .map(Response::into_inner)
}

async fn update_card(
    svc: &Served,
    request: UpdateCardRequest,
) -> Result<UpdateCardResponse, Status> {
    svc.update_card(Request::new(request))
        .await
        .map(Response::into_inner)
}

async fn service() -> (Served, Arc<Memory>, tempfile::TempDir) {
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
    (svc, memory, dir)
}

#[tokio::test]
async fn append_messages_over_grpc() {
    let (svc, memory, _dir) = service().await;
    let sid = memory.sessions.create(None, None).await.unwrap();
    let turn = |id: &str, role: &str| pb::CapturedMessage {
        turn_id: id.to_string(),
        role: role.to_string(),
        text: format!("turn {id}"),
        timestamp_unix_ms: Some(1_760_000_000_000),
        ..Default::default()
    };
    let request = |messages| pb::AppendMessagesRequest {
        session_id: sid.to_string(),
        messages,
    };

    let bad = append_messages(&svc, request(vec![turn("x", "robot")])).await;
    assert_eq!(bad.unwrap_err().code(), tonic::Code::InvalidArgument);

    let ok = append_messages(
        &svc,
        request(vec![turn("a", "user"), turn("b", "Assistant")]),
    )
    .await
    .unwrap();
    assert_eq!(ok.appended, [0, 1]);
    let again = append_messages(&svc, request(vec![turn("a", "user")]))
        .await
        .unwrap();
    assert_eq!((again.appended.len(), again.skipped), (0, 1));
    let stored = memory
        .messages
        .page(sid, MessageCursor::From(0), 10, MessageOrder::Asc)
        .await
        .unwrap();
    assert_eq!(stored[0].timestamp.timestamp_millis(), 1_760_000_000_000);

    let unknown = append_messages(
        &svc,
        pb::AppendMessagesRequest {
            session_id: uuid::Uuid::new_v4().to_string(),
            messages: vec![turn("c", "user")],
        },
    )
    .await;
    assert_eq!(unknown.unwrap_err().code(), tonic::Code::NotFound);
}

#[tokio::test]
async fn cards_over_grpc() {
    let (svc, memory, _dir) = service().await;
    let ws = memory
        .workspaces
        .create(CreateWorkspace {
            name: "g".to_string(),
            description: String::new(),
        })
        .await
        .unwrap()
        .id;

    let neither = get_card(&svc, pb::GetCardRequest::default()).await;
    assert_eq!(neither.unwrap_err().code(), tonic::Code::InvalidArgument);

    let update = |content: &str, expected_version| pb::UpdateCardRequest {
        workspace_id: Some(ws.to_string()),
        content: content.to_string(),
        expected_version,
        ..Default::default()
    };
    let first = update_card(&svc, update("## Goals\nship", 0))
        .await
        .unwrap();
    assert!(first.written);
    assert_eq!(
        first.card.as_ref().unwrap().scope,
        format!("workspace:{ws}")
    );
    let stale = update_card(&svc, update("other", 0)).await.unwrap();
    assert!(!stale.written);
    assert_eq!(stale.card.unwrap().version, 1);

    let read = get_card(
        &svc,
        pb::GetCardRequest {
            workspace_id: Some(ws.to_string()),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(read.card.unwrap().text, "## Goals\nship");

    let user = get_card(
        &svc,
        pb::GetCardRequest {
            user: true,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert!(user.card.is_none());
}
