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
use post_cortex_core::core::message::{Message, MessageRole, NewMessage};
use post_cortex_core::session::active_session::ActiveSession;
use post_cortex_embeddings::{
    Corpus, EmbeddingConfig, EmbeddingModelType, Reranker, SearchMatch, VectorDbConfig,
    VectorMetadata,
};
use post_cortex_storage::traits::VectorStorage;

use super::{ContentType, ContentVectorizer, ContentVectorizerConfig, SearchOptions};

/// In-memory persisted embeddings, keyed like RocksDB: (session, content id).
#[derive(Default)]
struct FakeStorage {
    rows: Mutex<HashMap<(String, String), (Vec<f32>, VectorMetadata)>>,
    fail_writes: AtomicBool,
    batch_calls: AtomicUsize,
    /// When set, `search_text` answers like a full-text backend with these
    /// content ids, in this order (restricted to the requested sessions).
    full_text: Mutex<Option<Vec<String>>>,
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

    async fn search_text(
        &self,
        _query: &str,
        _query_vector: &[f32],
        k: usize,
        sources: &[String],
        corpus: Corpus,
    ) -> Result<Option<Vec<SearchMatch>>> {
        let Some(ids) = self.full_text.lock().clone() else {
            return Ok(None);
        };
        let rows = self.rows.lock();
        Ok(Some(
            ids.iter()
                .flat_map(|id| rows.values().filter(move |(_, m)| &m.id == id))
                .filter(|(_, m)| sources.is_empty() || sources.contains(&m.source))
                .filter(|(_, m)| corpus.admits(&m.content_type))
                .take(k)
                .map(|(_, metadata)| SearchMatch {
                    vector_id: 0,
                    similarity: 0.0,
                    metadata: metadata.clone(),
                })
                .collect(),
        ))
    }

    async fn remove_vector(&self, id: &str) -> Result<bool> {
        let mut rows = self.rows.lock();
        let before = rows.len();
        rows.retain(|(_, content_id), _| content_id != id);
        Ok(rows.len() != before)
    }

    async fn remove_session_vector(&self, session_id: &str, content_id: &str) -> Result<bool> {
        Ok(self
            .rows
            .lock()
            .remove(&(session_id.to_string(), content_id.to_string()))
            .is_some())
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
    vectorizer_with(false).await
}

/// Vectorizer on the fake storage; `full_text_index` turns on the vector
/// DB's in-memory BM25 index (what RocksDB-backed daemons use).
async fn vectorizer_with(full_text_index: bool) -> (ContentVectorizer, Arc<FakeStorage>) {
    let model_type = EmbeddingModelType::StaticSimilarityMRL;
    let config = ContentVectorizerConfig {
        embedding_config: EmbeddingConfig {
            model_type,
            ..Default::default()
        },
        vector_db_config: VectorDbConfig {
            dimension: model_type.embedding_dimension(),
            max_vectors: usize::MAX,
            full_text_index,
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
        author: None,
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
    // ...and only a's persisted copy: b's must survive a restart.
    assert!(!storage.has(a.id(), "entity:shared"));
    assert!(storage.has(b.id(), "entity:shared"));
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

/// A record the vector search does not rank in the top `limit` comes back
/// once the full-text backend ranks it first, and results never exceed
/// `limit`. Without a full-text index the vector ranking is used as is.
#[tokio::test]
async fn full_text_hits_are_fused_with_vector_results() {
    let (v, storage) = vectorizer().await;
    let titles: Vec<(String, String)> = (0..30)
        .map(|i| (format!("filler topic number {i}"), format!("e{i}")))
        .chain([("zebracorn migration plan".to_string(), "zebra".to_string())])
        .collect();
    let refs: Vec<(&str, &str)> = titles
        .iter()
        .map(|(t, e)| (t.as_str(), e.as_str()))
        .collect();
    let session = session_with(&refs).await;
    v.vectorize_session(&session).await.unwrap();
    let zebra = session
        .incremental_updates
        .iter()
        .find(|u| u.content.title.starts_with("zebracorn"))
        .unwrap()
        .id
        .to_string();
    let opts = || SearchOptions {
        recency_bias: Some(0.0),
        ..SearchOptions::default()
    };
    let has_zebra = |rs: &[super::SemanticSearchResult]| rs.iter().any(|r| r.content_id == zebra);

    let vector_only = v
        .semantic_search("filler topic number", 3, Some(session.id()), opts())
        .await
        .unwrap();
    assert_eq!(vector_only.len(), 3);
    assert!(!has_zebra(&vector_only));

    *storage.full_text.lock() = Some(vec![zebra.clone()]);
    // Words on most records are not searched for, so the query needs one
    // that is not ("migration").
    let fused = v
        .semantic_search(
            "filler topic number migration",
            3,
            Some(session.id()),
            opts(),
        )
        .await
        .unwrap();
    assert_eq!(fused.len(), 3);
    assert!(
        has_zebra(&fused),
        "{:?}",
        fused.iter().map(|r| &r.text_content).collect::<Vec<_>>()
    );

    // Other sessions' full-text hits stay out of a session-scoped search.
    let other = session_with(&[("unrelated", "u")]).await;
    let fused_other = v
        .semantic_search("filler topic number", 3, Some(other.id()), opts())
        .await
        .unwrap();
    assert!(!has_zebra(&fused_other));
}

/// Scores a document by whether it contains `needle`; fails when `needle`
/// is empty.
struct FakeReranker {
    needle: &'static str,
}

#[async_trait]
impl Reranker for FakeReranker {
    async fn score(&self, _query: &str, documents: &[String]) -> Result<Vec<f32>> {
        anyhow::ensure!(!self.needle.is_empty(), "reranker down");
        Ok(documents
            .iter()
            .map(|d| if d.contains(self.needle) { 1.0 } else { 0.0 })
            .collect())
    }
}

/// The reranker reorders the whole candidate pool, not just the top
/// `limit`: a record the vector search ranks below `limit` comes first once
/// the reranker prefers it. A failing reranker leaves the fused order.
#[tokio::test]
async fn reranker_reorders_the_candidate_pool() {
    let titles: Vec<(String, String)> = (0..30)
        .map(|i| (format!("filler topic number {i}"), format!("e{i}")))
        .chain([("zebracorn migration plan".to_string(), "zebra".to_string())])
        .collect();
    let refs: Vec<(&str, &str)> = titles
        .iter()
        .map(|(t, e)| (t.as_str(), e.as_str()))
        .collect();
    let session = session_with(&refs).await;
    let opts = || SearchOptions {
        recency_bias: Some(0.0),
        ..SearchOptions::default()
    };
    let texts = |rs: &[super::SemanticSearchResult]| {
        rs.iter()
            .map(|r| r.text_content.clone())
            .collect::<Vec<_>>()
    };

    let (mut v, _) = vectorizer().await;
    v.set_reranker(Arc::new(FakeReranker {
        needle: "zebracorn",
    }));
    v.vectorize_session(&session).await.unwrap();
    let reranked = v
        .semantic_search("filler topic number", 3, Some(session.id()), opts())
        .await
        .unwrap();
    assert_eq!(reranked.len(), 3);
    assert!(
        reranked[0].text_content.contains("zebracorn"),
        "{:?}",
        texts(&reranked)
    );

    let (mut down, _) = vectorizer().await;
    down.set_reranker(Arc::new(FakeReranker { needle: "" }));
    down.vectorize_session(&session).await.unwrap();
    let fallback = down
        .semantic_search("filler topic number", 3, Some(session.id()), opts())
        .await
        .unwrap();
    assert_eq!(fallback.len(), 3);
    assert!(
        !fallback[0].text_content.contains("zebracorn"),
        "{:?}",
        texts(&fallback)
    );
}

/// Each mode uses its own reranker, `accurate` is the default when a remote
/// reranker is set, and the query cache keeps modes apart.
#[tokio::test]
async fn search_modes_pick_their_reranker() {
    let titles: Vec<(String, String)> = (0..30)
        .map(|i| (format!("filler topic number {i}"), format!("e{i}")))
        .chain([
            ("zebracorn migration plan".to_string(), "zebra".to_string()),
            ("quokka release notes".to_string(), "quokka".to_string()),
        ])
        .collect();
    let refs: Vec<(&str, &str)> = titles
        .iter()
        .map(|(t, e)| (t.as_str(), e.as_str()))
        .collect();
    let session = session_with(&refs).await;
    let (mut v, _) = vectorizer().await;
    v.set_reranker(Arc::new(FakeReranker {
        needle: "zebracorn",
    }));
    v.set_accurate_reranker(Arc::new(FakeReranker { needle: "quokka" }));
    v.vectorize_session(&session).await.unwrap();

    use super::SearchMode::{Accurate, Fast, Local};
    let id = session.id();
    assert!(top(&v, id, Some(Local)).await.contains("zebracorn"));
    assert!(top(&v, id, Some(Accurate)).await.contains("quokka"));
    assert!(
        top(&v, id, None).await.contains("quokka"),
        "default is accurate"
    );
    let fast = top(&v, id, Some(Fast)).await;
    assert!(
        !fast.contains("zebracorn") && !fast.contains("quokka"),
        "{fast}"
    );

    v.set_default_mode(Local);
    assert!(top(&v, id, None).await.contains("zebracorn"));
}

/// Text of the best result for "filler topic number" in `mode`.
async fn top(v: &ContentVectorizer, session: Uuid, mode: Option<super::SearchMode>) -> String {
    let opts = SearchOptions {
        recency_bias: Some(0.0),
        mode,
        ..SearchOptions::default()
    };
    v.semantic_search("filler topic number", 3, Some(session), opts)
        .await
        .unwrap()[0]
        .text_content
        .clone()
}

/// Vectorizing stores each update's graph terms and agent entities in the
/// vector's metadata, and `graph_records` reads them back for the session's
/// retrieval graph.
#[tokio::test]
async fn update_vectors_carry_graph_terms_and_entities() {
    use post_cortex_core::graph::retrieval_graph::RetrievalGraph;

    let titles: Vec<(String, String)> = (0..40)
        .map(|i| (format!("filler topic number {i}"), format!("e{i}")))
        .chain([
            (
                "Caroline adopted a puppy called Biscuit".to_string(),
                "puppy".to_string(),
            ),
            (
                "Biscuit chewed the sofa yesterday".to_string(),
                "sofa".to_string(),
            ),
        ])
        .collect();
    let refs: Vec<(&str, &str)> = titles
        .iter()
        .map(|(t, e)| (t.as_str(), e.as_str()))
        .collect();
    let session = session_with(&refs).await;
    let (v, _) = vectorizer().await;
    v.vectorize_session(&session).await.unwrap();

    let records = v.graph_records(session.id());
    assert_eq!(records.len(), 42, "entity vectors are not records");
    let adopted = records
        .iter()
        .find(|r| r.terms.contains("biscuit") && r.terms.contains("adopt"))
        .expect("terms stored with the vector");
    assert!(
        adopted.entities.iter().any(|e| e == "puppy"),
        "{:?}",
        adopted.entities
    );

    let id_of = |title: &str| {
        session
            .incremental_updates
            .iter()
            .find(|u| u.content.title == title)
            .unwrap()
            .id
            .to_string()
    };
    let ranked = RetrievalGraph::build(records, &[]).rank("What did Caroline's puppy chew?", 5);
    assert_eq!(
        ranked.first(),
        Some(&id_of("Caroline adopted a puppy called Biscuit"))
    );
    assert!(
        ranked.contains(&id_of("Biscuit chewed the sofa yesterday")),
        "second hop"
    );
}

/// With a storage backend that has no full-text index (RocksDB), BM25 comes
/// from the vector DB's in-memory index, scoped to the session.
#[tokio::test]
async fn in_memory_bm25_when_storage_has_none() {
    let titles: Vec<(String, String)> = (0..30)
        .map(|i| (format!("filler topic number {i}"), format!("e{i}")))
        .chain([("zebracorn migration plan".to_string(), "zebra".to_string())])
        .collect();
    let refs: Vec<(&str, &str)> = titles
        .iter()
        .map(|(t, e)| (t.as_str(), e.as_str()))
        .collect();
    let session = session_with(&refs).await;
    let other = session_with(&[("zebracorn in another session", "z")]).await;
    let sources = [session.id().to_string()];
    let query_vector = vec![0.0; EmbeddingModelType::StaticSimilarityMRL.embedding_dimension()];

    let (plain, _) = vectorizer_with(false).await;
    plain.vectorize_session(&session).await.unwrap();
    let terms = plain.vector_db.query_terms("zebracorn");
    assert!(
        plain
            .vector_db
            .search_text(&terms, &query_vector, 5, &sources, Corpus::Records)
            .is_none()
    );

    let (v, storage) = vectorizer_with(true).await;
    v.vectorize_session(&session).await.unwrap();
    v.vectorize_session(&other).await.unwrap();
    // "filler" is on most records, so it is not searched for.
    let terms = v.vector_db.query_terms("filler zebracorn");
    assert_eq!(terms, ["zebracorn"]);
    let hits = v
        .vector_db
        .search_text(&terms, &query_vector, 5, &sources, Corpus::Records)
        .unwrap();
    assert!(
        hits[0].metadata.text.contains("zebracorn migration"),
        "{hits:?}"
    );
    assert!(hits.iter().all(|h| h.metadata.source == sources[0]));

    // The fake storage reports no full-text index, so search falls back to
    // the in-memory one: the rare word is found.
    assert!(storage.full_text.lock().is_none());
    let results = v
        .semantic_search(
            "zebracorn",
            10,
            Some(session.id()),
            SearchOptions {
                recency_bias: Some(0.0),
                ..SearchOptions::default()
            },
        )
        .await
        .unwrap();
    assert!(results.iter().any(|r| r.text_content.contains("zebracorn")));
}

/// A long update is embedded as chunks, so a passage far past the first
/// chunk is found; the hit is the update itself (one result, the update's
/// id), and removing the update removes every chunk.
#[tokio::test]
async fn long_updates_are_chunked_and_found_whole() {
    let mut long = update("design notes", "notes");
    let filler: String = (0..300)
        .map(|i| format!("ordinary sentence {i}. "))
        .collect();
    long.content.description = format!("{filler} The quokkaplan deadline is Friday. {filler}");
    let mut session = ActiveSession::new(Uuid::new_v4(), None, None);
    session.add_incremental_update(long.clone()).await.unwrap();
    for i in 0..5 {
        session
            .add_incremental_update(update(&format!("short note {i}"), &format!("n{i}")))
            .await
            .unwrap();
    }

    let (v, _) = vectorizer_with(true).await;
    v.vectorize_session(&session).await.unwrap();
    let long_id = long.id.to_string();
    let chunk_ids: Vec<String> = (2..)
        .map(|n| format!("{long_id}#{n}"))
        .take_while(|id| v.vector_db.find_vector_id_by_content_id(id).is_some())
        .collect();
    assert!(chunk_ids.len() > 3, "long update split into chunks");

    let results = v
        .semantic_search(
            "quokkaplan deadline",
            5,
            Some(session.id()),
            SearchOptions {
                recency_bias: Some(0.0),
                ..SearchOptions::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(
        results[0].content_id, long_id,
        "the chunk answers for the update"
    );
    assert_eq!(
        results.iter().filter(|r| r.content_id == long_id).count(),
        1,
        "one result per update"
    );

    v.remove_content(session.id(), &long_id).await.unwrap();
    assert!(v.vector_db.find_vector_id_by_content_id(&long_id).is_none());
    assert!(
        chunk_ids
            .iter()
            .all(|id| v.vector_db.find_vector_id_by_content_id(id).is_none())
    );
}

fn chat(session_id: Uuid, seq: u64, text: &str) -> Message {
    NewMessage {
        turn_id: format!("turn-{seq}"),
        role: if seq.is_multiple_of(2) {
            MessageRole::User
        } else {
            MessageRole::Assistant
        },
        author: None,
        source_session: None,
        timestamp: Utc::now(),
        text: text.to_string(),
        tools: Vec::new(),
        files: Vec::new(),
    }
    .into_message(session_id, seq)
}

/// Messages are embedded as `Message` vectors under `message:<seq>`, are
/// left out of searches unless asked for, and do not count as vectorized
/// updates of the session.
#[tokio::test]
async fn messages_are_searched_only_when_asked_for() {
    let session = session_with(&[("zebracorn migration plan", "zebra")]).await;
    let (v, storage) = vectorizer_with(true).await;
    v.vectorize_session(&session).await.unwrap();
    let messages = [
        chat(session.id(), 0, "what happened to the zebracorn yesterday?"),
        chat(session.id(), 1, "the zebracorn import failed on a timeout"),
    ];
    assert_eq!(v.vectorize_messages(&messages).await.unwrap(), 2);
    assert_eq!(
        v.vectorize_messages(&messages).await.unwrap(),
        0,
        "turns with vectors are skipped"
    );
    assert!(
        storage
            .rows
            .lock()
            .contains_key(&(session.id().to_string(), "message:1".to_string())),
        "message vectors are persisted"
    );
    assert!(
        !v.needs_vectorization(&session),
        "messages are not updates waiting for vectors"
    );

    let search = |corpus: Corpus| {
        v.semantic_search(
            "zebracorn",
            10,
            Some(session.id()),
            SearchOptions {
                recency_bias: Some(0.0),
                corpus,
                ..SearchOptions::default()
            },
        )
    };
    let records = search(Corpus::Records).await.unwrap();
    assert!(!records.is_empty());
    assert!(
        records
            .iter()
            .all(|r| r.content_type != ContentType::Message),
        "{records:?}"
    );
    let mut ids: Vec<String> = search(Corpus::Messages)
        .await
        .unwrap()
        .into_iter()
        .inspect(|r| assert_eq!(r.content_type, ContentType::Message))
        .map(|r| r.content_id)
        .collect();
    ids.sort();
    assert_eq!(ids, ["message:0", "message:1"]);
    assert_eq!(search(Corpus::All).await.unwrap().len(), records.len() + 2);

    v.remove_content(session.id(), "message:0").await.unwrap();
    let left = search(Corpus::Messages).await.unwrap();
    assert_eq!(left.len(), 1);
    assert_eq!(left[0].content_id, "message:1");

    // A turn whose vector is missing is embedded again; the other is not.
    assert_eq!(v.vectorize_messages(&messages).await.unwrap(), 1);
    assert_eq!(search(Corpus::Messages).await.unwrap().len(), 2);
}

/// Records carry their author into the vector index: results show it and a
/// search can keep one author's records only.
#[tokio::test]
async fn records_are_filtered_by_author() {
    let mut session = ActiveSession::new(Uuid::new_v4(), None, None);
    for (title, author) in [
        ("zebracorn rollout plan by alice", Some("alice")),
        ("zebracorn rollback notes by bob", Some("bob")),
        ("zebracorn history from before authorship", None),
    ] {
        let mut u = update(title, "zebra");
        u.author = author.map(String::from);
        session.add_incremental_update(u).await.unwrap();
    }
    let (v, _) = vectorizer_with(true).await;
    v.vectorize_session(&session).await.unwrap();

    let search = |author: Option<&str>| {
        v.semantic_search(
            "zebracorn",
            10,
            Some(session.id()),
            SearchOptions {
                recency_bias: Some(0.0),
                author: author.map(String::from),
                ..SearchOptions::default()
            },
        )
    };
    let all = search(None).await.unwrap();
    let mut authors: Vec<Option<String>> = all
        .iter()
        .filter(|r| r.content_type != ContentType::EntityDescription)
        .map(|r| r.author.clone())
        .collect();
    authors.sort();
    assert_eq!(authors, [None, Some("alice".into()), Some("bob".into())]);

    let alice = search(Some("alice")).await.unwrap();
    assert_eq!(alice.len(), 1, "{alice:?}");
    assert!(alice[0].text_content.contains("by alice"));
    assert!(search(Some("carol")).await.unwrap().is_empty());
}
