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

use anyhow::Result;
use rayon::prelude::*;
use std::collections::{HashMap, HashSet};
use tracing::debug;
use uuid::Uuid;

use post_cortex_core::core::context_update::ContextUpdate;
use post_cortex_core::core::message::{self, Message};
use post_cortex_core::graph::retrieval_graph;
use post_cortex_core::session::active_session::ActiveSession;
use post_cortex_embeddings::{MESSAGE_CONTENT_TYPE, VectorMetadata};

use super::types::{AUTHOR_KEY, CHUNK_PARENT_KEY, ContentType, MESSAGE_ROLE_KEY, MESSAGE_SEQ_KEY};
use super::vectorizer::{ContentVectorizer, source_content_ids};

/// Threshold for switching to parallel processing.
/// Below this threshold, sequential processing is faster due to reduced overhead.
const PARALLEL_PROCESSING_THRESHOLD: usize = 50;

impl ContentVectorizer {
    /// Vectorize all content in a session.
    ///
    /// # Errors
    /// Returns an error if vectorization fails or the vector database errors out.
    pub async fn vectorize_session(&self, session: &ActiveSession) -> Result<usize> {
        debug!("Vectorizing session: {}", session.id());
        let mut vectorized_count = 0;

        let (update_count, updates_complete) = self.vectorize_context_updates(session).await?;
        vectorized_count += update_count;

        let (entity_count, entities_complete) = if self.config.enable_entity_vectorization {
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

    /// Vectorize only the most recent update (incremental vectorization).
    /// Much more efficient than re-vectorizing the entire session.
    pub async fn vectorize_latest_update(&self, session: &ActiveSession) -> Result<usize> {
        match session.hot_context.back_arc() {
            Some(update) => self.vectorize_update(session, update.id).await,
            None => {
                debug!(
                    "vectorize_latest_update: no updates in hot_context for session {}",
                    session.id()
                );
                Ok(0)
            }
        }
    }

    /// Vectorize one specific update. Background jobs must name the update
    /// they were queued for: by the time a job runs, later writes may have
    /// landed, and vectorizing "the latest" then embeds the newer update
    /// twice and never the one that queued the job.
    pub async fn vectorize_update(
        &self,
        session: &ActiveSession,
        update_id: Uuid,
    ) -> Result<usize> {
        if session.vectorized_update_ids.contains(&update_id) {
            debug!("vectorize_update: {} already vectorized", update_id);
            return Ok(0);
        }
        // Hot context first (recent, no scan of the full history), then the
        // complete update list for updates already evicted from it.
        let Some(update) = session.hot_context.find(|u| u.id == update_id).or_else(|| {
            session
                .incremental_updates
                .iter()
                .rev()
                .find(|u| u.id == update_id)
                .cloned()
        }) else {
            debug!(
                "vectorize_update: update {} not found in session {}",
                update_id,
                session.id()
            );
            return Ok(0);
        };

        let raw_text = extract_text_from_update(&update);
        if !self.should_vectorize_text(&raw_text) {
            debug!(
                "vectorize_update: text too short ({} chars) for session {}",
                raw_text.len(),
                session.id()
            );
            return Ok(0);
        }

        let vectors = update_vectors(
            session.id(),
            &update,
            &raw_text,
            self.config.max_text_length,
        );
        let attempted = vectors.len();
        let (texts, metadata): (Vec<String>, Vec<VectorMetadata>) = vectors.into_iter().unzip();
        let embeddings = self.embedding_engine.encode_batch(texts).await?;
        if embeddings.len() != attempted {
            tracing::warn!("No embedding generated for update {}", update.id);
            return Ok(0);
        }

        let added = self
            .add_and_persist_many(embeddings.into_iter().zip(metadata).collect())
            .await;
        if added.len() == attempted {
            debug!(
                "Vectorized and persisted update {} ({} vectors)",
                update.id, attempted
            );
            session.vectorized_update_ids.insert(update.id);
            self.invalidate_cached(session.id());
        }
        Ok(added.len())
    }

    /// Vectorize turns of a session's message stream (see
    /// [`message_vectors`]); turns too short to embed and turns that already
    /// have vectors are skipped. Returns how many vectors were added.
    ///
    /// # Errors
    /// Returns an error if the texts cannot be embedded.
    pub async fn vectorize_messages(&self, messages: &[Message]) -> Result<usize> {
        let vectors: Vec<(String, VectorMetadata)> = messages
            .iter()
            .filter(|m| self.should_vectorize_text(&m.text))
            .filter(|m| {
                source_content_ids(
                    &self.vector_db,
                    &message::content_id(m.seq),
                    &m.session_id.to_string(),
                )
                .is_empty()
            })
            .flat_map(|m| message_vectors(m, self.config.max_text_length))
            .collect();
        if vectors.is_empty() {
            return Ok(0);
        }
        let attempted = vectors.len();
        let (texts, metadata): (Vec<String>, Vec<VectorMetadata>) = vectors.into_iter().unzip();
        let embeddings = self.embedding_engine.encode_batch(texts).await?;
        if embeddings.len() != attempted {
            anyhow::bail!(
                "{} embeddings generated for {} message texts",
                embeddings.len(),
                attempted
            );
        }
        let added = self
            .add_and_persist_many(embeddings.into_iter().zip(metadata).collect())
            .await;
        let mut sessions: Vec<Uuid> = messages.iter().map(|m| m.session_id).collect();
        sessions.dedup();
        for session_id in sessions {
            self.invalidate_cached(session_id);
        }
        Ok(added.len())
    }

    /// Vectorize context updates from a session. Returns how many were added
    /// and whether every collected update was added.
    pub(super) async fn vectorize_context_updates(
        &self,
        session: &ActiveSession,
    ) -> Result<(usize, bool)> {
        // CRITICAL: vectorized_update_ids is persisted to RocksDB but vector_db is in-memory.
        // After daemon restart, vectorized_update_ids may contain IDs that are no longer in vector_db.
        // We must verify EACH update_id actually exists in vector_db, not just check if session has any.
        // Update ids are UUIDs, so the O(1) content-id index answers that.
        let stale_ids: Vec<Uuid> = session
            .vectorized_update_ids
            .iter()
            .filter(|id| !self.vector_db.has_update_embedding(&id.to_string()))
            .map(|id| *id)
            .collect();

        if !stale_ids.is_empty() {
            debug!(
                "Removing {} stale vectorized_update_ids for session {} (not in vector_db)",
                stale_ids.len(),
                session.id()
            );
            for stale_id in stale_ids {
                session.vectorized_update_ids.remove(&stale_id);
            }
        }

        let total_updates = session.incremental_updates.len();
        let vectorized_ids_count = session.vectorized_update_ids.len();
        let non_vectorized_count = session
            .incremental_updates
            .iter()
            .filter(|u| !session.vectorized_update_ids.contains(&u.id))
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
                self.collect_updates_parallel(session)
            } else {
                debug!(
                    "Using sequential processing for {} updates",
                    non_vectorized_count
                );
                self.collect_updates_sequential(session)
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
        let embeddings = self.embedding_engine.encode_batch(texts_to_embed).await?;

        let added = self
            .add_and_persist_many(embeddings.into_iter().zip(metadata_list).collect())
            .await;
        for content_id in &added {
            if let Ok(update_id) = Uuid::parse_str(content_id) {
                session.vectorized_update_ids.insert(update_id);
            }
        }

        debug!(
            "Added {} context update vectors, marked as vectorized",
            added.len()
        );
        Ok((added.len(), added.len() == attempted))
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
        let entity_count = session.entity_graph.entities.len();

        let (texts_to_embed, metadata_list) = if entity_count >= PARALLEL_PROCESSING_THRESHOLD {
            debug!(
                "Using parallel processing for {} entities (threshold: {})",
                entity_count, PARALLEL_PROCESSING_THRESHOLD
            );
            let results: Vec<(String, VectorMetadata)> = session
                .entity_graph
                .entities
                .par_iter()
                .filter(|(entity_name, _)| wanted(entity_name))
                .filter_map(|(entity_name, entity_data)| {
                    let entity_text = build_entity_description(session, entity_name, entity_data);
                    if self.should_vectorize_text(&entity_text) {
                        let metadata = VectorMetadata::new(
                            format!("entity:{entity_name}"),
                            entity_text.clone(),
                            session.id().to_string(),
                            "EntityDescription".to_string(),
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
                .entity_graph
                .entities
                .iter()
                .filter(|(entity_name, _)| wanted(entity_name))
            {
                let entity_text = build_entity_description(session, entity_name, entity_data);
                if self.should_vectorize_text(&entity_text) {
                    texts_to_embed.push(entity_text.clone());
                    metadata_list.push(
                        VectorMetadata::new(
                            format!("entity:{entity_name}"),
                            entity_text,
                            session.id().to_string(),
                            "EntityDescription".to_string(),
                        )
                        .with_timestamp(entity_data.last_mentioned),
                    );
                }
            }

            (texts_to_embed, metadata_list)
        };

        // Keep only entities that are new or whose description changed.
        let source = session.id().to_string();
        let mut replaced: HashMap<String, Vec<u32>> = HashMap::new();
        let (texts_to_embed, metadata_list): (Vec<String>, Vec<VectorMetadata>) = texts_to_embed
            .into_iter()
            .zip(metadata_list)
            .filter(|(_, metadata)| {
                let existing = source_content_ids(&self.vector_db, &metadata.id, &source);
                let unchanged = existing.len() == 1
                    && self
                        .vector_db
                        .get_metadata(existing[0])
                        .is_some_and(|m| m.text == metadata.text);
                if !unchanged && !existing.is_empty() {
                    replaced.insert(metadata.id.clone(), existing);
                }
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
        let embeddings = self.embedding_engine.encode_batch(texts_to_embed).await?;

        let added = self
            .add_and_persist_many(embeddings.into_iter().zip(metadata_list).collect())
            .await;

        // Drop the superseded copies only once their replacement is stored.
        let superseded: Vec<u32> = added
            .iter()
            .filter_map(|content_id| replaced.remove(content_id))
            .flatten()
            .collect();
        self.vector_db.remove_vectors(&superseded);

        debug!(
            "Added {} entity vectors ({} superseded)",
            added.len(),
            superseded.len()
        );
        Ok((added.len(), added.len() == attempted))
    }

    /// Collect updates sequentially (for small sessions).
    /// Uses incremental_updates to ensure ALL updates are vectorized (including evicted ones).
    fn collect_updates_sequential(
        &self,
        session: &ActiveSession,
    ) -> (Vec<String>, Vec<VectorMetadata>) {
        let mut texts_to_embed = Vec::new();
        let mut metadata_list = Vec::new();

        for update in session.incremental_updates.iter() {
            if session.vectorized_update_ids.contains(&update.id) {
                continue;
            }

            let raw_text = extract_text_from_update(update);
            if self.should_vectorize_text(&raw_text) {
                for (text, metadata) in
                    update_vectors(session.id(), update, &raw_text, self.config.max_text_length)
                {
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
    ) -> (Vec<String>, Vec<VectorMetadata>) {
        let results: Vec<(String, VectorMetadata)> = session
            .incremental_updates
            .par_iter()
            .filter(|update| !session.vectorized_update_ids.contains(&update.id))
            .flat_map_iter(|update| {
                let raw_text = extract_text_from_update(update);
                if self.should_vectorize_text(&raw_text) {
                    update_vectors(session.id(), update, &raw_text, self.config.max_text_length)
                } else {
                    Vec::new()
                }
            })
            .collect();

        results.into_iter().unzip()
    }

    fn should_vectorize_text(&self, text: &str) -> bool {
        text.trim().len() >= self.config.min_text_length
    }
}

/// Build a comprehensive natural-language description of an entity for vectorization.
fn build_entity_description(
    session: &ActiveSession,
    entity_name: &str,
    entity_data: &post_cortex_core::core::context_update::EntityData,
) -> String {
    let mut description_parts = Vec::new();
    description_parts.push(entity_name.to_string());
    description_parts.push(format!("Type: {:?}", entity_data.entity_type));

    let related: Vec<String> = session
        .entity_graph
        .get_entity_relationships(entity_name)
        .into_iter()
        .take(5)
        .map(|(target, rel, _)| format!("{} {:?}", target, rel))
        .collect();
    if !related.is_empty() {
        description_parts.push(format!("Related to: {}", related.join(", ")));
    }

    if entity_data.mention_count > 0 {
        description_parts.push(format!("Mentioned {} times", entity_data.mention_count));
    }

    description_parts.join(". ")
}

/// Characters shared by consecutive chunks of a long update, so a passage cut
/// at a chunk boundary is whole in one of them.
const CHUNK_OVERLAP: usize = 200;

/// The vectors for an update: its text and metadata, one per chunk. Text up
/// to `chunk_size` characters is one vector with the update's id; longer
/// text is split into overlapping chunks of about `chunk_size` characters
/// (the rerankers see about that much), the first keeping the update's id,
/// the others `<id>#<n>` with [`CHUNK_PARENT_KEY`] naming the update. The
/// first also carries the retrieval-graph terms of the whole update and the
/// names of the entities the agent attached to it (see
/// `post_cortex_core::graph::retrieval_graph`).
fn update_vectors(
    session_id: Uuid,
    update: &ContextUpdate,
    raw_text: &str,
    chunk_size: usize,
) -> Vec<(String, VectorMetadata)> {
    let content_type = format!("{:?}", determine_content_type(update));
    let entities = update
        .typed_entities
        .iter()
        .map(|e| e.name.as_str())
        .chain(update.creates_entities.iter().map(String::as_str))
        .chain(update.references_entities.iter().map(String::as_str));
    let terms = retrieval_graph::terms(raw_text);
    chunk_text(raw_text, chunk_size, CHUNK_OVERLAP)
        .into_iter()
        .enumerate()
        .map(|(n, chunk)| {
            let id = match n {
                0 => update.id.to_string(),
                n => format!("{}#{}", update.id, n + 1),
            };
            let mut metadata = VectorMetadata::new(
                id,
                chunk.to_string(),
                session_id.to_string(),
                content_type.clone(),
            )
            .with_timestamp(update.timestamp);
            if let Some(author) = &update.author {
                metadata = metadata.with_metadata(AUTHOR_KEY.to_string(), author.clone());
            }
            if n == 0 {
                metadata = metadata
                    .with_metadata(
                        retrieval_graph::TERMS_KEY.to_string(),
                        retrieval_graph::join_keys(terms.iter().map(String::as_str)),
                    )
                    .with_metadata(
                        retrieval_graph::ENTITIES_KEY.to_string(),
                        retrieval_graph::join_keys(entities.clone()),
                    );
            } else {
                metadata =
                    metadata.with_metadata(CHUNK_PARENT_KEY.to_string(), update.id.to_string());
            }
            (chunk.to_string(), metadata)
        })
        .collect()
}

/// The vectors of one message: its text chunked like an update's (see
/// [`update_vectors`]), content ids `message:<seq>` and `message:<seq>#<n>`,
/// with the role, author and position in the metadata.
fn message_vectors(message: &Message, chunk_size: usize) -> Vec<(String, VectorMetadata)> {
    let parent = message::content_id(message.seq);
    chunk_text(&message.text, chunk_size, CHUNK_OVERLAP)
        .into_iter()
        .enumerate()
        .map(|(n, chunk)| {
            let id = match n {
                0 => parent.clone(),
                n => format!("{}#{}", parent, n + 1),
            };
            let mut metadata = VectorMetadata::new(
                id,
                chunk.to_string(),
                message.session_id.to_string(),
                MESSAGE_CONTENT_TYPE.to_string(),
            )
            .with_timestamp(message.timestamp)
            .with_metadata(
                MESSAGE_ROLE_KEY.to_string(),
                message.role.as_str().to_string(),
            )
            .with_metadata(MESSAGE_SEQ_KEY.to_string(), message.seq.to_string());
            if let Some(author) = &message.author {
                metadata = metadata.with_metadata(AUTHOR_KEY.to_string(), author.clone());
            }
            if n > 0 {
                metadata = metadata.with_metadata(CHUNK_PARENT_KEY.to_string(), parent.clone());
            }
            (chunk.to_string(), metadata)
        })
        .collect()
}

/// `text` as windows of at most `size` characters, consecutive ones sharing
/// about `overlap` characters, cut at whitespace where there is any in the
/// second half of a window. Text up to `size` characters is one window.
fn chunk_text(text: &str, size: usize, overlap: usize) -> Vec<&str> {
    let starts: Vec<usize> = text.char_indices().map(|(i, _)| i).collect();
    let chars = starts.len();
    if chars <= size || size == 0 {
        return vec![text];
    }
    let byte = |c: usize| starts.get(c).copied().unwrap_or(text.len());
    let is_space = |c: usize| text[byte(c)..].starts_with(char::is_whitespace);
    let mut chunks = Vec::new();
    let mut start = 0;
    loop {
        let mut end = (start + size).min(chars);
        if end < chars {
            // Back off to the last whitespace in the second half.
            if let Some(cut) = (start + size / 2..end).rev().find(|&c| is_space(c)) {
                end = cut;
            }
        }
        chunks.push(text[byte(start)..byte(end)].trim());
        if end >= chars {
            break;
        }
        let mut next = end.saturating_sub(overlap).max(start + 1);
        // Start the next window at a word.
        while next < end && !is_space(next) {
            next += 1;
        }
        start = if next < end { next + 1 } else { end };
    }
    chunks
}

/// Extract text content from a context update.
pub(crate) fn extract_text_from_update(update: &ContextUpdate) -> String {
    let mut text_parts = Vec::new();

    // ALWAYS include title (question) for all update types.
    // For QA updates: the question is critical for semantic search — users typically
    // search with questions, not answers, so the question must be embedded.
    let title = update.content.title.clone();
    let description = update.content.description.clone();

    tracing::debug!(
        "extract_text_from_update: title='{}', description='{}'",
        &title[..title.len().min(50)],
        &description[..description.len().min(50)]
    );

    text_parts.push(title);
    text_parts.push(description);
    text_parts.extend(update.content.details.iter().cloned());
    text_parts.extend(update.content.examples.iter().cloned());
    text_parts.extend(update.content.implications.iter().cloned());

    if let Some(code_ref) = &update.related_code {
        text_parts.push(code_ref.code_snippet.clone());
        text_parts.push(code_ref.file_path.clone());
    }

    text_parts.join(" ")
}

/// Determine content type from context update.
pub(crate) const fn determine_content_type(update: &ContextUpdate) -> ContentType {
    use post_cortex_core::core::context_update::UpdateType;
    match &update.update_type {
        UpdateType::QuestionAnswered => ContentType::UserMessage,
        UpdateType::ProblemSolved => ContentType::ProblemSolution,
        UpdateType::CodeChanged => ContentType::CodeSnippet,
        UpdateType::DecisionMade => ContentType::DecisionPoint,
        UpdateType::ConceptDefined | UpdateType::RequirementAdded => ContentType::UpdateContent,
    }
}

#[cfg(test)]
mod tests {
    use super::chunk_text;

    #[test]
    fn short_text_is_one_chunk() {
        assert_eq!(chunk_text("a few words", 1000, 200), ["a few words"]);
    }

    #[test]
    fn long_text_is_cut_at_words_with_overlap() {
        let text: String = (0..400).map(|i| format!("word{i} ")).collect();
        let chunks = chunk_text(&text, 1000, 200);
        assert!(chunks.len() > 1);
        for chunk in &chunks {
            assert!(chunk.chars().count() <= 1000);
            assert!(chunk.starts_with("word") && chunk.ends_with(|c: char| c.is_ascii_digit()));
        }
        // Consecutive chunks overlap, and together they cover the text.
        for pair in chunks.windows(2) {
            let last = pair[0].split(' ').next_back().unwrap();
            assert!(pair[1].contains(last), "no overlap at {last}");
        }
        assert!(chunks.last().unwrap().ends_with("word399"));
    }

    #[test]
    fn multibyte_text_is_cut_on_char_boundaries() {
        let text = "дума ".repeat(500);
        let chunks = chunk_text(&text, 300, 50);
        assert!(chunks.len() > 1);
        assert!(chunks.iter().all(|c| c.chars().count() <= 300));
    }
}
