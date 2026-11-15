// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Every kind of change reaches a subscriber once storage holds it, in the
//! order it was made.

use super::*;
use crate::capabilities::CreateWorkspace;
use crate::capabilities::UpdateWorkspace;
use crate::capabilities::test_support::rocksdb_memory;
use crate::memory_system::{CardChange, MessagesChangeKind, WorkspaceChangeKind};
use chrono::Utc;
use post_cortex_core::core::card::CardScope;
use post_cortex_core::core::message::{MessageRole, NewMessage};
use post_cortex_core::services::{AppendMessagesRequest, UpdateCardRequest};
use post_cortex_core::workspace::SessionRole;
use uuid::Uuid;

/// Every change already in the feed, records left out.
fn drain(rx: &mut broadcast::Receiver<Change>) -> Vec<Change> {
    std::iter::from_fn(|| rx.try_recv().ok())
        .filter(|c| c.record().is_none())
        .collect()
}

fn turn(id: &str) -> NewMessage {
    NewMessage {
        turn_id: id.into(),
        role: MessageRole::User,
        author: None,
        source_session: None,
        timestamp: Utc::now(),
        text: format!("turn {id}"),
        tools: Vec::new(),
        files: Vec::new(),
        parent: None,
        blocks: Vec::new(),
    }
}

#[tokio::test]
async fn workspaces_cards_and_messages_publish_their_changes() {
    let (memory, _dir) = rocksdb_memory().await;
    let sid = memory.sessions.create(None, None).await.unwrap();
    let mut rx = memory.changes.subscribe();

    let ws = memory
        .workspaces
        .create(CreateWorkspace {
            name: "harbour".into(),
            description: String::new(),
        })
        .await
        .unwrap()
        .id;
    memory
        .workspaces
        .add_session(ws, sid, SessionRole::Primary)
        .await
        .unwrap();
    memory
        .workspaces
        .update(
            ws,
            UpdateWorkspace {
                name: Some("port".into()),
                description: None,
                ..UpdateWorkspace::default()
            },
        )
        .await
        .unwrap();
    memory.workspaces.set_archived(ws, true).await.unwrap();
    // Archiving twice changes nothing and says nothing.
    memory.workspaces.set_archived(ws, true).await.unwrap();
    memory.workspaces.remove_session(ws, sid).await.unwrap();
    let kinds: Vec<WorkspaceChangeKind> = drain(&mut rx)
        .into_iter()
        .map(|c| match c {
            Change::Workspace(w) if w.workspace_id == ws => w.kind,
            other => panic!("unexpected {other:?}"),
        })
        .collect();
    assert_eq!(
        kinds,
        [
            WorkspaceChangeKind::Created,
            WorkspaceChangeKind::SessionAdded(sid),
            WorkspaceChangeKind::Updated,
            WorkspaceChangeKind::Archived,
            WorkspaceChangeKind::SessionRemoved(sid),
        ]
    );

    memory
        .cards
        .update(UpdateCardRequest {
            scope: CardScope::Workspace(ws),
            text: "keep the light on".into(),
            author: None,
            expected_version: 0,
        })
        .await
        .unwrap();
    assert!(matches!(
        drain(&mut rx).as_slice(),
        [Change::Card(CardChange { scope: CardScope::Workspace(w), version: 1, .. })] if *w == ws
    ));

    memory
        .messages
        .append(AppendMessagesRequest {
            session_id: sid,
            messages: vec![turn("a"), turn("b")],
        })
        .await
        .unwrap();
    memory.messages.delete(sid, 0).await.unwrap();
    let streams: Vec<(MessagesChangeKind, Vec<u64>)> = drain(&mut rx)
        .into_iter()
        .map(|c| match c {
            Change::Messages(m) if m.session_id == sid => (m.kind, m.seqs),
            other => panic!("unexpected {other:?}"),
        })
        .collect();
    assert_eq!(
        streams,
        [
            (MessagesChangeKind::Appended, vec![0, 1]),
            (MessagesChangeKind::Deleted, vec![0]),
        ]
    );

    // Deleting a missing turn or an empty stream says nothing.
    assert!(!memory.messages.delete(sid, 9).await.unwrap());
    let other = memory.sessions.create(None, None).await.unwrap();
    memory.messages.delete_all(other).await.unwrap();
    memory.workspaces.delete(ws).await.unwrap();
    assert!(matches!(
        drain(&mut rx).as_slice(),
        [Change::Workspace(w)] if w.kind == WorkspaceChangeKind::Deleted
    ));
    assert!(!memory.workspaces.delete(Uuid::new_v4()).await.unwrap());
    assert!(drain(&mut rx).is_empty());
}

#[tokio::test]
async fn memory_writes_name_the_workspaces_of_a_record() {
    let (memory, _dir) = rocksdb_memory().await;
    let ws = memory
        .workspaces
        .create(CreateWorkspace {
            name: "followed".into(),
            description: String::new(),
        })
        .await
        .unwrap()
        .id;
    let sid = memory.sessions.create(None, None).await.unwrap();
    memory
        .workspaces
        .add_session(ws, sid, SessionRole::Primary)
        .await
        .unwrap();
    let mut writes = memory.changes.memory_writes();
    let record = crate::memory_system::write_path_tests::text_record("followed");
    let id = record.id.to_string();
    memory.records.commit(sid, record, true).await.unwrap();
    let next = tokio::time::timeout(std::time::Duration::from_secs(10), writes.next())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        next,
        vec![Written {
            write: MemoryWrite::Record {
                workspace_id: ws.to_string(),
                id,
                record_type: Some("concept_defined".to_string()),
                deleted: false,
            },
            reach: None,
        }]
    );
}

#[tokio::test]
async fn a_feed_that_fell_behind_says_so_once() {
    use crate::memory_system::DocumentChange;
    use crate::memory_system::change_log::CHANGE_CAPACITY;
    use post_cortex_core::core::document::DocumentKind;

    let (memory, _dir) = rocksdb_memory().await;
    let mut writes = memory.changes.memory_writes();
    let kind: DocumentKind = "board/task".parse().unwrap();
    let workspace_id = Uuid::new_v4();
    for _ in 0..=CHANGE_CAPACITY {
        memory.changes.log.publish(Change::Document(DocumentChange {
            workspace_id,
            id: Uuid::new_v4(),
            kind: kind.clone(),
            version: Some(1),
            at: Utc::now(),
            reach: Some(vec![workspace_id]),
        }));
    }
    assert_eq!(
        writes.next().await,
        Some(vec![Written {
            write: MemoryWrite::FellBehind,
            reach: None,
        }])
    );
    // Then the changes still held, one by one, each with the reach of the
    // call that made it.
    let held = writes.next().await.unwrap();
    assert!(matches!(held[0].write, MemoryWrite::Document { .. }));
    assert_eq!(held[0].reach, Some(vec![workspace_id]));
}
