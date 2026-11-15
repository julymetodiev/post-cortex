// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use super::*;
use crate::surrealdb_storage::SurrealDBStorage;
use post_cortex_core::session::active_session::ActiveSession;
use post_cortex_ports::storage::SessionStore;

/// The edge id is built from the wire names alone: the same on every
/// toolchain (it was a `DefaultHasher` over the `Debug` name).
#[test]
fn edge_ids_are_stable() {
    let session = Uuid::from_u128(1);
    assert_eq!(
        crate::surrealdb_storage::ids::relation_id(session, "a_b", "c", &RelationType::DependsOn),
        "00000000-0000-0000-0000-000000000001_v1_depends_on_3:a_b_c"
    );
    // The length prefix keeps `a_b`+`c` and `a`+`b_c` apart.
    assert_ne!(
        crate::surrealdb_storage::ids::relation_id(session, "a_b", "c", &RelationType::DependsOn),
        crate::surrealdb_storage::ids::relation_id(session, "a", "b_c", &RelationType::DependsOn)
    );
}

async fn edge_keys(storage: &SurrealDBStorage, table: &str) -> Vec<String> {
    let mut response = storage
        .db()
        .query(format!("SELECT VALUE meta::id(id) FROM {table};"))
        .await
        .unwrap();
    let mut keys: Vec<String> = response.take(0).unwrap();
    keys.sort();
    keys
}

/// Edges stored under the old hashed ids are re-keyed once (duplicates
/// merge, orphans go) and the graph loads as before; the version is
/// recorded and a second run changes nothing.
#[tokio::test]
async fn old_edge_ids_are_rekeyed() {
    let storage = SurrealDBStorage::new("mem://", None, None, None, None)
        .await
        .unwrap();
    let mut session = ActiveSession::new(Uuid::new_v4(), None, None, chrono::Utc::now());
    session.apply_record(post_cortex_ports::conformance::record(1, "ab", &["A", "B"]));
    session.apply_record(post_cortex_ports::conformance::record(2, "bc", &["B", "C"]));
    storage
        .save_session(
            &session,
            post_cortex_ports::storage::SessionChanges::everything(&session, &[]),
        )
        .await
        .unwrap();
    let sid = session.id();
    let want = |from: &str, to: &str| {
        crate::surrealdb_storage::ids::relation_id(sid, from, to, &RelationType::DependsOn)
    };

    // What 0.5.0 left: the edges under hashed ids, one of them twice, and
    // an orphan whose source entity is gone.
    storage
        .db()
        .query(
            "DELETE depends_on; \
             FOR $e IN $edges { UPSERT type::record('depends_on', $e[0]) SET \
                in = type::record('entity', $e[1]), out = type::record('entity', $e[2]), \
                context = 'uses', session_id = $sid; }; \
             DELETE pcx_meta:schema;",
        )
        .bind(("sid", sid.to_string()))
        .bind((
            "edges",
            vec![
                vec![
                    format!("{sid}_aaaa"),
                    format!("{sid}_A"),
                    format!("{sid}_B"),
                ],
                vec![
                    format!("{sid}_bbbb"),
                    format!("{sid}_A"),
                    format!("{sid}_B"),
                ],
                vec![
                    format!("{sid}_cccc"),
                    format!("{sid}_B"),
                    format!("{sid}_C"),
                ],
                vec![
                    format!("{sid}_dddd"),
                    format!("{sid}_Gone"),
                    format!("{sid}_C"),
                ],
            ],
        ))
        .await
        .unwrap()
        .check()
        .unwrap();
    assert_eq!(
        crate::surrealdb_storage::migrate::data_version(&storage.db())
            .await
            .unwrap(),
        0
    );

    crate::surrealdb_storage::migrate::migrate(&storage.db(), &storage.name)
        .await
        .unwrap();
    let mut expected = vec![want("A", "B"), want("B", "C")];
    expected.sort();
    assert_eq!(edge_keys(&storage, "depends_on").await, expected);
    assert_eq!(
        crate::surrealdb_storage::migrate::data_version(&storage.db())
            .await
            .unwrap(),
        DATA_VERSION
    );
    let loaded = storage.load_session(sid).await.unwrap().session;
    assert_eq!(loaded.entity_graph().get_all_relationships().len(), 2);

    crate::surrealdb_storage::migrate::migrate(&storage.db(), &storage.name)
        .await
        .unwrap();
    assert_eq!(edge_keys(&storage, "depends_on").await, expected);
}

/// Version 3: a document stored before keys (no order rows) gets its place
/// in its kind's order, unkeyed.
#[tokio::test]
async fn version_2_documents_are_ordered_and_unkeyed() {
    use post_cortex_core::core::document::{DocumentData, DocumentDraft, DocumentKind};
    use post_cortex_ports::storage::{DocumentStore, LatestQuery};

    let storage = SurrealDBStorage::new("mem://", None, None, None, None)
        .await
        .unwrap();
    let (ws, id) = (Uuid::new_v4(), Uuid::new_v4());
    let kind: DocumentKind = "board/task".parse().unwrap();
    let draft = DocumentDraft {
        workspace_id: ws,
        id,
        kind: kind.clone(),
        data: DocumentData::Message {
            type_name: "pcx.board.v1.Task".into(),
            bytes: vec![10, 1, b'a'],
        },
        title: "a".into(),
        text: String::new(),
        author: None,
        keys: Vec::new(),
    };
    storage.put_document(draft, 0).await.unwrap();
    // What version 2 left: the document, no order.
    storage
        .db()
        .query(
            "DELETE document_order; DELETE document_key; DELETE document_unkeyed; \
             UPSERT pcx_meta:schema SET version = 2;",
        )
        .await
        .unwrap()
        .check()
        .unwrap();
    let query = LatestQuery {
        workspace_id: ws,
        kind: &kind,
        keys: &[],
        after: None,
        limit: 10,
    };
    assert_eq!(storage.latest_page(query).await.unwrap().total, 0);

    crate::surrealdb_storage::migrate::migrate(&storage.db(), &storage.name)
        .await
        .unwrap();
    let page = storage.latest_page(query).await.unwrap();
    assert_eq!(
        page.documents.iter().map(|d| d.id).collect::<Vec<_>>(),
        [id]
    );
    let unkeyed = storage.unkeyed_documents(ws).await.unwrap();
    assert_eq!(unkeyed.iter().map(|d| d.id).collect::<Vec<_>>(), [id]);
}

/// A database of a newer build is refused.
#[tokio::test]
async fn a_newer_data_version_is_refused() {
    let storage = SurrealDBStorage::new("mem://", None, None, None, None)
        .await
        .unwrap();
    storage
        .db()
        .query("UPSERT pcx_meta:schema SET version = 99;")
        .await
        .unwrap()
        .check()
        .unwrap();
    let err = crate::surrealdb_storage::migrate::migrate(&storage.db(), &storage.name)
        .await
        .unwrap_err();
    assert!(format!("{err:#}").contains("newer"), "{err:#}");
}

/// Step 4: workspaces stored before slugs get theirs, the oldest first (a
/// later one of the same name carries its id), and then the slug is unique.
#[tokio::test]
async fn workspaces_get_their_slugs() {
    use post_cortex_ports::storage::WorkspaceStore;
    let storage = SurrealDBStorage::new("mem://", None, None, None, None)
        .await
        .unwrap();
    let (older, newer) = (Uuid::from_u128(7), Uuid::from_u128(8));
    // As data version 3 left them: no slug and no index.
    storage
        .db()
        .query(
            "REMOVE INDEX idx_workspace_slug ON workspace; REMOVE FIELD slug ON workspace; \
             CREATE type::record('workspace', $newer) CONTENT { workspace_id: $newer, name: 'Checkout', description: '', created_at: 20 }; \
             CREATE type::record('workspace', $older) CONTENT { workspace_id: $older, name: 'Checkout', description: '', created_at: 10 }; \
             DEFINE FIELD slug ON workspace TYPE string; \
             UPSERT pcx_meta:schema SET version = 3;",
        )
        .bind(("older", older.to_string()))
        .bind(("newer", newer.to_string()))
        .await
        .unwrap()
        .check()
        .unwrap();
    migrate(&storage.db(), "test").await.unwrap();
    let slugs: std::collections::HashMap<Uuid, String> = storage
        .list_workspaces()
        .await
        .unwrap()
        .into_iter()
        .map(|w| (w.id, w.slug))
        .collect();
    let hex = newer.simple().to_string();
    assert_eq!(slugs[&older], "checkout");
    assert_eq!(slugs[&newer], format!("checkout-{}", &hex[..6]));
    let twin = storage
        .db()
        .query("CREATE workspace CONTENT { workspace_id: 'x', name: 'x', slug: 'checkout', description: '', created_at: 1 };")
        .await
        .unwrap()
        .check();
    assert!(twin.is_err(), "the slug is unique");
}

/// Data version 3 to 5 in one start, as production went (2026-11-07): the
/// schema of this build is defined first, then the slug step updates rows
/// that have no remote_rerank yet, which must not fail for its NONE.
#[tokio::test]
async fn workspaces_of_version_3_reach_version_5() {
    use post_cortex_ports::storage::WorkspaceStore;
    let storage = SurrealDBStorage::new("mem://", None, None, None, None)
        .await
        .unwrap();
    let id = Uuid::from_u128(11);
    // As data version 3 left it: no slug, no remote_rerank.
    storage
        .db()
        .query(
            "REMOVE INDEX idx_workspace_slug ON workspace; REMOVE FIELD slug ON workspace; \
             REMOVE FIELD remote_rerank ON workspace; \
             CREATE type::record('workspace', $id) CONTENT { workspace_id: $id, name: 'Checkout', description: '', created_at: 10 }; \
             UPSERT pcx_meta:schema SET version = 3;",
        )
        .bind(("id", id.to_string()))
        .await
        .unwrap()
        .check()
        .unwrap();
    crate::surrealdb_storage::schema::initialize(&storage.db())
        .await
        .unwrap();
    migrate(&storage.db(), "test").await.unwrap();
    assert_eq!(data_version(&storage.db()).await.unwrap(), DATA_VERSION);
    let ws = storage.list_workspaces().await.unwrap().remove(0);
    assert_eq!((ws.id, ws.slug.as_str()), (id, "checkout"));
    assert!(!ws.remote_rerank);
}

/// Step 5: a workspace stored before remote reranking forbids the remote
/// reranker and keeps the rest of its row.
#[tokio::test]
async fn workspaces_forbid_the_remote_reranker() {
    use post_cortex_ports::storage::WorkspaceStore;
    let storage = SurrealDBStorage::new("mem://", None, None, None, None)
        .await
        .unwrap();
    let id = Uuid::from_u128(9);
    // As data version 4 left it: no remote_rerank.
    storage
        .db()
        .query(
            "REMOVE FIELD remote_rerank ON workspace; \
             CREATE type::record('workspace', $id) CONTENT { workspace_id: $id, name: 'Checkout', slug: 'checkout', description: '', created_at: 10 }; \
             DEFINE FIELD remote_rerank ON workspace TYPE bool DEFAULT false; \
             UPSERT pcx_meta:schema SET version = 4;",
        )
        .bind(("id", id.to_string()))
        .await
        .unwrap()
        .check()
        .unwrap();
    migrate(&storage.db(), "test").await.unwrap();
    let ws = storage.list_workspaces().await.unwrap().remove(0);
    assert_eq!((ws.id, ws.slug.as_str()), (id, "checkout"));
    assert!(!ws.remote_rerank);
}
