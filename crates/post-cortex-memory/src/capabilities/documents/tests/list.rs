// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Lists of documents: by kind and filter, the newest first, in pages read
//! by storage (docs/memory.md, document keys and pages).

use super::*;

#[tokio::test]
async fn lists_filter_by_fields_newest_first_in_pages() {
    let (memory, _dir) = rocksdb_memory().await;
    let docs = &memory.documents;
    let kinds = board_kinds();
    let board = Writer {
        module: Some("board"),
        kinds: &kinds,
        reach: None,
    };
    let ws = workspace(&memory).await;
    let mut todo = Vec::new();
    for n in 0..3 {
        let mut req = put(ws, None, json!({}), 0);
        req.data = task_data(&format!("t{n}"), "todo");
        todo.push(docs.put(req, board).await.unwrap().document.unwrap().id);
    }
    let done = put(ws, None, json!({"name": "d", "description": "done"}), 0);
    docs.put(done, board).await.unwrap();

    let todo_only = || {
        Some(DocumentFilter {
            example: task_data("", "todo"),
            fields: vec!["description".to_string()],
        })
    };
    let list = |after, filter| ListDocuments {
        workspace_id: ws,
        kind: task(),
        filter,
        after,
        limit: 2,
    };
    let first = docs.list(list(None, todo_only()), &kinds).await.unwrap();
    assert_eq!(first.total, 3);
    let ids: Vec<Uuid> = first.documents.iter().map(|d| d.id).collect();
    assert_eq!(ids, [todo[2], todo[1]]);
    let rest = docs
        .list(list(first.next, todo_only()), &kinds)
        .await
        .unwrap();
    assert_eq!(
        rest.documents.iter().map(|d| d.id).collect::<Vec<_>>(),
        [todo[0]]
    );
    assert_eq!(rest.next, None);
    let every = docs.list(list(None, None), &kinds).await.unwrap();
    assert_eq!(every.total, 4);
    // A filter of a field the type does not have is refused.
    let unknown = Some(DocumentFilter {
        example: task_data("", "todo"),
        fields: vec!["column".to_string()],
    });
    assert!(matches!(
        docs.list(list(None, unknown), &kinds).await,
        Err(DocumentError::InvalidArgument(_))
    ));
}

/// A filter compares top-level singular scalar fields only; a document
/// not keyed yet (a store from before keys) is found by a filter all the
/// same.
#[tokio::test]
async fn a_filter_names_scalar_fields_and_finds_a_document_not_keyed_yet() {
    let (memory, _dir) = rocksdb_memory().await;
    let docs = &memory.documents;
    let kinds = board_kinds();
    let ws = workspace(&memory).await;
    let comments = ListDocuments {
        workspace_id: ws,
        kind: "board/comment".parse().unwrap(),
        filter: Some(DocumentFilter {
            example: data(json!({"entity": "cards"})),
            fields: vec!["dependents".to_string()],
        }),
        after: None,
        limit: 10,
    };
    assert!(matches!(
        docs.list(comments, &kinds).await,
        Err(DocumentError::InvalidArgument(reason)) if reason.contains("dependents")
    ));

    let (version, _) = DocumentDraft {
        workspace_id: ws,
        id: Uuid::new_v4(),
        kind: task(),
        data: task_data("old", "todo"),
        title: "old".to_string(),
        text: String::new(),
        author: None,
        keys: Vec::new(),
    }
    .into_version(1, chrono::Utc::now());
    let id = version.id;
    docs.documents
        .restore_document_versions(ws, id, vec![version], true, None)
        .await
        .unwrap();
    let todo = |description: &str| ListDocuments {
        workspace_id: ws,
        kind: task(),
        filter: Some(DocumentFilter {
            example: task_data("", description),
            fields: vec!["description".to_string()],
        }),
        after: None,
        limit: 10,
    };
    let found = docs.list(todo("todo"), &kinds).await.unwrap();
    assert_eq!(
        found.documents.iter().map(|d| d.id).collect::<Vec<_>>(),
        [id]
    );
    assert!(
        docs.list(todo("done"), &kinds)
            .await
            .unwrap()
            .documents
            .is_empty()
    );
    assert_eq!(docs.migrate(&kinds).await.unwrap().keyed, 1);
    let keyed = docs.list(todo("todo"), &kinds).await.unwrap();
    assert_eq!((keyed.documents.len(), keyed.total), (1, 1));
}
