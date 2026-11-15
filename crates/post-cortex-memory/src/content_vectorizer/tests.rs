// Copyright (c) 2025 Julius ML
//
// Permission is hereby granted, free of charge, to any person obtaining a copy
// of this software and associated documentation files (the "Software"), to deal
// in the Software without restriction, including without limitation the rights
// to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
// copies of the Software, and to permit persons to whom the Software is
// furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in all
// copies or substantial portions of the Software.

//! Vectorizer tests. They use the hash-based embedding backend (no model
//! download) and an in-memory `VectorStore` fake.

use crate::content_vectorizer::VectorScope;
use crate::memory_system::VectorizedIds;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use chrono::Utc;
use uuid::Uuid;

use post_cortex_core::core::context_update::{
    ContextUpdate, EntityRelationship, RelationType, UpdateContent, UpdateType,
};
use post_cortex_core::session::active_session::ActiveSession;
use post_cortex_embeddings::MemoryIndex;
use post_cortex_embeddings::test_support::hash_engine;
use post_cortex_ports::{EntrySelect, VectorStore};

use super::{ContentVectorizer, ContentVectorizerConfig, SearchOptions};
use crate::content_vectorizer::test_doubles::FakeStorage;

/// Vectorizer on an index (with its in-memory BM25) over the fake storage.
pub(super) async fn vectorizer() -> (ContentVectorizer, Arc<FakeStorage>) {
    let storage = Arc::new(FakeStorage::default());
    let index = MemoryIndex::new(DIM, Some(Arc::clone(&storage) as Arc<dyn VectorStore>));
    let vectorizer = ContentVectorizer::new(
        ContentVectorizerConfig::default(),
        Arc::new(hash_engine(DIM)),
        Arc::new(index),
    );
    (vectorizer, storage)
}

/// How many entries of `session` have exactly `content_id`.
pub(super) async fn copies(v: &ContentVectorizer, session: Uuid, content_id: &str) -> usize {
    v.index()
        .entries(
            &session.to_string(),
            &EntrySelect::ContentIds(vec![content_id.to_string()]),
        )
        .await
        .unwrap()
        .iter()
        .filter(|e| e.meta.id == content_id)
        .count()
}

/// Dimension of the hash embeddings these tests use.
pub(super) const DIM: usize = 256;

pub(super) fn update(title: &str, entity: &str) -> ContextUpdate {
    ContextUpdate {
        meta: Default::default(),
        id: Uuid::new_v4(),
        timestamp: Utc::now(),
        update_type: UpdateType::ConceptDefined,
        content: UpdateContent {
            title: title.to_string(),
            description: format!("{title} described in enough words to be embedded"),
            details: vec![],
            examples: vec![],
            implications: vec![],
        },
        related_code: None,
        parent_update: None,
        user_marked_important: false,
        creates_entities: vec![entity.to_string(), "hub".to_string()],
        creates_relationships: vec![EntityRelationship {
            from_entity: entity.to_string(),
            to_entity: "hub".to_string(),
            relation_type: RelationType::DependsOn,
            context: String::new(),
        }],
        references_entities: vec![],
        typed_entities: vec![],
        author: None,
        derived_from: Vec::new(),
    }
}

pub(super) async fn session_with(titles: &[(&str, &str)]) -> ActiveSession {
    let mut session = ActiveSession::new(Uuid::new_v4(), None, None, chrono::Utc::now());
    for (title, entity) in titles {
        session.apply_record(update(title, entity));
    }
    session
}

pub(super) fn cache_len(v: &ContentVectorizer) -> usize {
    v.query_cache().map_or(0, |c| c.len())
}

#[tokio::test]
async fn write_to_a_session_invalidates_global_cached_results() {
    let marks = VectorizedIds::default();
    let (v, _storage) = vectorizer().await;
    let a = session_with(&[("alpha topic", "alpha")]).await;
    v.vectorize_session(&a, &marks).await.unwrap();

    let first = v
        .search("alpha", 10, VectorScope::All, SearchOptions::default())
        .await
        .unwrap();
    assert!(!first.is_empty());
    assert_eq!(cache_len(&v), 1);

    // Another session gets new content: the global entry must go.
    let b = session_with(&[("beta topic", "beta")]).await;
    v.vectorize_session(&b, &marks).await.unwrap();
    assert_eq!(cache_len(&v), 0, "global cache entry survived a write");

    let second = v
        .search("alpha", 10, VectorScope::All, SearchOptions::default())
        .await
        .unwrap();
    assert!(second.iter().any(|r| r.source.session() == Some(b.id())));
}

#[tokio::test]
async fn cache_hit_skips_search_and_is_scoped() {
    let marks = VectorizedIds::default();
    let (v, _storage) = vectorizer().await;
    let a = session_with(&[("alpha topic", "alpha")]).await;
    let b = session_with(&[("beta topic", "beta")]).await;
    v.vectorize_session(&a, &marks).await.unwrap();
    v.vectorize_session(&b, &marks).await.unwrap();

    v.search(
        "topic",
        10,
        VectorScope::Sessions(&[a.id()]),
        SearchOptions::default(),
    )
    .await
    .unwrap();
    v.search(
        "topic",
        10,
        VectorScope::Sessions(&[b.id(), a.id()]),
        SearchOptions::default(),
    )
    .await
    .unwrap();
    // Same session set in another order shares the entry.
    let hit = v
        .search(
            "topic",
            10,
            VectorScope::Sessions(&[a.id(), b.id()]),
            SearchOptions::default(),
        )
        .await
        .unwrap();
    assert!(!hit.is_empty());
    assert_eq!(cache_len(&v), 2);
    let stats = v.get_query_cache_stats().unwrap();
    assert_eq!(stats.cache_hits, 1);

    // A write to b leaves a's single-session entry alone.
    v.invalidate_cached(b.id());
    assert_eq!(cache_len(&v), 1);
}

#[tokio::test]
async fn revectorizing_does_not_duplicate_entities_and_marks_session() {
    let marks = VectorizedIds::default();
    let (v, _storage) = vectorizer().await;
    let session = session_with(&[("short", "alpha"), ("x", "beta")]).await;

    assert!(v.needs_vectorization(&session));
    v.vectorize_session(&session, &marks).await.unwrap();
    let after_first = v.count_session_embeddings(session.id()).await.unwrap();
    assert!(after_first > 0);
    assert!(!v.needs_vectorization(&session), "complete run marks it");

    // A forced second run adds nothing: entity descriptions are unchanged.
    let added = v.vectorize_session(&session, &marks).await.unwrap();
    assert_eq!(added, 0);
    assert_eq!(
        v.count_session_embeddings(session.id()).await.unwrap(),
        after_first
    );
    assert_eq!(copies(&v, session.id(), "entity:hub").await, 1);

    // New content makes the session pending again.
    let mut grown = session.clone();
    grown.apply_record(update("gamma topic", "gamma"));
    assert!(v.needs_vectorization(&grown));
}

#[tokio::test]
async fn changed_entity_description_replaces_old_vector() {
    let marks = VectorizedIds::default();
    let (v, storage) = vectorizer().await;
    let mut session = session_with(&[("alpha topic", "alpha")]).await;
    v.vectorize_session(&session, &marks).await.unwrap();
    let old = hub_text(&v, session.id()).await;

    // "hub" gains a relationship and a mention: its description changes.
    session.apply_record(update("beta topic", "beta"));
    v.vectorize_session(&session, &marks).await.unwrap();

    let new = hub_text(&v, session.id()).await;
    assert_ne!(new, old);
    assert!(storage.has(session.id(), "entity:hub"));
}

/// The text of the one entry of `entity:hub` of `session`.
async fn hub_text(v: &ContentVectorizer, session: Uuid) -> String {
    let entries = v
        .index()
        .entries(
            &session.to_string(),
            &EntrySelect::ContentIds(vec!["entity:hub".into()]),
        )
        .await
        .unwrap();
    assert_eq!(entries.len(), 1, "exactly one current copy");
    entries[0].meta.text.to_string()
}

#[tokio::test]
async fn failed_persist_rolls_back_memory_add() {
    let marks = VectorizedIds::default();
    let (v, storage) = vectorizer().await;
    let session = session_with(&[("alpha topic", "alpha"), ("beta topic", "beta")]).await;

    storage.fail_writes.store(true, Ordering::Relaxed);
    v.vectorize_session(&session, &marks).await.unwrap();
    assert_eq!(v.count_session_embeddings(session.id()).await.unwrap(), 0);
    assert!(marks.is_empty());
    assert!(
        v.needs_vectorization(&session),
        "failed run must be retried"
    );

    storage.fail_writes.store(false, Ordering::Relaxed);
    v.vectorize_session(&session, &marks).await.unwrap();
    assert_eq!(marks.len(), 2);
    assert_eq!(
        v.count_session_embeddings(session.id()).await.unwrap(),
        storage
            .count_session_embeddings(&session.id().to_string())
            .await
            .unwrap()
    );
    // Update vectors are persisted as one batch.
    assert!(storage.batch_calls.load(Ordering::Relaxed) >= 1);
}

#[tokio::test]
async fn remove_content_is_session_scoped_and_persists() {
    let marks = VectorizedIds::default();
    let (v, storage) = vectorizer().await;
    let a = session_with(&[("alpha topic", "shared")]).await;
    let b = session_with(&[("beta topic", "shared")]).await;
    v.vectorize_session(&a, &marks).await.unwrap();
    v.vectorize_session(&b, &marks).await.unwrap();
    let update_id = a.records()[0].id.to_string();

    v.search("topic", 10, VectorScope::All, SearchOptions::default())
        .await
        .unwrap();
    assert_eq!(cache_len(&v), 1);

    let removed = v
        .remove_contents(a.id(), std::slice::from_ref(&update_id))
        .await
        .unwrap();
    assert_eq!(removed, 1);
    assert_eq!(copies(&v, a.id(), &update_id).await, 0);
    assert!(!storage.has(a.id(), &update_id));
    assert_eq!(cache_len(&v), 0);

    // Entity ids repeat across sessions: only a's in-memory copy goes.
    let removed = v
        .remove_contents(a.id(), &["entity:shared".to_string()])
        .await
        .unwrap();
    assert_eq!(removed, 1);
    assert_eq!(copies(&v, b.id(), "entity:shared").await, 1);
    // ...and only a's persisted copy: b's must survive a restart.
    assert!(!storage.has(a.id(), "entity:shared"));
    assert!(storage.has(b.id(), "entity:shared"));
}

#[tokio::test]
async fn remove_session_vectors_removes_its_entries() {
    let marks = VectorizedIds::default();
    let (v, storage) = vectorizer().await;
    let a = session_with(&[("alpha topic", "alpha")]).await;
    let b = session_with(&[("beta topic", "beta")]).await;
    v.vectorize_session(&a, &marks).await.unwrap();
    v.vectorize_session(&b, &marks).await.unwrap();
    let count = v.count_session_embeddings(a.id()).await.unwrap();
    assert!(count > 0);

    assert_eq!(v.remove_session_vectors(a.id()).await.unwrap(), count);
    assert_eq!(v.count_session_embeddings(a.id()).await.unwrap(), 0);
    assert!(v.needs_vectorization(&a));
    // The other session keeps its entries, in the index and in storage.
    assert_eq!(
        storage.len(),
        v.count_session_embeddings(b.id()).await.unwrap()
    );
}
