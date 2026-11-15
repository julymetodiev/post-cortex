// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! gRPC records: one atomic bulk write with ids in input order,
//! QueryContext by wire names with a real total, and the health count.

use super::*;
use std::sync::Arc;

async fn service() -> (Served, tempfile::TempDir) {
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
    (served(Arc::new(memory)), dir)
}

fn item(interaction_type: &str, title: &str, relation: &str) -> ContextUpdateItem {
    ContextUpdateItem {
        interaction_type: interaction_type.into(),
        content: Some(ContextContent {
            title: title.into(),
            description: format!("about {title}"),
            ..Default::default()
        }),
        entities: vec![
            EntityInfo {
                name: "Lighthouse".into(),
                entity_type: "concept".into(),
            },
            EntityInfo {
                name: "Keeper".into(),
                entity_type: "concept".into(),
            },
        ],
        relations: vec![RelationInfo {
            from_entity: "Lighthouse".into(),
            to_entity: "Keeper".into(),
            relation_type: relation.into(),
            context: "test".into(),
        }],
        ..Default::default()
    }
}

fn bulk(
    session: &uuid::Uuid,
    updates: Vec<ContextUpdateItem>,
) -> Request<BulkUpdateContextRequest> {
    Request::new(BulkUpdateContextRequest {
        session_id: session.to_string(),
        updates,
        author: None,
    })
}

/// A bulk update is one bulk write: the ids match the inputs by position,
/// and a bad item refuses the whole batch (it wrote the others one by one).
#[tokio::test]
async fn bulk_update_is_one_atomic_write_with_ids_in_input_order() {
    let (grpc, _dir) = service().await;
    let sid = grpc.memory.sessions.create(None, None).await.unwrap();
    let titles = ["first", "second", "third"];
    let resp = grpc
        .bulk_update_context(bulk(
            &sid,
            titles
                .iter()
                .map(|t| item("decision_made", t, "related_to"))
                .collect(),
        ))
        .await
        .unwrap()
        .into_inner();
    assert_eq!((resp.success_count, resp.failure_count), (3, 0));
    let session = grpc.memory.sessions.get(sid).await.unwrap();
    for (id, title) in resp.update_ids.iter().zip(titles) {
        let record = session.record(&id.parse().unwrap()).unwrap();
        assert_eq!(record.content.title, title, "ids are in input order");
    }

    let err = grpc
        .bulk_update_context(bulk(
            &sid,
            vec![
                item("decision_made", "good", "related_to"),
                item("decision_made", "bad", "no_such_relation"),
            ],
        ))
        .await
        .unwrap_err();
    assert_eq!(err.code(), tonic::Code::InvalidArgument, "{err:?}");
    assert!(err.message().contains("updates[1]"), "{}", err.message());
    let session = grpc.memory.sessions.get(sid).await.unwrap();
    assert_eq!(session.records().len(), 3, "the good item was not written");
}

/// QueryContext filters by the wire names of the types (the Debug-name
/// substring match let "d" match everything), reports the total the filter
/// matches, and sends wire names.
#[tokio::test]
async fn query_context_filters_by_wire_names_with_a_real_total() {
    let (grpc, _dir) = service().await;
    let sid = grpc.memory.sessions.create(None, None).await.unwrap();
    let items = vec![
        item("decision_made", "d1", "related_to"),
        item("decision_made", "d2", "related_to"),
        item("qa", "q1", "related_to"),
    ];
    grpc.bulk_update_context(bulk(&sid, items)).await.unwrap();
    let query = |interaction_type: &str, limit: u32| {
        Request::new(QueryContextRequest {
            session_id: sid.to_string(),
            interaction_type: interaction_type.into(),
            limit,
            after_unix: 0,
        })
    };
    let decisions = grpc
        .query_context(query("decision_made", 1))
        .await
        .unwrap()
        .into_inner();
    assert_eq!((decisions.updates.len(), decisions.total), (1, 2));
    assert_eq!(decisions.updates[0].interaction_type, "decision_made");
    let both = grpc
        .query_context(query("qa, decision_made", 0))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(both.total, 3);
    let err = grpc.query_context(query("d", 0)).await.unwrap_err();
    assert_eq!(err.code(), tonic::Code::InvalidArgument);
}

/// Health reports the request count under its own name (the old
/// `total_updates` field carries the same count for older clients).
#[tokio::test]
async fn health_reports_total_requests() {
    let (grpc, _dir) = service().await;
    grpc.memory.sessions.create(None, None).await.unwrap();
    let health = <PcxGrpcService as pb::post_cortex_server::PostCortex>::health(
        &grpc,
        Request::new(HealthRequest {}),
    )
    .await
    .unwrap()
    .into_inner();
    assert_eq!(health.total_requests, 1, "the session create");
    let legacy = health.total_updates;
    assert_eq!(legacy, health.total_requests);
}
