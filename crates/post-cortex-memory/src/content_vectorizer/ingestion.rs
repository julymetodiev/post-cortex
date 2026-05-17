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
use tracing::{debug, info};
use uuid::Uuid;

use post_cortex_core::core::context_update::ContextUpdate;
use post_cortex_core::session::active_session::ActiveSession;
use post_cortex_embeddings::VectorMetadata;

use super::types::ContentType;
use super::vectorizer::ContentVectorizer;

/// Threshold for switching to parallel processing.
/// Below this threshold, sequential processing is faster due to reduced overhead.
const PARALLEL_PROCESSING_THRESHOLD: usize = 50;

impl ContentVectorizer {
    /// Vectorize all content in a session.
    ///
    /// # Errors
    /// Returns an error if vectorization fails or the vector database errors out.
    pub async fn vectorize_session(&self, session: &ActiveSession) -> Result<usize> {
        info!("Vectorizing session: {}", session.id());
        let mut vectorized_count = 0;

        let update_count = self.vectorize_context_updates(session).await?;
        vectorized_count += update_count;

        let entity_count = if self.config.enable_entity_vectorization {
            let c = self.vectorize_entities(session).await?;
            vectorized_count += c;
            c
        } else {
            0
        };

        info!(
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
        info!(
            "vectorize_latest_update: session={}, hot_context_len={}",
            session.id(),
            session.hot_context.len()
        );

        let update = match session.hot_context.back() {
            Some(u) => u,
            None => {
                info!(
                    "vectorize_latest_update: no updates in hot_context for session {}",
                    session.id()
                );
                return Ok(0);
            }
        };

        let raw_text = extract_text_from_update(&update);
        if !self.should_vectorize_text(&raw_text) {
            info!(
                "vectorize_latest_update: text too short ({} chars) for session {}",
                raw_text.len(),
                session.id()
            );
            return Ok(0);
        }

        let prepared_text = self.prepare_text_for_vectorization(&raw_text);
        debug!(
            "Prepared text for vectorization: {} chars (original: {} chars)",
            prepared_text.len(),
            raw_text.len()
        );

        let content_type = determine_content_type(&update);
        let embeddings = self
            .embedding_engine
            .encode_batch(vec![prepared_text.clone()])
            .await?;

        let embedding = match embeddings.into_iter().next() {
            Some(e) => e,
            None => {
                tracing::warn!("No embedding generated for latest update");
                return Ok(0);
            }
        };

        let metadata = VectorMetadata::new(
            update.id.to_string(),
            prepared_text,
            session.id().to_string(),
            format!("{content_type:?}"),
        );

        if self.add_and_persist(embedding, metadata).await {
            debug!(
                "Successfully vectorized and persisted latest update {}",
                update.id
            );
            session.vectorized_update_ids.insert(update.id);
            Ok(1)
        } else {
            Ok(0)
        }
    }

    /// Vectorize context updates from a session.
    pub(super) async fn vectorize_context_updates(&self, session: &ActiveSession) -> Result<usize> {
        // CRITICAL: vectorized_update_ids is persisted to RocksDB but vector_db is in-memory.
        // After daemon restart, vectorized_update_ids may contain IDs that are no longer in vector_db.
        // We must verify EACH update_id actually exists in vector_db, not just check if session has any.
        let session_id_str = session.id().to_string();

        let actual_vectorized: std::collections::HashSet<String> = self
            .vector_db
            .get_vectorized_update_ids(&session_id_str)
            .into_iter()
            .collect();

        let stale_ids: Vec<Uuid> = session
            .vectorized_update_ids
            .iter()
            .filter(|id| !actual_vectorized.contains(&id.to_string()))
            .map(|id| *id)
            .collect();

        if !stale_ids.is_empty() {
            info!(
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

        info!(
            "vectorize_context_updates: session={}, total_updates={}, vectorized_ids={}, non_vectorized={}, actual_in_vector_db={}",
            session.id(),
            total_updates,
            vectorized_ids_count,
            non_vectorized_count,
            actual_vectorized.len()
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
            info!(
                "vectorize_context_updates: NO texts to embed for session {} (non_vectorized={}, collected=0 — texts too short or filtered)",
                session.id(),
                non_vectorized_count
            );
            return Ok(0);
        }

        let embeddings = self
            .embedding_engine
            .encode_batch(texts_to_embed.clone())
            .await?;

        let mut added_count = 0;
        for (embedding, metadata) in embeddings.into_iter().zip(metadata_list.iter()) {
            if self.add_and_persist(embedding, metadata.clone()).await {
                added_count += 1;
                if let Ok(update_id) = Uuid::parse_str(&metadata.id) {
                    session.vectorized_update_ids.insert(update_id);
                }
            }
        }

        debug!(
            "Added {} context update vectors, marked as vectorized",
            added_count
        );
        Ok(added_count)
    }

    /// Vectorize entity descriptions.
    pub(super) async fn vectorize_entities(&self, session: &ActiveSession) -> Result<usize> {
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
                .filter_map(|(entity_name, entity_data)| {
                    let entity_text = build_entity_description(session, entity_name, entity_data);
                    if self.should_vectorize_text(&entity_text) {
                        let metadata = VectorMetadata::new(
                            format!("entity:{entity_name}"),
                            entity_text.clone(),
                            session.id().to_string(),
                            "EntityDescription".to_string(),
                        );
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

            for (entity_name, entity_data) in &session.entity_graph.entities {
                let entity_text = build_entity_description(session, entity_name, entity_data);
                if self.should_vectorize_text(&entity_text) {
                    texts_to_embed.push(entity_text.clone());
                    metadata_list.push(VectorMetadata::new(
                        format!("entity:{entity_name}"),
                        entity_text,
                        session.id().to_string(),
                        "EntityDescription".to_string(),
                    ));
                }
            }

            (texts_to_embed, metadata_list)
        };

        if texts_to_embed.is_empty() {
            debug!("No entity texts to vectorize for session {}", session.id());
            return Ok(0);
        }

        let embeddings = self
            .embedding_engine
            .encode_batch(texts_to_embed.clone())
            .await?;

        let mut added_count = 0;
        for (embedding, metadata) in embeddings.into_iter().zip(metadata_list) {
            if self.add_and_persist(embedding, metadata).await {
                added_count += 1;
            }
        }

        debug!("Added {} entity vectors", added_count);
        Ok(added_count)
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
                let content_type = determine_content_type(update);
                let prepared_text = self.prepare_text_for_vectorization(&raw_text);
                texts_to_embed.push(prepared_text.clone());
                metadata_list.push(VectorMetadata::new(
                    update.id.to_string(),
                    prepared_text,
                    session.id().to_string(),
                    format!("{content_type:?}"),
                ));
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
            .filter_map(|update| {
                if session.vectorized_update_ids.contains(&update.id) {
                    return None;
                }

                let raw_text = extract_text_from_update(update);
                if self.should_vectorize_text(&raw_text) {
                    let content_type = determine_content_type(update);
                    let prepared_text = self.prepare_text_for_vectorization(&raw_text);
                    let metadata = VectorMetadata::new(
                        update.id.to_string(),
                        prepared_text.clone(),
                        session.id().to_string(),
                        format!("{content_type:?}"),
                    );
                    Some((prepared_text, metadata))
                } else {
                    None
                }
            })
            .collect();

        results.into_iter().unzip()
    }

    fn should_vectorize_text(&self, text: &str) -> bool {
        text.trim().len() >= self.config.min_text_length
    }

    fn prepare_text_for_vectorization(&self, text: &str) -> String {
        let len = text.trim().len();
        if len <= self.config.max_text_length {
            return text.to_string();
        }

        debug!(
            "Text length {} exceeds max {}, applying smart summarization",
            len, self.config.max_text_length
        );
        extract_key_points(text, self.config.max_text_length)
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

/// Extract text content from a context update.
pub(super) fn extract_text_from_update(update: &ContextUpdate) -> String {
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
pub(super) const fn determine_content_type(update: &ContextUpdate) -> ContentType {
    use post_cortex_core::core::context_update::UpdateType;
    match &update.update_type {
        UpdateType::QuestionAnswered => ContentType::UserMessage,
        UpdateType::ProblemSolved => ContentType::ProblemSolution,
        UpdateType::CodeChanged => ContentType::CodeSnippet,
        UpdateType::DecisionMade => ContentType::DecisionPoint,
        UpdateType::ConceptDefined | UpdateType::RequirementAdded => ContentType::UpdateContent,
    }
}

/// Extract key points from long text using smart heuristics.
///
/// Strategy:
/// 1. Preserve Title and Description (most important context)
/// 2. Extract code snippets (technical details)
/// 3. Extract bullet points and numbered lists (structured info)
/// 4. Include beginning (context) and end (conclusion)
/// 5. Preserve key phrases and technical terms
fn extract_key_points(text: &str, max_length: usize) -> String {
    let mut parts = Vec::new();
    let mut current_length = 0;

    let add_part = |parts: &mut Vec<String>, current_len: &mut usize, part: &str| -> bool {
        let part_len = part.len();
        if *current_len + part_len + 3 <= max_length {
            // +3 for " | "
            parts.push(part.to_string());
            *current_len += part_len + 3;
            true
        } else {
            false
        }
    };

    // 1. Always preserve Title and Description (highest priority)
    for line in text.lines() {
        if (line.starts_with("Title:") || line.starts_with("Description:"))
            && !add_part(&mut parts, &mut current_length, line)
        {
            // Title/Description alone exceeds max_length — truncate it
            let truncated = &line.chars().take(max_length - 20).collect::<String>();
            parts.push(format!("{}...", truncated));
            return parts.join(" | ");
        }
    }

    // 2. Extract code snippets
    let code_blocks: Vec<&str> = text
        .split("Code:")
        .skip(1)
        .filter_map(|segment| segment.split('|').next().map(|s| s.trim()))
        .collect();

    for code in code_blocks.iter().take(2) {
        if !add_part(&mut parts, &mut current_length, &format!("Code: {}", code)) {
            break;
        }
    }

    // 3. Extract bullet points and key technical terms
    let mut key_lines = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('-')
            || trimmed.starts_with('*')
            || trimmed.chars().next().is_some_and(|c| c.is_ascii_digit())
            || trimmed.contains("Performance:")
            || trimmed.contains("Algorithm:")
            || trimmed.contains("O(") // Big-O notation
            || trimmed.contains("speedup")
            || trimmed.contains("optimization")
        {
            key_lines.push(trimmed);
        }
    }

    for line in key_lines.iter().take(5) {
        if !add_part(&mut parts, &mut current_length, line) {
            break;
        }
    }

    // 4. If we still have space, add beginning context
    if current_length < max_length * 3 / 4 {
        let intro_budget = (max_length - current_length).min(300);
        if intro_budget > 50 {
            let intro: String = text.chars().take(intro_budget).collect();
            if let Some(last_space) = intro.rfind(' ') {
                let _ = add_part(
                    &mut parts,
                    &mut current_length,
                    &format!("Context: {}...", &intro[..last_space]),
                );
            }
        }
    }

    // 5. Metadata footer
    if current_length < max_length - 50 {
        let _ = add_part(
            &mut parts,
            &mut current_length,
            &format!("[Summarized from {} chars]", text.len()),
        );
    }

    if parts.is_empty() {
        let truncated: String = text.chars().take(max_length - 20).collect();
        if let Some(last_space) = truncated.rfind(' ') {
            format!("{}... [Truncated]", &truncated[..last_space])
        } else {
            format!("{}... [Truncated]", truncated)
        }
    } else {
        parts.join(" | ")
    }
}
