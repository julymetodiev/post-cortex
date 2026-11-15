// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! `pcx.views.v1` over the daemon's memory: records written through
//! `pcx.v1` come back through the views, and requests the views cannot read
//! are refused, never read as a default.

use super::*;
use post_cortex_proto::views as v;
use post_cortex_proto::views::views_server::Views;
use post_cortex_transport::views::ViewsService;
use std::sync::Arc;
use tonic::Code;

async fn services() -> (Served, ViewsService, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let memory = Arc::new(
        crate::memory_tests::rocksdb_memory(
            &dir,
            post_cortex_memory::SystemConfig {
                enable_embeddings: false,
                ..Default::default()
            },
        )
        .await
        .unwrap(),
    );
    (served(Arc::clone(&memory)), ViewsService::new(memory), dir)
}

fn item(interaction_type: &str, title: &str) -> ContextUpdateItem {
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
                entity_type: "technology".into(),
            },
            EntityInfo {
                name: "Keeper".into(),
                entity_type: "concept".into(),
            },
        ],
        relations: vec![RelationInfo {
            from_entity: "Keeper".into(),
            to_entity: "Lighthouse".into(),
            relation_type: "depends_on".into(),
            context: "test".into(),
        }],
        ..Default::default()
    }
}

fn session_scope(id: uuid::Uuid) -> Option<v::Scope> {
    Some(v::Scope {
        scope: Some(v::scope::Scope::SessionId(id.to_string())),
    })
}

fn refused<T>(r: Result<T, tonic::Status>) -> Code {
    match r {
        Ok(_) => panic!("expected a refusal"),
        Err(status) => status.code(),
    }
}

/// Two decisions and a question written through `pcx.v1`.
async fn seeded() -> (ViewsService, uuid::Uuid, tempfile::TempDir) {
    let (grpc, views, dir) = services().await;
    let sid = grpc.memory.sessions.create(None, None).await.unwrap();
    grpc.bulk_update_context(Request::new(BulkUpdateContextRequest {
        session_id: sid.to_string(),
        updates: vec![
            item("decision_made", "light the lamp"),
            item("decision_made", "trim the wick"),
            item("qa", "who keeps it"),
        ],
        author: Some("ana".into()),
    }))
    .await
    .unwrap();
    (views, sid, dir)
}

#[tokio::test]
async fn records_written_through_pcx_v1_come_back_through_the_views() {
    let (views, sid, _dir) = seeded().await;

    let graph = views
        .get_graph(Request::new(v::GetGraphRequest {
            scope: session_scope(sid),
            ..Default::default()
        }))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(graph.total_nodes, 2);
    let keeper = graph.nodes.iter().find(|n| n.name == "Keeper").unwrap();
    assert_eq!(keeper.authors, ["ana"]);
    assert_eq!(keeper.session_ids, [sid.to_string()]);
    assert_eq!(graph.edges[0].relation_type, "depends_on");

    let page = views
        .list_records(Request::new(v::ListRecordsRequest {
            scope: session_scope(sid),
            interaction_types: vec!["decision_made".into()],
            limit: 1,
            ..Default::default()
        }))
        .await
        .unwrap()
        .into_inner();
    assert_eq!((page.records.len(), page.total), (1, 2));
    assert_eq!(page.records[0].interaction_type, "decision_made");
    let second = views
        .list_records(Request::new(v::ListRecordsRequest {
            scope: session_scope(sid),
            interaction_types: vec!["decision_made".into()],
            limit: 1,
            cursor: page.next_cursor.clone(),
            ..Default::default()
        }))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(second.records.len(), 1);
    assert_ne!(second.records[0].id, page.records[0].id);
    assert!(second.next_cursor.is_empty());

    let record = views
        .get_record(Request::new(v::GetRecordRequest {
            scope: session_scope(sid),
            id: page.records[0].id.clone(),
        }))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(record.author.as_deref(), Some("ana"));
    assert_eq!(record.relations[0].to, "Lighthouse");

    let entity = views
        .get_entity(Request::new(v::GetEntityRequest {
            scope: session_scope(sid),
            name: "lighthouse".into(),
            ..Default::default()
        }))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(entity.entity.unwrap().name, "Lighthouse");
    assert_eq!(entity.total_records, 3);

    let overview = views
        .get_overview(Request::new(v::GetOverviewRequest {
            scope: Some(v::Scope {
                scope: Some(v::scope::Scope::Global(true)),
            }),
            ..Default::default()
        }))
        .await
        .unwrap()
        .into_inner();
    assert_eq!((overview.records, overview.sessions), (3, 1));
}

#[tokio::test]
async fn what_the_views_cannot_read_is_refused() {
    let (views, sid, _dir) = seeded().await;

    let no_scope = views
        .get_graph(Request::new(v::GetGraphRequest::default()))
        .await;
    assert_eq!(refused(no_scope), Code::InvalidArgument);
    let not_global = views
        .get_graph(Request::new(v::GetGraphRequest {
            scope: Some(v::Scope {
                scope: Some(v::scope::Scope::Global(false)),
            }),
            ..Default::default()
        }))
        .await;
    assert_eq!(refused(not_global), Code::InvalidArgument);
    let bad_id = views
        .get_graph(Request::new(v::GetGraphRequest {
            scope: Some(v::Scope {
                scope: Some(v::scope::Scope::SessionId("not-a-uuid".into())),
            }),
            ..Default::default()
        }))
        .await;
    assert_eq!(refused(bad_id), Code::InvalidArgument);
    let unknown_session = views
        .get_graph(Request::new(v::GetGraphRequest {
            scope: session_scope(uuid::Uuid::new_v4()),
            ..Default::default()
        }))
        .await;
    assert_eq!(refused(unknown_session), Code::NotFound);

    let unknown_type = views
        .list_records(Request::new(v::ListRecordsRequest {
            scope: session_scope(sid),
            interaction_types: vec!["decision".into()],
            ..Default::default()
        }))
        .await;
    assert_eq!(refused(unknown_type), Code::InvalidArgument);
    let bad_cursor = views
        .list_records(Request::new(v::ListRecordsRequest {
            scope: session_scope(sid),
            cursor: "12_nope".into(),
            ..Default::default()
        }))
        .await;
    assert_eq!(refused(bad_cursor), Code::InvalidArgument);
    let unknown_validity = views
        .list_records(Request::new(v::ListRecordsRequest {
            scope: session_scope(sid),
            validity: 9,
            ..Default::default()
        }))
        .await;
    assert_eq!(refused(unknown_validity), Code::InvalidArgument);
    let bad_time = views
        .get_graph(Request::new(v::GetGraphRequest {
            scope: session_scope(sid),
            at: Some(prost_types::Timestamp {
                seconds: 0,
                nanos: -1,
            }),
            ..Default::default()
        }))
        .await;
    assert_eq!(refused(bad_time), Code::InvalidArgument);
    let missing_record = views
        .get_record(Request::new(v::GetRecordRequest {
            scope: session_scope(sid),
            id: uuid::Uuid::new_v4().to_string(),
        }))
        .await;
    assert_eq!(refused(missing_record), Code::NotFound);
    let unknown_mode = views
        .search(Request::new(v::SearchRequest {
            scope: session_scope(sid),
            query: "lamp".into(),
            mode: 7,
            ..Default::default()
        }))
        .await;
    assert_eq!(refused(unknown_mode), Code::InvalidArgument);
}

#[tokio::test]
async fn watch_activity_pushes_a_record_written_through_pcx_v1() {
    use futures::StreamExt;
    let (grpc, views, _dir) = services().await;
    let sid = grpc.memory.sessions.create(None, None).await.unwrap();
    let mut events = views
        .watch_activity(Request::new(v::WatchActivityRequest {
            scope: session_scope(sid),
            first_only: true,
            ..Default::default()
        }))
        .await
        .unwrap()
        .into_inner();
    grpc.bulk_update_context(Request::new(BulkUpdateContextRequest {
        session_id: sid.to_string(),
        updates: vec![item("decision_made", "light the lamp")],
        author: None,
    }))
    .await
    .unwrap();
    let event = tokio::time::timeout(std::time::Duration::from_secs(10), events.next())
        .await
        .expect("an event in time")
        .expect("one event")
        .expect("no error");
    assert_eq!(event.kind(), v::ActivityKind::ActivityWritten);
    assert_eq!(event.session_id, sid.to_string());
    assert_eq!(event.record.unwrap().title, "light the lamp");
    assert!(events.next().await.is_none(), "first_only ends the stream");

    let no_scope = views
        .watch_activity(Request::new(v::WatchActivityRequest::default()))
        .await;
    assert_eq!(refused(no_scope), Code::InvalidArgument);
}
