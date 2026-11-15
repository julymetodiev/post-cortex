// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The bus: one ordered log of events whose positions are never reused,
//! read by name after a position, old events dropped, and each follower's
//! cursor per name moving only forward.

use std::collections::BTreeSet;

use chrono::{DateTime, Utc};
use uuid::Uuid;

use post_cortex_core::core::event::NewBusEvent;

use crate::conformance::at;
use crate::storage::Stores;

fn event(name: &str, emitted: DateTime<Utc>) -> NewBusEvent {
    NewBusEvent {
        id: Uuid::now_v7(),
        name: name.to_string(),
        module: "board".to_string(),
        workspace_id: Some(Uuid::new_v4()),
        author: Some("ann".to_string()),
        at: emitted,
        payload_type: "pcx.board.v1.TaskMoved".to_string(),
        payload: vec![1, 2, 3],
    }
}

/// Positions follow the appends; a name's events read after a position in
/// order, at most the limit; an event reads back as it was appended.
pub async fn positions_are_ordered_by_name_and_never_reused(stores: &Stores) {
    assert_eq!(stores.events.oldest_event().await.unwrap(), None);
    let mut appended = Vec::new();
    for name in [
        "board.task_moved",
        "board.decision_made",
        "board.task_moved",
    ] {
        appended.push(
            stores
                .events
                .append_event(event(name, at(0)))
                .await
                .unwrap(),
        );
    }
    let positions: Vec<u64> = appended.iter().map(|e| e.position).collect();
    assert!(positions.windows(2).all(|w| w[0] < w[1]), "{positions:?}");
    assert_eq!(
        stores.events.oldest_event().await.unwrap(),
        Some(positions[0])
    );

    let moved = stores
        .events
        .events_after("board.task_moved", 0, 10)
        .await
        .unwrap();
    assert_eq!(moved, vec![appended[0].clone(), appended[2].clone()]);
    let after = stores
        .events
        .events_after("board.task_moved", positions[0], 10)
        .await
        .unwrap();
    assert_eq!(after, vec![appended[2].clone()]);
    let one = stores
        .events
        .events_after("board.task_moved", 0, 1)
        .await
        .unwrap();
    assert_eq!(one, vec![appended[0].clone()]);
    assert!(
        stores
            .events
            .events_after("mail.sent", 0, 10)
            .await
            .unwrap()
            .is_empty()
    );
}

/// Appends at once each get a position of their own.
pub async fn racing_appends_get_distinct_positions(stores: &Stores) {
    let appends = (0..16).map(|_| stores.events.append_event(event("board.task_moved", at(0))));
    let positions: BTreeSet<u64> = futures::future::join_all(appends)
        .await
        .into_iter()
        .map(|appended| appended.unwrap().position)
        .collect();
    assert_eq!(positions.len(), 16);
    let read = stores
        .events
        .events_after("board.task_moved", 0, 100)
        .await
        .unwrap();
    assert_eq!(read.len(), 16);
}

/// Events emitted before a time are dropped, each name keeping the newest
/// position dropped; the next append still gets a position after every one
/// ever given.
pub async fn old_events_are_dropped(stores: &Stores) {
    let old = stores
        .events
        .append_event(event("board.task_moved", at(0)))
        .await
        .unwrap();
    let kept = stores
        .events
        .append_event(event("board.task_moved", at(100)))
        .await
        .unwrap();
    assert_eq!(
        stores
            .events
            .dropped_through("board.task_moved")
            .await
            .unwrap(),
        0
    );
    assert_eq!(stores.events.drop_events_before(at(50)).await.unwrap(), 1);
    assert_eq!(
        stores.events.oldest_event().await.unwrap(),
        Some(kept.position)
    );
    assert_eq!(
        stores
            .events
            .dropped_through("board.task_moved")
            .await
            .unwrap(),
        old.position
    );
    assert_eq!(stores.events.dropped_through("mail.sent").await.unwrap(), 0);
    assert_eq!(stores.events.drop_events_before(at(200)).await.unwrap(), 1);
    assert_eq!(stores.events.oldest_event().await.unwrap(), None);
    assert_eq!(
        stores
            .events
            .dropped_through("board.task_moved")
            .await
            .unwrap(),
        kept.position
    );
    let next = stores
        .events
        .append_event(event("board.task_moved", at(300)))
        .await
        .unwrap();
    assert!(next.position > kept.position && kept.position > old.position);
}

/// A follower's cursor is per name, starts at 0 and only moves forward.
pub async fn cursors_only_move_forward(stores: &Stores) {
    let name = "board.task_moved";
    assert_eq!(stores.cursors.cursor("team", name).await.unwrap(), 0);
    stores
        .cursors
        .advance_cursor("team", name, 5)
        .await
        .unwrap();
    stores
        .cursors
        .advance_cursor("team", name, 3)
        .await
        .unwrap();
    assert_eq!(stores.cursors.cursor("team", name).await.unwrap(), 5);
    stores
        .cursors
        .advance_cursor("team", name, 9)
        .await
        .unwrap();
    assert_eq!(stores.cursors.cursor("team", name).await.unwrap(), 9);
    assert_eq!(
        stores
            .cursors
            .cursor("team", "board.decision_made")
            .await
            .unwrap(),
        0
    );
    assert_eq!(stores.cursors.cursor("telegram", name).await.unwrap(), 0);
}
