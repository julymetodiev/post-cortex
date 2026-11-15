// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Search hits as results: content kind, scores (relevance, importance,
//! recency, preferred author) and traces.

use crate::content_vectorizer::fusion::Traces;
use crate::content_vectorizer::types::{
    AUTHOR_KEY, CHUNK_PARENT_KEY, ContentKind, HitSource, PREFER_AUTHOR_BOOST, SemanticSearchResult,
};
use crate::content_vectorizer::vectorizer::ContentVectorizer;
use chrono::Utc;
use post_cortex_ports::IndexHit;
use post_cortex_ports::error::{Result, corrupt};
use std::collections::HashMap;
use tracing::debug;
use uuid::Uuid;

/// Turn ranked matches into semantic results. Each match comes with its
/// relevance in [0, 1] (cosine similarity, or the fused score), which is
/// what the combined score ranks by; `similarity_score` stays the cosine.
/// Results by `prefer_author` get [`PREFER_AUTHOR_BOOST`] on top.
/// With `traces` (and whether the relevance is a reranker score), each
/// result gets its [`Signals`].
pub fn process_search_results(
    vectorizer: &ContentVectorizer,
    search_results: Vec<(IndexHit, f32)>,
    recency_bias: f32,
    prefer_author: Option<&str>,
    traces: Option<(&Traces, bool)>,
) -> Result<Vec<SemanticSearchResult>> {
    let now = Utc::now();
    // Defense-in-depth: clamp recency_bias to prevent negative or extreme values
    let recency_bias = recency_bias.clamp(0.0, 10.0);

    let score_adjuster: Option<Box<dyn crate::scoring::ScoreAdjuster>> = if recency_bias > 0.0 {
        Some(Box::new(crate::scoring::TemporalDecayAdjuster::new(
            recency_bias,
            now,
        )))
    } else {
        None
    };

    let decay_start = std::time::Instant::now();
    let mut decay_count = 0;

    let mut results = Vec::new();
    for (result, relevance) in search_results {
        let content_type: ContentKind = result
            .meta
            .content_type
            .parse()
            .map_err(|e| corrupt("index entry", &result.meta.id, e))?;
        let owner = Uuid::parse_str(&result.meta.source)
            .map_err(|e| corrupt("index entry", &result.meta.id, e))?;
        let source = match content_type {
            ContentKind::Document => HitSource::Workspace(owner),
            _ => HitSource::Session(owner),
        };

        let importance_weight = content_type.importance_weight();
        let base_score = relevance.mul_add(0.7, importance_weight * 0.3);

        let mut combined_score = if let Some(ref adjuster) = score_adjuster {
            decay_count += 1;
            adjuster.adjust(base_score, &result.meta)
        } else {
            base_score
        };
        let author = result.meta.metadata.get(AUTHOR_KEY).cloned();
        if prefer_author.is_some() && author.as_deref() == prefer_author {
            combined_score += PREFER_AUTHOR_BOOST;
        }

        let signals = traces.map(|(traces, reranked)| {
            let mut signals = traces
                .get(&(result.meta.source.clone(), result.meta.id.clone()))
                .cloned()
                .unwrap_or_default();
            if reranked {
                signals.rerank = Some(relevance);
            }
            signals
        });
        // A chunk of a long update answers for the update.
        let content_id = match result.meta.metadata.get(CHUNK_PARENT_KEY) {
            Some(parent) => parent.clone(),
            None => result.meta.id.clone(),
        };
        results.push(SemanticSearchResult {
            content_id,
            source,
            content_type,
            text_content: result.meta.text.to_string(),
            similarity_score: result.similarity,
            importance_score: importance_weight,
            timestamp: result.meta.timestamp,
            combined_score,
            neighbour_of: None,
            author,
            title: None,
            derived_from: Vec::new(),
            document: None,
            validity: None,
            confidence: None,
            signals,
            full_chars: None,
        });
    }

    if decay_count > 0 {
        let decay_duration_ns = decay_start.elapsed().as_nanos() as u64;
        vectorizer.record_recency_pass(decay_duration_ns, decay_count);
    }

    let original_count = results.len();
    let results = dedup_and_rank(results);
    debug!(
        "Deduplicated {} results to {} unique",
        original_count,
        results.len()
    );

    debug!("Found {} semantic search results", results.len());
    Ok(results)
}

/// One result per (source, content id), the best scored (the newest on a
/// tie), ordered by [`by_rank`]. Content ids repeat across sessions (entity
/// names, message positions), so the source is part of the key.
pub fn dedup_and_rank(results: Vec<SemanticSearchResult>) -> Vec<SemanticSearchResult> {
    let mut best: HashMap<(HitSource, String), SemanticSearchResult> =
        HashMap::with_capacity(results.len());
    for result in results {
        match best.entry((result.source, result.content_id.clone())) {
            std::collections::hash_map::Entry::Occupied(mut entry) => {
                if by_rank(&result, entry.get()).is_lt() {
                    entry.insert(result);
                }
            }
            std::collections::hash_map::Entry::Vacant(entry) => {
                entry.insert(result);
            }
        }
    }
    let mut results: Vec<SemanticSearchResult> = best.into_values().collect();
    results.sort_by(by_rank);
    results
}

/// The total order of results: higher combined score first, then the
/// newer, then by source and content id (so equal scores always come back
/// in one order).
pub fn by_rank(a: &SemanticSearchResult, b: &SemanticSearchResult) -> std::cmp::Ordering {
    b.combined_score
        .total_cmp(&a.combined_score)
        .then_with(|| b.timestamp.cmp(&a.timestamp))
        .then_with(|| a.source.cmp(&b.source))
        .then_with(|| a.content_id.cmp(&b.content_id))
}

#[cfg(test)]
mod tests;
