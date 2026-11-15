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

//! Vectorization ingestion: full-session, incremental, entity, plus text preparation helpers.

use crate::content_vectorizer::vector_text::build_entity_description;
use crate::content_vectorizer::vector_text::extract_text_from_update;
use crate::content_vectorizer::vector_text::update_vectors;
use post_cortex_ports::error::Result;
use rayon::prelude::*;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tracing::debug;
use uuid::Uuid;

use post_cortex_core::core::context_update::ContextUpdate;
use post_cortex_core::session::active_session::ActiveSession;

use crate::content_vectorizer::ContentVectorizer;
use crate::content_vectorizer::types::ContentKind;
use crate::memory_system::VectorizedIds;
use post_cortex_ports::{EntrySelect, IndexEntry, VectorMetadata};

/// Threshold for switching to parallel processing.
/// Below this threshold, sequential processing is faster due to reduced overhead.
const PARALLEL_PROCESSING_THRESHOLD: usize = 50;

impl ContentVectorizer {
    /// Vectorize all content in a session.
    ///
    /// # Errors
    /// Returns an error if vectorization fails or the vector database errors out.
    pub async fn vectorize_session(
        &self,
        session: &ActiveSession,
        vectorized: &VectorizedIds,
    ) -> Result<usize> {
        debug!("Vectorizing session: {}", session.id());
        let mut vectorized_count = 0;

        let (update_count, updates_complete) =
            self.vectorize_context_updates(session, vectorized).await?;
        vectorized_count += update_count;

        let (entity_count, entities_complete) = if self.config().enable_entity_vectorization {
            let (c, complete) = self.vectorize_entities(session, None).await?;
            vectorized_count += c;
            (c, complete)
        } else {
            (0, true)
        };

        if vectorized_count > 0 {
            self.invalidate_cached(session.id());
        }
        // Only a run where nothing failed may let later searches skip this
        // session; otherwise the next search retries.
        if updates_complete && entities_complete {
            self.mark_vectorized(session);
        }

        debug!(
            "Vectorized {} items for session {} (updates={}, entities={})",
            vectorized_count,
            session.id(),
            update_count,
            entity_count
        );
        Ok(vectorized_count)
    }

    /// Vectorize the records `update_ids` of a session with one
    /// `encode_batch` and one store batch. Background jobs name the records
    /// they were queued for: by the time a job runs, later writes may have
    /// landed. Records already vectorized, gone or too short are skipped; a
    /// record whose vectors were all stored is marked vectorized. Returns
    /// how many vectors were added.
    ///
    /// # Errors
    /// Returns an error if the embedding engine fails.
    pub async fn vectorize_updates(
        &self,
        session: &ActiveSession,
        vectorized: &VectorizedIds,
        update_ids: &[Uuid],
    ) -> Result<usize> {
        let mut wanted: HashSet<Uuid> = update_ids
            .iter()
            .filter(|id| !vectorized.contains(id))
            .copied()
            .collect();
        // One pass, newest first: queued records are usually recent.
        let mut vectors_of: HashMap<Uuid, usize> = HashMap::new();
        let mut texts = Vec::new();
        let mut metadata = Vec::new();
        for update in session.records().iter().rev() {
            if wanted.is_empty() {
                break;
            }
            if !wanted.remove(&update.id) {
                continue;
            }
            let raw_text = extract_text_from_update(update);
            if !crate::content_vectorizer::vector_text::worth_embedding(self, &raw_text) {
                continue;
            }
            let vectors = update_vectors(
                session.id(),
                update,
                &raw_text,
                self.config().max_text_length,
            );
            vectors_of.insert(update.id, vectors.len());
            for (text, meta) in vectors {
                texts.push(text);
                metadata.push(meta);
            }
        }
        if !wanted.is_empty() {
            debug!(
                "vectorize_updates: {} record(s) not in session {} (or too short)",
                wanted.len(),
                session.id()
            );
        }
        if texts.is_empty() {
            return Ok(0);
        }

        let attempted = metadata.len();
        let embeddings = self.embedding_engine().embed(texts).await?;
        if embeddings.len() != attempted {
            tracing::warn!(
                "{} embedding(s) for {attempted} text(s) of session {}",
                embeddings.len(),
                session.id()
            );
            return Ok(0);
        }

        let added = self.store_vectors(entries(embeddings, metadata)).await;
        // Chunks `<id>#<n>` count towards their record.
        let mut stored: HashMap<Uuid, usize> = HashMap::new();
        for content_id in &added {
            let parent = content_id
                .split_once('#')
                .map_or(content_id.as_str(), |(p, _)| p);
            if let Ok(id) = Uuid::parse_str(parent) {
                *stored.entry(id).or_default() += 1;
            }
        }
        for (id, count) in stored {
            if vectors_of.get(&id) == Some(&count) {
                vectorized.mark(id);
            }
        }
        if !added.is_empty() {
            self.invalidate_cached(session.id());
        }
        debug!(
            "Vectorized {} record(s) of session {} ({} of {attempted} vectors)",
            vectors_of.len(),
            session.id(),
            added.len()
        );
        Ok(added.len())
    }

    /// Vectorize context updates from a session. Returns how many were added
    /// and whether every collected update was added.
    async fn vectorize_context_updates(
        &self,
        session: &ActiveSession,
        vectorized: &VectorizedIds,
    ) -> Result<(usize, bool)> {
        // The vectorized ids are persisted with the session, the vectors in
        // the index: after a restart or a lost write they can disagree, so
        // each id is checked against the stored entries.
        let stored = self.stored_update_ids(session, vectorized).await?;
        let stale_ids: Vec<Uuid> = vectorized
            .ids()
            .into_iter()
            .filter(|id| !stored.contains(&id.to_string()))
            .collect();

        if !stale_ids.is_empty() {
            debug!(
                "Removing {} stale vectorized_update_ids for session {} (not in the index)",
                stale_ids.len(),
                session.id()
            );
            for stale_id in stale_ids {
                vectorized.unmark(&stale_id);
            }
        }

        // Updates whose vectors are stored but whose ids the session lost
        // (it was not written after their embedding): mark, never re-embed.
        for update in session.records().iter() {
            if !vectorized.contains(&update.id) && stored.contains(&update.id.to_string()) {
                vectorized.mark(update.id);
            }
        }

        let total_updates = session.records().len();
        let vectorized_ids_count = vectorized.len();
        let non_vectorized_count = session
            .records()
            .iter()
            .filter(|u| !vectorized.contains(&u.id))
            .count();

        debug!(
            "vectorize_context_updates: session={}, total_updates={}, vectorized_ids={}, non_vectorized={}",
            session.id(),
            total_updates,
            vectorized_ids_count,
            non_vectorized_count,
        );

        let (texts_to_embed, metadata_list) =
            if non_vectorized_count >= PARALLEL_PROCESSING_THRESHOLD {
                debug!(
                    "Using parallel processing for {} updates (threshold: {})",
                    non_vectorized_count, PARALLEL_PROCESSING_THRESHOLD
                );
                self.collect_updates_parallel(session, vectorized)
            } else {
                debug!(
                    "Using sequential processing for {} updates",
                    non_vectorized_count
                );
                self.collect_updates_sequential(session, vectorized)
            };

        if texts_to_embed.is_empty() {
            debug!(
                "vectorize_context_updates: NO texts to embed for session {} (non_vectorized={}, collected=0 — texts too short or filtered)",
                session.id(),
                non_vectorized_count
            );
            return Ok((0, true));
        }

        let attempted = metadata_list.len();
        let embeddings = self.embedding_engine().embed(texts_to_embed).await?;

        let added = self.store_vectors(entries(embeddings, metadata_list)).await;
        for content_id in &added {
            if let Ok(update_id) = Uuid::parse_str(content_id) {
                vectorized.mark(update_id);
            }
        }

        debug!(
            "Added {} context update vectors, marked as vectorized",
            added.len()
        );
        Ok((added.len(), added.len() == attempted))
    }

    /// Whether the session has an update long enough to embed that has no
    /// vector.
    ///
    /// # Errors
    /// The index could not be read.
    pub async fn has_unembedded_updates(&self, session: &ActiveSession) -> Result<bool> {
        let stored = self
            .stored_update_ids(session, &VectorizedIds::default())
            .await?;
        Ok(session.records().iter().any(|update| {
            !stored.contains(&update.id.to_string())
                && crate::content_vectorizer::vector_text::worth_embedding(
                    self,
                    &extract_text_from_update(update),
                )
        }))
    }

    /// The content ids of the stored entries of the session's records and of
    /// `vectorized`'s ids (with their chunks).
    async fn stored_update_ids(
        &self,
        session: &ActiveSession,
        vectorized: &VectorizedIds,
    ) -> Result<HashSet<String>> {
        let mut ids: HashSet<String> = vectorized.ids().iter().map(Uuid::to_string).collect();
        ids.extend(session.records().iter().map(|u| u.id.to_string()));
        if ids.is_empty() {
            return Ok(HashSet::new());
        }
        let select = EntrySelect::ContentIds(ids.into_iter().collect());
        Ok(self
            .index()
            .entries(&session.id().to_string(), &select)
            .await?
            .into_iter()
            .map(|e| e.meta.id)
            .collect())
    }

    /// Vectorize entity descriptions. An entity whose description is
    /// already stored unchanged for this session is skipped; a changed one
    /// replaces its old vector. Returns how many were added and whether
    /// every one that needed (re-)embedding was added.
    pub(crate) async fn vectorize_entities(
        &self,
        session: &ActiveSession,
        only: Option<&HashSet<String>>,
    ) -> Result<(usize, bool)> {
        let wanted = |name: &String| only.is_none_or(|names| names.contains(name));
        let entity_count = session.entity_graph().entities.len();

        let (texts_to_embed, metadata_list) = if entity_count >= PARALLEL_PROCESSING_THRESHOLD {
            debug!(
                "Using parallel processing for {} entities (threshold: {})",
                entity_count, PARALLEL_PROCESSING_THRESHOLD
            );
            let results: Vec<(String, VectorMetadata)> = session
                .entity_graph()
                .entities
                .sorted()
                .into_par_iter()
                .filter(|(entity_name, _)| wanted(entity_name))
                .filter_map(|(entity_name, entity_data)| {
                    let entity_text = build_entity_description(session, entity_name, entity_data);
                    if crate::content_vectorizer::vector_text::worth_embedding(self, &entity_text) {
                        let metadata = VectorMetadata::new(
                            format!("entity:{entity_name}"),
                            entity_text.clone(),
                            session.id().to_string(),
                            ContentKind::EntityDescription.as_str().to_string(),
                        )
                        .with_timestamp(entity_data.last_mentioned);
                        Some((entity_text, metadata))
                    } else {
                        None
                    }
                })
                .collect();

            results.into_iter().unzip()
        } else {
            debug!("Using sequential processing for {} entities", entity_count);

            let mut texts_to_embed = Vec::new();
            let mut metadata_list = Vec::new();

            for (entity_name, entity_data) in session
                .entity_graph()
                .entities
                .sorted()
                .into_iter()
                .filter(|(entity_name, _)| wanted(entity_name))
            {
                let entity_text = build_entity_description(session, entity_name, entity_data);
                if crate::content_vectorizer::vector_text::worth_embedding(self, &entity_text) {
                    texts_to_embed.push(entity_text.clone());
                    metadata_list.push(
                        VectorMetadata::new(
                            format!("entity:{entity_name}"),
                            entity_text,
                            session.id().to_string(),
                            ContentKind::EntityDescription.as_str().to_string(),
                        )
                        .with_timestamp(entity_data.last_mentioned),
                    );
                }
            }

            (texts_to_embed, metadata_list)
        };

        // Keep only entities that are new or whose description changed.
        let source = session.id().to_string();
        let select = EntrySelect::ContentIds(metadata_list.iter().map(|m| m.id.clone()).collect());
        let mut stored: HashMap<String, Vec<Arc<str>>> = HashMap::new();
        if !metadata_list.is_empty() {
            for entry in self.index().entries(&source, &select).await? {
                stored
                    .entry(entry.meta.id)
                    .or_default()
                    .push(entry.meta.text);
            }
        }
        let (texts_to_embed, metadata_list): (Vec<String>, Vec<VectorMetadata>) = texts_to_embed
            .into_iter()
            .zip(metadata_list)
            .filter(|(_, metadata)| {
                let unchanged = stored
                    .get(&metadata.id)
                    .is_some_and(|texts| texts.len() == 1 && *texts[0] == *metadata.text);
                !unchanged
            })
            .unzip();

        if texts_to_embed.is_empty() {
            debug!(
                "No new or changed entity texts to vectorize for session {}",
                session.id()
            );
            return Ok((0, true));
        }

        let attempted = metadata_list.len();
        let embeddings = self.embedding_engine().embed(texts_to_embed).await?;

        // An entry replaces the stored one of its (session, content id).
        let added = self.store_vectors(entries(embeddings, metadata_list)).await;

        debug!("Added {} entity vectors", added.len());
        Ok((added.len(), added.len() == attempted))
    }

    /// Collect updates sequentially (for small sessions).
    /// Uses incremental_updates to ensure ALL updates are vectorized (including evicted ones).
    fn collect_updates_sequential(
        &self,
        session: &ActiveSession,
        vectorized: &VectorizedIds,
    ) -> (Vec<String>, Vec<VectorMetadata>) {
        let mut texts_to_embed = Vec::new();
        let mut metadata_list = Vec::new();

        for update in session.records().iter() {
            if vectorized.contains(&update.id) {
                continue;
            }

            let raw_text = extract_text_from_update(update);
            if crate::content_vectorizer::vector_text::worth_embedding(self, &raw_text) {
                for (text, metadata) in update_vectors(
                    session.id(),
                    update,
                    &raw_text,
                    self.config().max_text_length,
                ) {
                    texts_to_embed.push(text);
                    metadata_list.push(metadata);
                }
            }
        }

        (texts_to_embed, metadata_list)
    }

    /// Collect updates in parallel (for large sessions).
    fn collect_updates_parallel(
        &self,
        session: &ActiveSession,
        vectorized: &VectorizedIds,
    ) -> (Vec<String>, Vec<VectorMetadata>) {
        let records: Vec<&Arc<ContextUpdate>> = session.records().iter().collect();
        let results: Vec<(String, VectorMetadata)> = records
            .into_par_iter()
            .filter(|update| !vectorized.contains(&update.id))
            .flat_map_iter(|update| {
                let raw_text = extract_text_from_update(update);
                if crate::content_vectorizer::vector_text::worth_embedding(self, &raw_text) {
                    update_vectors(
                        session.id(),
                        update,
                        &raw_text,
                        self.config().max_text_length,
                    )
                } else {
                    Vec::new()
                }
            })
            .collect();

        results.into_iter().unzip()
    }
}

/// Index entries of `vectors` and the `metadata` they embed, pairwise.
pub fn entries(vectors: Vec<Vec<f32>>, metadata: Vec<VectorMetadata>) -> Vec<IndexEntry> {
    vectors
        .into_iter()
        .zip(metadata)
        .map(|(vector, meta)| IndexEntry { vector, meta })
        .collect()
}
