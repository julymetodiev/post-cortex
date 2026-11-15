// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use post_cortex_memory::SystemConfig;

use super::*;

async fn memory() -> (Arc<Memory>, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let memory = crate::memory_tests::rocksdb_memory(
        &dir,
        SystemConfig {
            enable_embeddings: false,
            ..SystemConfig::default()
        },
    )
    .await
    .unwrap();
    (Arc::new(memory), dir)
}

fn stamped(workspace: &str) -> BusEvent {
    BusEvent {
        id: Uuid::now_v7().to_string(),
        position: 0,
        name: "board.task_moved".to_string(),
        module: "board".to_string(),
        workspace_id: workspace.to_string(),
        author: "ann".to_string(),
        at: Some(Timestamp {
            seconds: 1_793_000_000,
            nanos: 5,
        }),
        payload: Some(Any {
            type_url: format!("{TYPE_URL}pcx.board.v1.Task"),
            value: vec![8, 1].into(),
        }),
    }
}

#[tokio::test]
async fn an_event_reads_back_as_the_host_stamped_it_with_its_position() {
    let (memory, _dir) = memory().await;
    let store = MemoryEvents::new(&memory);
    let event = stamped(&Uuid::new_v4().to_string());
    let kept = store.append(event.clone()).await.unwrap();
    assert_eq!(
        kept,
        BusEvent {
            position: 1,
            ..event
        }
    );
    let read = store.read("team", "board.task_moved", 10).await.unwrap();
    assert_eq!(read.events, vec![kept.clone()]);
    store
        .acknowledge("team", "board.task_moved", 1)
        .await
        .unwrap();
    assert!(
        store
            .read("team", "board.task_moved", 10)
            .await
            .unwrap()
            .events
            .is_empty()
    );
}

#[tokio::test]
async fn an_event_out_of_shape_is_refused() {
    let (memory, _dir) = memory().await;
    let store = MemoryEvents::new(&memory);
    for event in [
        BusEvent {
            id: "x".into(),
            ..stamped("")
        },
        stamped("not a workspace"),
        BusEvent {
            at: None,
            ..stamped("")
        },
        BusEvent {
            payload: None,
            ..stamped("")
        },
    ] {
        let refused = store.append(event).await.unwrap_err();
        assert_eq!(refused.kind, ModuleErrorKind::InvalidArgument);
    }
}
