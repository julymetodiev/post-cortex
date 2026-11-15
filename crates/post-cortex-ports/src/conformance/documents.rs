// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Documents: versions with optimistic concurrency and a fixed kind,
//! history, listing per workspace and kind, pages by kind and key,
//! deletion with every version, and imports.

use serde_json::{Value, json};
use uuid::Uuid;

use post_cortex_core::core::document::{
    Document, DocumentCursor, DocumentData, DocumentDraft, DocumentKey, DocumentKind,
};

use crate::conformance::at;
use crate::storage::{DocumentWrite, LatestPage, LatestQuery, Stores};

fn kind(name: &str) -> DocumentKind {
    DocumentKind::new("board", name).unwrap()
}

/// A message's data: its encoding has every byte value a store must keep.
fn data(title: &str) -> DocumentData {
    let mut bytes = title.as_bytes().to_vec();
    bytes.extend([0, 1, 127, 128, 255]);
    DocumentData::Message {
        type_name: "pcx.board.v1.Task".to_string(),
        bytes,
    }
}

/// Data written before W23, kept as JSON until the migration.
fn legacy(value: Value) -> DocumentData {
    match value {
        Value::Object(map) => DocumentData::Json(map),
        other => panic!("not an object: {other}"),
    }
}

fn draft(workspace_id: Uuid, id: Uuid, kind_name: &str, title: &str) -> DocumentDraft {
    DocumentDraft {
        workspace_id,
        id,
        kind: kind(kind_name),
        data: data(title),
        title: title.to_string(),
        text: format!("{title} text"),
        author: Some("claude".into()),
        keys: Vec::new(),
    }
}

fn key(field: u32, value: u8) -> DocumentKey {
    DocumentKey::new(field, [value; 16])
}

fn keyed(draft: DocumentDraft, keys: &[DocumentKey]) -> DocumentDraft {
    DocumentDraft {
        keys: keys.to_vec(),
        ..draft
    }
}

fn written(write: DocumentWrite) -> Document {
    match write {
        DocumentWrite::Written(document) => document,
        DocumentWrite::Conflict(latest) => panic!("unexpected conflict, latest {latest:?}"),
    }
}

async fn put(stores: &Stores, draft: DocumentDraft, expected: u64) -> Document {
    written(
        stores
            .documents
            .put_document(draft, expected)
            .await
            .unwrap(),
    )
}

/// Every write is a new version; a write from a stale version is a
/// conflict carrying the latest; every version is read back as written.
pub async fn versions_and_conflicts(stores: &Stores) {
    let (ws, id) = (Uuid::new_v4(), Uuid::new_v4());
    let docs = &stores.documents;
    assert_eq!(docs.get_document(ws, id, None).await.unwrap(), None);

    let v1 = put(stores, draft(ws, id, "task", "one"), 0).await;
    assert_eq!((v1.version, v1.author.as_deref()), (1, Some("claude")));
    assert_eq!(v1.data, data("one"));
    let old = DocumentDraft {
        data: legacy(json!({"title": "old", "labels": ["a", 1, null], "nested": {"x": 1.5}})),
        ..draft(ws, Uuid::new_v4(), "task", "old")
    };
    let kept = put(stores, old.clone(), 0).await;
    assert_eq!(kept.data, old.data);

    match docs
        .put_document(draft(ws, id, "task", "late"), 0)
        .await
        .unwrap()
    {
        DocumentWrite::Conflict(Some(latest)) => assert_eq!(latest, v1),
        other => panic!("expected a conflict, got {other:?}"),
    }

    let v2 = put(stores, draft(ws, id, "task", "two"), 1).await;
    assert_eq!(v2.version, 2);
    assert_eq!(
        docs.get_document(ws, id, None).await.unwrap(),
        Some(v2.clone())
    );
    assert_eq!(
        docs.get_document(ws, id, Some(1)).await.unwrap(),
        Some(v1.clone())
    );
    assert_eq!(docs.get_document(ws, id, Some(3)).await.unwrap(), None);
    // The id is the workspace's: another workspace does not see it.
    assert_eq!(
        docs.get_document(Uuid::new_v4(), id, None).await.unwrap(),
        None
    );

    let history = docs.document_history(ws, id, 10).await.unwrap();
    assert_eq!(history, vec![v2.clone(), v1], "newest first, all kept");
    assert_eq!(docs.document_history(ws, id, 1).await.unwrap(), vec![v2]);
}

/// A document keeps its kind: a write of another kind is a conflict.
pub async fn another_kind_is_a_conflict(stores: &Stores) {
    let (ws, id) = (Uuid::new_v4(), Uuid::new_v4());
    let v1 = put(stores, draft(ws, id, "task", "one"), 0).await;
    match stores
        .documents
        .put_document(draft(ws, id, "comment", "other"), 1)
        .await
        .unwrap()
    {
        DocumentWrite::Conflict(Some(latest)) => assert_eq!(latest, v1),
        other => panic!("expected a conflict, got {other:?}"),
    }
}

/// Of two writers racing from the same version, one wins and one gets the
/// conflict.
pub async fn one_of_two_racing_writers_wins(stores: &Stores) {
    let (ws, id) = (Uuid::new_v4(), Uuid::new_v4());
    put(stores, draft(ws, id, "task", "v1"), 0).await;
    let (a, b) = futures::join!(
        stores.documents.put_document(draft(ws, id, "task", "a"), 1),
        stores.documents.put_document(draft(ws, id, "task", "b"), 1),
    );
    let results = [a.unwrap(), b.unwrap()];
    let wins = results
        .iter()
        .filter(|w| matches!(w, DocumentWrite::Written(_)))
        .count();
    assert_eq!(wins, 1, "{results:?}");
    let latest = stores.documents.get_document(ws, id, None).await.unwrap();
    assert_eq!(latest.map(|d| d.version), Some(2));
}

fn ids(mut documents: Vec<Document>) -> Vec<(Uuid, u64)> {
    documents.sort_by_key(|d| d.id);
    documents.into_iter().map(|d| (d.id, d.version)).collect()
}

/// The latest version of each document of a workspace, of one kind or of
/// every kind.
pub async fn latest_documents_by_workspace_and_kind(stores: &Stores) {
    let (ws, other_ws) = (Uuid::new_v4(), Uuid::new_v4());
    let (a, b, c) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
    put(stores, draft(ws, a, "task", "a1"), 0).await;
    put(stores, draft(ws, a, "task", "a2"), 1).await;
    put(stores, draft(ws, b, "task", "b1"), 0).await;
    put(stores, draft(ws, c, "comment", "c1"), 0).await;
    put(stores, draft(other_ws, Uuid::new_v4(), "task", "x"), 0).await;

    let docs = &stores.documents;
    let mut tasks = vec![(a, 2), (b, 1)];
    tasks.sort();
    assert_eq!(
        ids(docs
            .latest_documents(ws, Some(&kind("task")))
            .await
            .unwrap()),
        tasks
    );
    let mut all = vec![(a, 2), (b, 1), (c, 1)];
    all.sort();
    assert_eq!(ids(docs.latest_documents(ws, None).await.unwrap()), all);
    assert!(
        docs.latest_documents(ws, Some(&kind("decision")))
            .await
            .unwrap()
            .is_empty()
    );
}

/// Deleting a document deletes every version; its id may then start again.
pub async fn delete_removes_every_version(stores: &Stores) {
    let (ws, id) = (Uuid::new_v4(), Uuid::new_v4());
    put(stores, draft(ws, id, "task", "one"), 0).await;
    put(stores, draft(ws, id, "task", "two"), 1).await;
    let docs = &stores.documents;
    assert!(!docs.delete_document(Uuid::new_v4(), id).await.unwrap());
    assert!(docs.delete_document(ws, id).await.unwrap());
    assert!(!docs.delete_document(ws, id).await.unwrap());
    assert_eq!(docs.get_document(ws, id, None).await.unwrap(), None);
    assert_eq!(docs.get_document(ws, id, Some(1)).await.unwrap(), None);
    assert!(docs.document_history(ws, id, 10).await.unwrap().is_empty());
    assert!(docs.latest_documents(ws, None).await.unwrap().is_empty());
    let again = put(stores, draft(ws, id, "comment", "new"), 0).await;
    assert_eq!(again.version, 1);
}

/// Deleting a workspace deletes its documents and their history.
pub async fn deleting_the_workspace_deletes_its_documents(stores: &Stores) {
    let ws = Uuid::new_v4();
    stores
        .workspaces
        .save_workspace_metadata(ws, "ws", &ws.to_string(), "", &[])
        .await
        .unwrap();
    let id = Uuid::new_v4();
    put(stores, draft(ws, id, "task", "one"), 0).await;
    let kept = put(
        stores,
        draft(Uuid::new_v4(), Uuid::new_v4(), "task", "x"),
        0,
    )
    .await;
    let (version, _) = draft(ws, Uuid::new_v4(), "task", "old").into_version(1, at(1));
    stores
        .documents
        .restore_document_versions(ws, version.id, vec![version], false, None)
        .await
        .unwrap();
    stores.workspaces.delete_workspace(ws).await.unwrap();
    let docs = &stores.documents;
    assert_eq!(paged(stores, ws, "task", &[], 10).await, (vec![], vec![0]));
    assert!(docs.unkeyed_documents(ws).await.unwrap().is_empty());
    assert_eq!(docs.get_document(ws, id, None).await.unwrap(), None);
    assert!(docs.document_history(ws, id, 10).await.unwrap().is_empty());
    assert!(docs.latest_documents(ws, None).await.unwrap().is_empty());
    assert_eq!(
        docs.get_document(kept.workspace_id, kept.id, None)
            .await
            .unwrap(),
        Some(kept)
    );
}

/// An import stores versions as given; the highest is the latest; with
/// `replace` the stored versions go first.
pub async fn restore_keeps_the_versions(stores: &Stores) {
    let (ws, id) = (Uuid::new_v4(), Uuid::new_v4());
    let version = |n: u64, title: &str| {
        let (mut document, _) = draft(ws, id, "task", title).into_version(n, at(n as i64));
        document.author = Some(format!("author {n}"));
        document
    };
    let docs = &stores.documents;
    docs.restore_document_versions(
        ws,
        id,
        vec![version(2, "two"), version(1, "one")],
        false,
        Some(vec![key(1, 2)]),
    )
    .await
    .unwrap();
    assert_eq!(
        docs.get_document(ws, id, None).await.unwrap(),
        Some(version(2, "two"))
    );
    assert_eq!(
        docs.document_history(ws, id, 10).await.unwrap(),
        vec![version(2, "two"), version(1, "one")]
    );
    // The next write follows the restored latest.
    assert_eq!(
        put(stores, draft(ws, id, "task", "three"), 2).await.version,
        3
    );

    docs.restore_document_versions(ws, id, vec![version(1, "only")], true, None)
        .await
        .unwrap();
    assert_eq!(
        docs.document_history(ws, id, 10).await.unwrap(),
        vec![version(1, "only")]
    );
    assert_eq!(
        ids(docs.latest_documents(ws, None).await.unwrap()),
        vec![(id, 1)]
    );
}

/// Every page of `kind` in `ws` with `keys`, `limit` at a time: the ids in
/// order, and the total each page gave.
async fn paged(
    stores: &Stores,
    ws: Uuid,
    kind_name: &str,
    keys: &[DocumentKey],
    limit: usize,
) -> (Vec<Uuid>, Vec<usize>) {
    let kind = kind(kind_name);
    let (mut ids, mut totals, mut after) = (Vec::new(), Vec::new(), None);
    loop {
        let LatestPage {
            documents,
            total,
            next,
        } = stores
            .documents
            .latest_page(LatestQuery {
                workspace_id: ws,
                kind: &kind,
                keys,
                after,
                limit,
            })
            .await
            .unwrap();
        assert!(documents.len() <= limit);
        ids.extend(documents.iter().map(|d| d.id));
        totals.push(total);
        match next {
            Some(cursor) => after = Some(cursor),
            None => return (ids, totals),
        }
    }
}

/// `ids` in the order of a page: the latest change first, then by id.
async fn in_page_order(stores: &Stores, ws: Uuid, ids: &[Uuid]) -> Vec<Uuid> {
    let mut latest = Vec::new();
    for id in ids {
        let document = stores.documents.get_document(ws, *id, None).await.unwrap();
        latest.push(DocumentCursor::of(&document.unwrap()));
    }
    latest.sort_by(|a, b| b.at.cmp(&a.at).then(a.id.cmp(&b.id)));
    latest.into_iter().map(|c| c.id).collect()
}

/// The latest versions of a kind come by page, the most recently changed
/// first, all of them or those with every key named; a change moves a
/// document to the front and takes its keys with it; a delete takes it
/// out.
pub async fn pages_by_kind_and_key_newest_first(stores: &Stores) {
    let ws = Uuid::new_v4();
    let (every, odd, none) = (key(1, 1), key(2, 9), key(3, 3));
    let tasks: Vec<Uuid> = (0..5).map(|_| Uuid::new_v4()).collect();
    for (i, id) in tasks.iter().enumerate() {
        let keys = if i % 2 == 0 {
            vec![every, odd]
        } else {
            vec![every]
        };
        put(stores, keyed(draft(ws, *id, "task", "t"), &keys), 0).await;
    }
    put(
        stores,
        keyed(draft(ws, Uuid::new_v4(), "comment", "c"), &[every]),
        0,
    )
    .await;
    let elsewhere = draft(Uuid::new_v4(), Uuid::new_v4(), "task", "x");
    put(stores, keyed(elsewhere, &[every]), 0).await;

    let all = in_page_order(stores, ws, &tasks).await;
    assert_eq!(
        paged(stores, ws, "task", &[], 2).await,
        (all.clone(), vec![5; 3])
    );
    assert_eq!(
        paged(stores, ws, "task", &[every], 10).await,
        (all, vec![5])
    );
    let evens = [tasks[0], tasks[2], tasks[4]];
    let odd_ones = in_page_order(stores, ws, &evens).await;
    assert_eq!(paged(stores, ws, "task", &[odd], 1).await.0, odd_ones);
    assert_eq!(
        paged(stores, ws, "task", &[every, odd], 2).await.0,
        odd_ones
    );
    assert_eq!(
        paged(stores, ws, "task", &[every, none], 2).await,
        (vec![], vec![0])
    );
    assert_eq!(
        paged(stores, ws, "decision", &[], 2).await,
        (vec![], vec![0])
    );

    // Task 1 changes and keeps only `odd`: it comes first, and no longer
    // has `every`.
    put(stores, keyed(draft(ws, tasks[1], "task", "t2"), &[odd]), 1).await;
    let (with_odd, _) = paged(stores, ws, "task", &[odd], 10).await;
    assert_eq!(with_odd[0], tasks[1]);
    assert_eq!(with_odd.len(), 4);
    let (with_every, totals) = paged(stores, ws, "task", &[every], 10).await;
    assert!(!with_every.contains(&tasks[1]));
    assert_eq!(totals, vec![4]);
    assert_eq!(paged(stores, ws, "task", &[], 10).await.0[0], tasks[1]);

    assert!(
        stores
            .documents
            .delete_document(ws, tasks[2])
            .await
            .unwrap()
    );
    assert!(
        !paged(stores, ws, "task", &[odd], 10)
            .await
            .0
            .contains(&tasks[2])
    );
    assert_eq!(paged(stores, ws, "task", &[], 10).await.1, vec![4]);
}

/// A latest version restored without keys is unkeyed: listed in its
/// kind's order but found by no key until it is keyed, and keyed only from
/// the version read.
pub async fn unkeyed_documents_are_listed_until_keyed(stores: &Stores) {
    let (ws, id) = (Uuid::new_v4(), Uuid::new_v4());
    let docs = &stores.documents;
    let keyed_one = put(
        stores,
        keyed(draft(ws, Uuid::new_v4(), "task", "k"), &[key(1, 1)]),
        0,
    )
    .await;
    let (version, _) = draft(ws, id, "task", "old").into_version(1, at(1));
    docs.restore_document_versions(ws, id, vec![version], false, None)
        .await
        .unwrap();
    let unkeyed: Vec<Uuid> = docs
        .unkeyed_documents(ws)
        .await
        .unwrap()
        .iter()
        .map(|d| d.id)
        .collect();
    assert_eq!(unkeyed, vec![id]);
    assert_eq!(
        paged(stores, ws, "task", &[], 10).await.0,
        vec![keyed_one.id, id]
    );
    // A page with keys holds the unkeyed, for memory to check.
    assert_eq!(
        paged(stores, ws, "task", &[key(9, 9)], 10).await.0,
        vec![id]
    );
    assert!(
        paged(stores, ws, "comment", &[key(9, 9)], 10)
            .await
            .0
            .is_empty()
    );

    assert!(
        !docs
            .set_document_keys(ws, id, 2, vec![key(1, 1)])
            .await
            .unwrap()
    );
    assert!(
        docs.set_document_keys(ws, id, 1, vec![key(1, 1)])
            .await
            .unwrap()
    );
    assert!(docs.unkeyed_documents(ws).await.unwrap().is_empty());
    assert_eq!(
        paged(stores, ws, "task", &[key(1, 1)], 10).await.0,
        vec![keyed_one.id, id]
    );
    assert!(
        paged(stores, ws, "task", &[key(9, 9)], 10)
            .await
            .0
            .is_empty()
    );
    assert!(
        !docs
            .set_document_keys(ws, Uuid::new_v4(), 1, vec![])
            .await
            .unwrap()
    );
}
