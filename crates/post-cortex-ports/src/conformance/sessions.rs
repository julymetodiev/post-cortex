// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Sessions: what is saved loads back the same, listings report the stored
//! session, and a replace replaces.

use crate::storage::{SessionChanges, Stores};
use chrono::{DateTime, Utc};
use uuid::Uuid;

use post_cortex_core::core::context_update::{CodeReference, ContextUpdate};
use post_cortex_core::graph::entity_graph::SimpleEntityGraph;
use post_cortex_core::session::active_session::{ActiveSession, StoredSession, UserPreferences};
use post_cortex_core::session::session_components::SessionMetadata;

use crate::conformance::{at, record};

/// A session's graph as comparable values: entities (name, type, counts,
/// times, description, aliases) and edges, both sorted.
pub type GraphView = (
    Vec<(
        String,
        String,
        u32,
        DateTime<Utc>,
        DateTime<Utc>,
        Option<String>,
        Vec<String>,
    )>,
    Vec<(String, String, String, String)>,
);

/// `graph` as a [`GraphView`].
pub fn graph_view(graph: &SimpleEntityGraph) -> GraphView {
    let mut entities: Vec<_> = graph
        .get_all_entities()
        .into_iter()
        .map(|e| {
            let mut aliases = e.aliases.clone();
            aliases.sort();
            (
                e.name,
                e.entity_type.as_str().to_string(),
                e.mention_count,
                e.first_mentioned,
                e.last_mentioned,
                e.description,
                aliases,
            )
        })
        .collect();
    entities.sort_by(|a, b| a.0.cmp(&b.0));
    let mut edges: Vec<_> = graph
        .get_all_relationships()
        .into_iter()
        .map(|r| {
            (
                r.from_entity,
                r.to_entity,
                r.relation_type.as_str().to_string(),
                r.context,
            )
        })
        .collect();
    edges.sort();
    (entities, edges)
}

/// A session with name, description, preferences, records (one with code,
/// one with record metadata), a graph and vectorized ids, last updated at
/// `at(500)` (after its records).
pub fn rich_session() -> StoredSession {
    let mut with_code = record(10, "storage", &["pcx", "RocksDB"]);
    with_code.related_code = Some(CodeReference {
        file_path: "src/lib.rs".into(),
        start_line: 1,
        end_line: 3,
        code_snippet: "fn main() {}".into(),
        commit_hash: Some("abc".into()),
        branch: Some("main".into()),
        change_description: "added".into(),
    });
    let mut retired = record(20, "schema", &["pcx", "SurrealDB"]);
    retired.meta.confidence = Some(0.5);
    let plain = record(30, "plain", &[]);
    let records = vec![with_code, retired, plain];

    let mut preferences = UserPreferences::default();
    preferences.auto_save_enabled = !preferences.auto_save_enabled;
    let metadata = SessionMetadata::new(
        Uuid::new_v4(),
        Some("rich".into()),
        Some("all the fields".into()),
        preferences,
        at(0),
    );
    let mut session =
        ActiveSession::from_records(metadata, at(0), Vec::new(), SimpleEntityGraph::new());
    for r in &records {
        session.apply_record(r.clone());
    }
    let session = ActiveSession::from_records(
        (*session.metadata()).clone(),
        at(500),
        session.records_owned(),
        session.entity_graph().clone(),
    );
    StoredSession::new(session, vec![records[0].id])
}

fn assert_same_session(loaded: &StoredSession, saved: &StoredSession) {
    let mut ids = (loaded.vectorized.clone(), saved.vectorized.clone());
    ids.0.sort();
    ids.1.sort();
    assert_eq!(ids.0, ids.1);
    let (loaded, saved) = (&loaded.session, &saved.session);
    let (l, s) = (loaded.metadata(), saved.metadata());
    assert_eq!(
        (l.id, &l.name, &l.description, l.created_at),
        (s.id, &s.name, &s.description, s.created_at)
    );
    assert_eq!(
        serde_json::to_value(&l.user_preferences).unwrap(),
        serde_json::to_value(&s.user_preferences).unwrap()
    );
    assert_eq!(loaded.last_updated(), saved.last_updated());
    let json = |u: &post_cortex_core::collections::SharedVec<std::sync::Arc<ContextUpdate>>| {
        serde_json::to_value(u).unwrap()
    };
    assert_eq!(json(loaded.records()), json(saved.records()));
    assert_eq!(loaded.current_state(), saved.current_state());
    assert_eq!(
        serde_json::to_value(loaded.code_references()).unwrap(),
        serde_json::to_value(saved.code_references()).unwrap()
    );
    assert_eq!(
        graph_view(loaded.entity_graph()),
        graph_view(saved.entity_graph())
    );
}

/// Every field of a session, its structured context and its graph come
/// back from a save.
pub async fn round_trip_keeps_the_session(stores: &Stores) {
    let stored = rich_session();
    let session = &stored.session;
    stores
        .sessions
        .save_session(
            session,
            SessionChanges::everything(session, &stored.vectorized),
        )
        .await
        .unwrap();
    let loaded = stores.sessions.load_session(session.id()).await.unwrap();
    assert_same_session(&loaded, &stored);
    assert_eq!(
        loaded
            .session
            .records()
            .iter()
            .map(|u| u.id)
            .collect::<Vec<_>>(),
        session.records().iter().map(|u| u.id).collect::<Vec<_>>()
    );
}

/// A listing reports what the session says, its last update time included
/// (not the time of the save).
pub async fn summary_reports_the_stored_session(stores: &Stores) {
    let stored = rich_session();
    let session = &stored.session;
    stores
        .sessions
        .save_session(
            session,
            SessionChanges::everything(session, &stored.vectorized),
        )
        .await
        .unwrap();
    let summary = stores
        .sessions
        .list_session_summaries()
        .await
        .unwrap()
        .into_iter()
        .find(|s| s.id == session.id())
        .expect("listed");
    assert_eq!(summary.name.as_deref(), Some("rich"));
    assert_eq!(summary.created_at, at(0));
    assert_eq!(summary.last_updated, at(500));
    assert_eq!(summary.update_count, 3);
}

/// Replacing a session leaves exactly the new one: records, graph,
/// vectors and messages of the old one are gone; checkpoints stay.
pub async fn replace_replaces_the_session(adapter: &crate::conformance::Adapter) {
    let stores: &Stores = adapter;
    let old = rich_session();
    let id = old.session.id();
    stores
        .sessions
        .save_session(
            &old.session,
            SessionChanges::everything(&old.session, &old.vectorized),
        )
        .await
        .unwrap();
    adapter
        .vectors
        .upsert_vectors(vec![(
            vec![0.5; 256],
            crate::vectors::VectorMetadata {
                id: "update:old".into(),
                text: "old".into(),
                source: id.to_string(),
                content_type: "UpdateContent".into(),
                timestamp: at(1),
                metadata: Default::default(),
            },
        )])
        .await
        .unwrap();
    stores
        .messages
        .append_messages(
            id,
            vec![post_cortex_core::core::message::NewMessage {
                turn_id: "t1".into(),
                role: post_cortex_core::core::message::MessageRole::User,
                author: None,
                source_session: None,
                timestamp: at(2),
                text: "hello".into(),
                tools: vec![],
                files: vec![],
                parent: None,
                blocks: Vec::new(),
            }],
        )
        .await
        .unwrap();
    let checkpoint = crate::conformance::checkpoints::checkpoint(id);
    stores
        .checkpoints
        .save_checkpoint(&checkpoint)
        .await
        .unwrap();

    let mut new = ActiveSession::new(id, Some("new".into()), None, at(0));
    new.apply_record(record(40, "only this", &["Only"]));
    stores.sessions.replace_session(&new, &[]).await.unwrap();

    let loaded = stores.sessions.load_session(id).await.unwrap();
    assert_same_session(&loaded, &StoredSession::new(new, Vec::new()));
    assert_eq!(loaded.session.records().len(), 1);
    assert!(
        adapter
            .vectors
            .get_session_vectors(&id.to_string())
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        stores
            .messages
            .load_messages(id, 0..10)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        stores
            .checkpoints
            .list_checkpoints(id)
            .await
            .unwrap()
            .iter()
            .map(|c| c.id)
            .collect::<Vec<_>>(),
        [checkpoint.id]
    );
}
