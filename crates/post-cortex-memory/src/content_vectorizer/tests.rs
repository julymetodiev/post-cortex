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
//! download) and an in-memory `VectorStorage` fake.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use anyhow::Result;
use async_trait::async_trait;
use chrono::Utc;
use parking_lot::Mutex;
use uuid::Uuid;

use post_cortex_core::core::context_update::{
    ContextUpdate, EntityRelationship, RelationType, UpdateContent, UpdateType,
};
use post_cortex_core::session::active_session::ActiveSession;
use post_cortex_embeddings::{
    EmbeddingConfig, EmbeddingModelType, SearchMatch, VectorDbConfig, VectorMetadata,
};
use post_cortex_storage::traits::VectorStorage;

use super::{ContentVectorizer, ContentVectorizerConfig, SearchOptions};

/// In-memory persisted embeddings, keyed like RocksDB: (session, content id).
#[derive(Default)]
struct FakeStorage {
    rows: Mutex<HashMap<(String, String), (Vec<f32>, VectorMetadata)>>,
    fail_writes: AtomicBool,
    batch_calls: AtomicUsize,
}

impl FakeStorage {
    fn has(&self, session: Uuid, content_id: &str) -> bool {
        self.rows
            .lock()
            .contains_key(&(session.to_string(), content_id.to_string()))
    }

    fn len(&self) -> usize {
        self.rows.lock().len()
    }

    fn put(&self, vector: Vec<f32>, metadata: VectorMetadata) -> Result<String> {
        if self.fail_writes.load(Ordering::Relaxed) {
            anyhow::bail!("storage down");
        }
        let id = metadata.id.clone();
        self.rows
            .lock()
            .insert((metadata.source.clone(), id.clone()), (vector, metadata));
        Ok(id)
    }
}

#[async_trait]
impl VectorStorage for FakeStorage {
    async fn add_vector(&self, vector: Vec<f32>, metadata: VectorMetadata) -> Result<String> {
        self.put(vector, metadata)
    }

    async fn add_vectors_batch(
        &self,
        vectors: Vec<(Vec<f32>, VectorMetadata)>,
    ) -> Result<Vec<String>> {
        self.batch_calls.fetch_add(1, Ordering::Relaxed);
        vectors.into_iter().map(|(v, m)| self.put(v, m)).collect()
    }

    async fn search(&self, _query: &[f32], _k: usize) -> Result<Vec<SearchMatch>> {
        Ok(Vec::new())
    }

    async fn search_in_session(
        &self,
        _query: &[f32],
        _k: usize,
        _session_id: &str,
    ) -> Result<Vec<SearchMatch>> {
        Ok(Vec::new())
    }

    async fn search_by_content_type(
        &self,
        _query: &[f32],
        _k: usize,
        _content_type: &str,
    ) -> Result<Vec<SearchMatch>> {
        Ok(Vec::new())
    }

    async fn remove_vector(&self, id: &str) -> Result<bool> {
        let mut rows = self.rows.lock();
        let before = rows.len();
        rows.retain(|(_, content_id), _| content_id != id);
        Ok(rows.len() != before)
    }

    async fn has_session_embeddings(&self, session_id: &str) -> bool {
        self.count_session_embeddings(session_id).await > 0
    }

    async fn count_session_embeddings(&self, session_id: &str) -> usize {
        self.rows
            .lock()
            .keys()
            .filter(|(sid, _)| sid == session_id)
            .count()
    }

    async fn total_count(&self) -> usize {
        self.len()
    }

    async fn get_session_vectors(
        &self,
        session_id: &str,
    ) -> Result<Vec<(Vec<f32>, VectorMetadata)>> {
        Ok(self
            .rows
            .lock()
            .iter()
            .filter(|((sid, _), _)| sid == session_id)
            .map(|(_, v)| v.clone())
            .collect())
    }

    async fn get_all_vectors(&self) -> Result<Vec<(Vec<f32>, VectorMetadata)>> {
        Ok(self.rows.lock().values().cloned().collect())
    }
}

async fn vectorizer() -> (ContentVectorizer, Arc<FakeStorage>) {
    let model_type = EmbeddingModelType::StaticSimilarityMRL;
    let config = ContentVectorizerConfig {
        embedding_config: EmbeddingConfig {
            model_type,
            ..Default::default()
        },
        vector_db_config: VectorDbConfig {
            dimension: model_type.embedding_dimension(),
            max_vectors: usize::MAX,
            ..Default::default()
        },
        ..Default::default()
    };
    let storage = Arc::new(FakeStorage::default());
    let vectorizer = ContentVectorizer::new(config)
        .await
        .expect("hash backend needs no model")
        .with_persistent_storage(Arc::clone(&storage) as Arc<dyn VectorStorage>);
    (vectorizer, storage)
}

fn update(title: &str, entity: &str) -> ContextUpdate {
    ContextUpdate {
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
    }
}

async fn session_with(titles: &[(&str, &str)]) -> ActiveSession {
    let mut session = ActiveSession::new(Uuid::new_v4(), None, None);
    for (title, entity) in titles {
        session
            .add_incremental_update(update(title, entity))
            .await
            .unwrap();
    }
    session
}

fn cache_len(v: &ContentVectorizer) -> usize {
    v.query_cache.as_ref().map_or(0, |c| c.len())
}

#[tokio::test]
async fn write_to_a_session_invalidates_global_cached_results() {
    let (v, _storage) = vectorizer().await;
    let a = session_with(&[("alpha topic", "alpha")]).await;
    v.vectorize_session(&a).await.unwrap();

    let first = v
        .semantic_search("alpha", 10, None, SearchOptions::default())
        .await
        .unwrap();
    assert!(!first.is_empty());
    assert_eq!(cache_len(&v), 1);

    // Another session gets new content: the global entry must go.
    let b = session_with(&[("beta topic", "beta")]).await;
    v.vectorize_session(&b).await.unwrap();
    assert_eq!(cache_len(&v), 0, "global cache entry survived a write");

    let second = v
        .semantic_search("alpha", 10, None, SearchOptions::default())
        .await
        .unwrap();
    assert!(second.iter().any(|r| r.session_id == b.id()));
}

#[tokio::test]
async fn cache_hit_skips_search_and_is_scoped() {
    let (v, _storage) = vectorizer().await;
    let a = session_with(&[("alpha topic", "alpha")]).await;
    let b = session_with(&[("beta topic", "beta")]).await;
    v.vectorize_session(&a).await.unwrap();
    v.vectorize_session(&b).await.unwrap();

    v.semantic_search("topic", 10, Some(a.id()), SearchOptions::default())
        .await
        .unwrap();
    v.semantic_search_multisession("topic", 10, &[b.id(), a.id()], SearchOptions::default())
        .await
        .unwrap();
    // Same session set in another order shares the entry.
    let hit = v
        .semantic_search_multisession("topic", 10, &[a.id(), b.id()], SearchOptions::default())
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
    let (v, _storage) = vectorizer().await;
    let session = session_with(&[("short", "alpha"), ("x", "beta")]).await;
    let sid = session.id().to_string();

    assert!(v.needs_vectorization(&session));
    v.vectorize_session(&session).await.unwrap();
    let after_first = v.count_session_embeddings(session.id());
    assert!(after_first > 0);
    assert!(!v.needs_vectorization(&session), "complete run marks it");

    // A forced second run adds nothing: entity descriptions are unchanged.
    let added = v.vectorize_session(&session).await.unwrap();
    assert_eq!(added, 0);
    assert_eq!(v.count_session_embeddings(session.id()), after_first);
    assert_eq!(
        super::vectorizer::source_content_ids(&v.vector_db, "entity:hub", &sid).len(),
        1
    );

    // New content makes the session pending again.
    let mut grown = session.clone();
    grown
        .add_incremental_update(update("gamma topic", "gamma"))
        .await
        .unwrap();
    assert!(v.needs_vectorization(&grown));
}

#[tokio::test]
async fn changed_entity_description_replaces_old_vector() {
    let (v, storage) = vectorizer().await;
    let mut session = session_with(&[("alpha topic", "alpha")]).await;
    let sid = session.id().to_string();
    v.vectorize_session(&session).await.unwrap();
    let old = super::vectorizer::source_content_ids(&v.vector_db, "entity:hub", &sid);
    assert_eq!(old.len(), 1);

    // "hub" gains a relationship and a mention: its description changes.
    session
        .add_incremental_update(update("beta topic", "beta"))
        .await
        .unwrap();
    v.vectorize_session(&session).await.unwrap();

    let new = super::vectorizer::source_content_ids(&v.vector_db, "entity:hub", &sid);
    assert_eq!(new.len(), 1, "exactly one current copy");
    assert_ne!(new, old);
    assert!(storage.has(session.id(), "entity:hub"));
}

#[tokio::test]
async fn failed_persist_rolls_back_memory_add() {
    let (v, storage) = vectorizer().await;
    let session = session_with(&[("alpha topic", "alpha"), ("beta topic", "beta")]).await;

    storage.fail_writes.store(true, Ordering::Relaxed);
    v.vectorize_session(&session).await.unwrap();
    assert_eq!(v.count_session_embeddings(session.id()), 0);
    assert!(session.vectorized_update_ids.is_empty());
    assert!(
        v.needs_vectorization(&session),
        "failed run must be retried"
    );

    storage.fail_writes.store(false, Ordering::Relaxed);
    v.vectorize_session(&session).await.unwrap();
    assert_eq!(session.vectorized_update_ids.len(), 2);
    assert_eq!(
        v.count_session_embeddings(session.id()),
        storage
            .count_session_embeddings(&session.id().to_string())
            .await
    );
    // Update vectors are persisted as one batch.
    assert!(storage.batch_calls.load(Ordering::Relaxed) >= 1);
}

#[tokio::test]
async fn remove_content_is_session_scoped_and_persists() {
    let (v, storage) = vectorizer().await;
    let a = session_with(&[("alpha topic", "shared")]).await;
    let b = session_with(&[("beta topic", "shared")]).await;
    v.vectorize_session(&a).await.unwrap();
    v.vectorize_session(&b).await.unwrap();
    let update_id = a.incremental_updates[0].id.to_string();

    v.semantic_search("topic", 10, None, SearchOptions::default())
        .await
        .unwrap();
    assert_eq!(cache_len(&v), 1);

    let removed = v.remove_content(a.id(), &update_id).await.unwrap();
    assert_eq!(removed, 1);
    assert!(!v.vector_db.has_update_embedding(&update_id));
    assert!(!storage.has(a.id(), &update_id));
    assert_eq!(cache_len(&v), 0);

    // Entity ids repeat across sessions: only a's in-memory copy goes.
    let removed = v.remove_content(a.id(), "entity:shared").await.unwrap();
    assert_eq!(removed, 1);
    let b_sid = b.id().to_string();
    assert_eq!(
        super::vectorizer::source_content_ids(&v.vector_db, "entity:shared", &b_sid).len(),
        1
    );
}

#[tokio::test]
async fn remove_session_vectors_and_reload_without_duplicates() {
    let (v, storage) = vectorizer().await;
    let a = session_with(&[("alpha topic", "alpha")]).await;
    v.vectorize_session(&a).await.unwrap();
    let count = v.count_session_embeddings(a.id());
    assert!(count > 0);

    // Loading what is already in memory adds nothing.
    assert_eq!(v.load_all_embeddings_from_storage().await.unwrap(), 0);
    assert_eq!(v.count_session_embeddings(a.id()), count);

    assert_eq!(v.remove_session_vectors(a.id()), count);
    assert_eq!(v.count_session_embeddings(a.id()), 0);
    assert!(v.needs_vectorization(&a));

    // Persisted copies are untouched and load back in bulk.
    assert_eq!(storage.len(), count);
    assert_eq!(v.load_all_embeddings_from_storage().await.unwrap(), count);
}
