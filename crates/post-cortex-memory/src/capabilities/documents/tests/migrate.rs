// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The migration at start: unkeyed documents get their keys, and those
//! written as JSON before W23 are converted once to their kinds' messages
//! first.

use super::*;
use crate::Memory;

#[tokio::test]
async fn documents_written_as_json_are_migrated_to_their_messages() {
    let (memory, _dir) = rocksdb_memory().await;
    let ws = workspace(&memory).await;
    let id = Uuid::new_v4();
    let old = DocumentDraft {
        workspace_id: ws,
        id,
        kind: task(),
        data: data(json!({"name": "old", "description": "todo"})),
        title: "old".to_string(),
        text: "written before W23".to_string(),
        author: Some("claude".to_string()),
        keys: Vec::new(),
    };
    unkeyed(&memory, old).await;

    // No running module declares the kind: the document waits.
    let waiting = memory.documents.migrate(&Fixed(Vec::new())).await.unwrap();
    assert_eq!(
        waiting,
        Migrated {
            undeclared: 1,
            ..Migrated::default()
        }
    );
    let migrated = memory.documents.migrate(&board_kinds()).await.unwrap();
    assert_eq!((migrated.keyed, migrated.converted), (1, 1));
    let keyed = memory
        .documents
        .documents
        .unkeyed_documents(ws)
        .await
        .unwrap();
    assert!(keyed.is_empty(), "keyed with its message");
    let read = memory.documents.get(ws, id, None).await.unwrap();
    assert_eq!((read.version, read.author.as_deref()), (1, Some("claude")));
    assert!(matches!(
        &read.data,
        DocumentData::Message { type_name, .. } if type_name == "pcx.v1.SessionInfo"
    ));
    assert_eq!(json_of(&memory, &read).await["description"], "todo");
    // Once is enough.
    assert_eq!(
        memory.documents.migrate(&board_kinds()).await.unwrap(),
        Migrated::default()
    );
}

/// The chief's kind `action`, a `pcx.chief.v1.Action` (its input a
/// `google.protobuf.Struct`).
fn chief_kinds() -> Fixed {
    Fixed(vec![KindDeclaration {
        kind: DocumentKind::new("chief", "action").unwrap(),
        type_name: "pcx.chief.v1.Action".to_string(),
        title_field: "tool".to_string(),
        searchable: true,
        descriptors: post_cortex_proto::CHIEF_DESCRIPTOR_SET.to_vec(),
    }])
}

/// `draft` stored as an unkeyed first version, as a store migrated from a
/// version before keys holds it.
async fn unkeyed(memory: &Memory, draft: DocumentDraft) {
    let (ws, id) = (draft.workspace_id, draft.id);
    let (version, _) = draft.into_version(1, chrono::Utc::now());
    memory
        .documents
        .documents
        .restore_document_versions(ws, id, vec![version], true, None)
        .await
        .unwrap();
}

/// A document of kind `kind` with `value` as its JSON data, written before
/// W23.
async fn written(memory: &Memory, ws: Uuid, kind: &str, value: Value) -> Uuid {
    let id = Uuid::new_v4();
    let draft = DocumentDraft {
        workspace_id: ws,
        id,
        kind: kind.parse().unwrap(),
        data: data(value),
        title: "old".to_string(),
        text: String::new(),
        author: None,
        keys: Vec::new(),
    };
    unkeyed(memory, draft).await;
    id
}

#[tokio::test]
async fn an_input_kept_as_its_json_text_is_read_as_its_object_and_a_bad_document_waits() {
    let (memory, _dir) = rocksdb_memory().await;
    let ws = workspace(&memory).await;
    // The chief's actions before W23 kept their input as its JSON text.
    let kept = written(
        &memory,
        ws,
        "chief/action",
        json!({"talk": "t1", "tool": "task_add", "input": "{\"title\":\"Ship\"}"}),
    )
    .await;
    let bad = written(&memory, ws, "chief/action", json!({"tool": 5})).await;

    let migrated = memory.documents.migrate(&chief_kinds()).await.unwrap();
    assert_eq!(
        migrated,
        Migrated {
            keyed: 1,
            converted: 1,
            undeclared: 0,
            unreadable: 1
        }
    );
    let read = memory.documents.get(ws, kept, None).await.unwrap();
    assert!(matches!(&read.data, DocumentData::Message { .. }));
    let left = memory.documents.get(ws, bad, None).await.unwrap();
    assert!(
        matches!(&left.data, DocumentData::Json(_)),
        "left as it was"
    );
}

#[tokio::test]
async fn an_enum_value_written_as_its_short_name_is_read_as_the_value() {
    let (memory, _dir) = rocksdb_memory().await;
    let ws = workspace(&memory).await;
    let team = Fixed(vec![KindDeclaration {
        kind: DocumentKind::new("team", "agent").unwrap(),
        type_name: "pcx.team.v1.Agent".to_string(),
        title_field: "name".to_string(),
        searchable: true,
        descriptors: post_cortex_proto::TEAM_DESCRIPTOR_SET.to_vec(),
    }]);
    // Team members before W23 named their authority so.
    let id = written(
        &memory,
        ws,
        "team/agent",
        json!({"name": "dev", "authority": "decide"}),
    )
    .await;
    let migrated = memory.documents.migrate(&team).await.unwrap();
    assert_eq!((migrated.converted, migrated.unreadable), (1, 0));
    let read = memory.documents.get(ws, id, None).await.unwrap();
    assert!(matches!(&read.data, DocumentData::Message { .. }));
}
