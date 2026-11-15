// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use chrono::Duration;
use uuid::Uuid;

use crate::capabilities::test_support::rocksdb_memory;

use super::*;

fn event(name: &str, at: DateTime<Utc>) -> NewBusEvent {
    NewBusEvent {
        id: Uuid::now_v7(),
        name: name.to_string(),
        module: "board".to_string(),
        workspace_id: None,
        author: None,
        at,
        payload_type: "pcx.board.v1.TaskMoved".to_string(),
        payload: Vec::new(),
    }
}

#[tokio::test]
async fn a_follower_reads_after_what_it_acknowledged_and_learns_it_fell_behind() {
    let (memory, _dir) = rocksdb_memory().await;
    let events = &memory.events;
    let now = Utc::now();
    let name = "board.task_moved";
    let first = events
        .append(event(name, now - Duration::days(9)))
        .await
        .unwrap();
    let second = events.append(event(name, now)).await.unwrap();

    // New: everything kept, not behind.
    let read = events.read("team", name, 10).await.unwrap();
    assert_eq!(read.events, vec![first.clone(), second.clone()]);
    assert!(!read.fell_behind);

    events
        .acknowledge("team", name, first.position)
        .await
        .unwrap();
    assert_eq!(
        events.read("team", name, 10).await.unwrap().events,
        vec![second.clone()]
    );

    // The old one is dropped; a follower that had acknowledged it is not
    // behind, one that acknowledged none is new.
    assert_eq!(events.drop_old(now).await.unwrap(), 1);
    assert!(!events.read("team", name, 10).await.unwrap().fell_behind);
    assert!(!events.read("telegram", name, 10).await.unwrap().fell_behind);

    // One that acknowledged an earlier position than the dropped one is.
    let third = events
        .append(event(name, now - Duration::days(8)))
        .await
        .unwrap();
    events
        .acknowledge("ops", name, second.position)
        .await
        .unwrap();
    assert_eq!(events.drop_old(now).await.unwrap(), 1);
    let behind = events.read("ops", name, 10).await.unwrap();
    assert!(
        behind.fell_behind && behind.events.is_empty(),
        "{behind:?} after {third:?}"
    );

    assert!(matches!(
        events.read("team", name, 0).await,
        Err(EventError::InvalidArgument(_))
    ));
}
