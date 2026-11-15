// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! One pipeline for every scope: the scope is resolved once (unknown ids
//! are typed errors, an empty set finds nothing), a workspace is its
//! sessions, and a hit's text is the record's own.

use std::time::Duration;

use super::*;
use crate::capabilities::records::good_request;
use crate::capabilities::{CreateWorkspace, Memory};
use crate::content_vectorizer::{ContentKind, SearchOptions};
use post_cortex_core::workspace::SessionRole;

async fn memory() -> (Memory, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let memory = crate::memory_system::backends_tests::rocksdb_memory_in(
        &dir,
        crate::SystemConfig {
            ..Default::default()
        },
    )
    .await
    .unwrap();
    (memory, dir)
}

async fn write(memory: &Memory, session_id: Uuid, title: &str, description: &str) {
    let mut req = good_request();
    req.session_id = session_id;
    req.content.title = title.into();
    req.content.description = description.into();
    memory.records.write(req).await.unwrap();
}

#[tokio::test]
async fn every_scope_runs_one_pipeline() {
    let (memory, _dir) = memory().await;
    let search = |scope| {
        memory
            .search
            .search(scope, "lighthouse keeper", SearchOptions::default())
    };

    let ghost = Uuid::new_v4();
    assert!(matches!(
        search(SearchScope::Session(ghost)).await,
        Err(SearchError::SessionNotFound(id)) if id == ghost
    ));
    assert!(matches!(
        search(SearchScope::Workspace(ghost)).await,
        Err(SearchError::WorkspaceNotFound(id)) if id == ghost
    ));
    assert!(
        search(SearchScope::Sessions(Vec::new()))
            .await
            .unwrap()
            .is_empty()
    );

    let sid = memory.sessions.create(None, None).await.unwrap();
    write(
        &memory,
        sid,
        "Lighthouse keeper",
        "the keeper rings the bell at dawn",
    )
    .await;
    assert!(memory.records.wait_idle(Duration::from_secs(120)).await);
    let ws = memory
        .workspaces
        .create(CreateWorkspace {
            name: "coast".into(),
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

    let record_ids = |hits: Vec<crate::content_vectorizer::SemanticSearchResult>| {
        let mut ids: Vec<String> = hits
            .into_iter()
            .filter(|h| h.content_type != ContentKind::EntityDescription)
            .map(|h| h.content_id)
            .collect();
        ids.sort();
        ids
    };
    let in_workspace = record_ids(search(SearchScope::Workspace(ws)).await.unwrap());
    let in_sessions = record_ids(search(SearchScope::Sessions(vec![sid])).await.unwrap());
    assert!(!in_workspace.is_empty());
    assert_eq!(in_workspace, in_sessions, "a workspace is its sessions");
}

/// A session search returns the record's own text: the graph "enrichment"
/// that rewrote hit text in session scope only is gone.
#[tokio::test]
async fn a_hit_carries_the_records_text() {
    let (memory, _dir) = memory().await;
    let sid = memory.sessions.create(None, None).await.unwrap();
    let text = "the foo service calls the bar service on every request";
    write(&memory, sid, "Foo calls Bar", text).await;
    assert!(memory.records.wait_idle(Duration::from_secs(120)).await);
    let hits = memory
        .search
        .search(
            SearchScope::Session(sid),
            "foo bar service",
            SearchOptions::default(),
        )
        .await
        .unwrap();
    let record = hits
        .iter()
        .find(|h| h.content_type != ContentKind::EntityDescription)
        .expect("the record is found");
    assert!(
        record.text_content.contains(text),
        "{}",
        record.text_content
    );
    for hit in &hits {
        assert!(
            !hit.text_content.contains("[System Knowledge Map]")
                && !hit.text_content.contains("(Graph expansion"),
            "{}",
            hit.text_content
        );
    }
}

#[tokio::test]
async fn with_embeddings_off_every_scope_is_refused_alike() {
    let dir = tempfile::tempdir().unwrap();
    let memory = crate::memory_system::backends_tests::rocksdb_memory_in(
        &dir,
        crate::SystemConfig {
            enable_embeddings: false,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let session = memory.sessions.create(None, None).await.unwrap();
    let workspace = memory
        .workspaces
        .create(CreateWorkspace {
            name: "w".into(),
            description: String::new(),
        })
        .await
        .unwrap()
        .id;
    for scope in [
        SearchScope::Global,
        SearchScope::Session(session),
        SearchScope::Sessions(vec![session]),
        SearchScope::Workspace(workspace),
    ] {
        let refused = memory
            .search
            .search(scope.clone(), "anything", SearchOptions::default())
            .await;
        assert!(
            matches!(refused, Err(SearchError::EmbeddingsDisabled)),
            "{scope:?}: {refused:?}"
        );
    }
    let related = memory.search.related(session, "anything", None).await;
    assert!(matches!(related, Err(SearchError::EmbeddingsDisabled)));
}

/// `update_types` keeps only records of those interaction types, by each
/// record's own type, and still fills the limit from the deeper ranking
/// (audit 2026-10-22: the filter was accepted and ignored).
#[tokio::test]
async fn interaction_types_filter_by_the_records_own_type() {
    use post_cortex_core::core::context_update::UpdateType;

    let (memory, _dir) = memory().await;
    let sid = memory.sessions.create(None, None).await.unwrap();
    for (i, kind) in [UpdateType::DecisionMade, UpdateType::ConceptDefined]
        .into_iter()
        .cycle()
        .take(6)
        .enumerate()
    {
        let mut req = good_request();
        req.session_id = sid;
        req.interaction_type = kind;
        req.content.title = format!("Harbour note {i}");
        req.content.description = format!("the harbour master logs note {i}");
        memory.records.write(req).await.unwrap();
    }
    assert!(memory.records.wait_idle(Duration::from_secs(120)).await);

    let options = SearchOptions {
        limit: Some(2),
        update_types: vec![UpdateType::DecisionMade],
        ..Default::default()
    };
    let hits = memory
        .search
        .search(SearchScope::Session(sid), "harbour master note", options)
        .await
        .unwrap();
    let hits: Vec<_> = hits.iter().filter(|h| h.neighbour_of.is_none()).collect();
    assert_eq!(hits.len(), 2);
    assert!(
        hits.iter()
            .all(|h| h.content_type == ContentKind::DecisionPoint),
        "{hits:?}"
    );
}
