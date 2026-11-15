// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Fusing the vector and BM25 rankings (reciprocal rank fusion), reranking
//! the candidate pool, and tracing each result's ranks.

use crate::content_vectorizer::types::Signals;
use post_cortex_ports::{IndexHit, Reranker};
use std::collections::HashMap;
use std::sync::Arc;
use tracing::warn;

/// Candidates taken from each ranking (vector and BM25) before fusion. Short
/// lists fused best in the retrieval prototype (LoCoMo all@50: 0.709 fusing
/// the top 50 of each, 0.691 the top 100, 0.662 the top 200).
pub const FUSION_DEPTH: usize = 50;

/// Candidates the reranker scores, taken from each ranking and from their
/// fusion. A cross-encoder over a deeper pool keeps gaining: LoCoMo all@50
/// with ±1 neighbours 0.878 reranking the top 300, 0.854 the top 200, 0.823
/// the top 100.
pub(crate) const RERANK_POOL: usize = 300;

/// Reciprocal rank fusion constant.
pub const RRF_K: f32 = 60.0;

/// Candidates taken from each ranking: the reranker's pool when the search
/// is reranked, else the depth that fuses best on its own.
pub fn pool_depth(reranking: bool) -> usize {
    if reranking { RERANK_POOL } else { FUSION_DEPTH }
}

/// How many vector candidates to fetch: deeper when they will be fused with
/// BM25 hits or reranked.
pub fn search_depth(limit: usize, lexical: Option<&Vec<IndexHit>>, reranking: bool) -> usize {
    match lexical {
        _ if reranking => limit.max(RERANK_POOL),
        Some(hits) if !hits.is_empty() => limit.max(FUSION_DEPTH),
        _ => limit,
    }
}

/// [`Signals`] per (session, content id) of every candidate.
pub type Traces = HashMap<(String, String), Signals>;

/// Where each candidate ranked in the vector, BM25 and graph rankings.
pub fn traces(
    vector: &[IndexHit],
    lexical: Option<&Vec<IndexHit>>,
    graph: Option<&Vec<IndexHit>>,
) -> Traces {
    let mut out: Traces = HashMap::new();
    let key = |m: &IndexHit| (m.meta.source.clone(), m.meta.id.clone());
    for (i, m) in vector.iter().enumerate() {
        let s = out.entry(key(m)).or_default();
        if s.vector_rank.is_none() {
            s.vector_rank = Some(i + 1);
            s.vector_similarity = Some(m.similarity);
        }
    }
    for (i, m) in lexical.into_iter().flatten().enumerate() {
        out.entry(key(m))
            .or_default()
            .bm25_rank
            .get_or_insert(i + 1);
    }
    for (i, m) in graph.into_iter().flatten().enumerate() {
        out.entry(key(m))
            .or_default()
            .graph_rank
            .get_or_insert(i + 1);
    }
    out
}

/// Record the fused score of each candidate in `traces` (when the ranking
/// was a fusion, i.e. some candidate is not ranked by similarity alone).
pub fn note_fused(
    ranked: Vec<(IndexHit, f32)>,
    traces: Option<&mut Traces>,
) -> Vec<(IndexHit, f32)> {
    if let Some(traces) = traces {
        let fused = traces
            .values()
            .any(|s| s.bm25_rank.is_some() || s.graph_rank.is_some());
        if fused {
            for (m, score) in &ranked {
                if let Some(s) = traces.get_mut(&(m.meta.source.clone(), m.meta.id.clone())) {
                    s.fused = Some(*score);
                }
            }
        }
    }
    ranked
}

/// Reorder the best [`RERANK_POOL`] candidates by the reranker's score;
/// the rest are dropped. The reranker's relevance replaces the fused score. If
/// scoring fails the fused order stands.
pub async fn rerank(
    reranker: Option<&Arc<dyn Reranker>>,
    query: &str,
    mut ranked: Vec<(IndexHit, f32)>,
) -> Vec<(IndexHit, f32)> {
    let Some(reranker) = reranker else {
        return ranked;
    };
    ranked.truncate(RERANK_POOL);
    let documents: Vec<Arc<str>> = ranked
        .iter()
        .map(|(m, _)| Arc::clone(&m.meta.text))
        .collect();
    let scores = match reranker.score(query, &documents).await {
        Ok(scores) if scores.len() == ranked.len() => scores,
        Ok(scores) => {
            warn!(
                "Reranker returned {} scores for {} candidates, keeping the fused order",
                scores.len(),
                ranked.len()
            );
            return ranked;
        }
        Err(e) => {
            warn!("Reranking failed, keeping the fused order: {:#}", e);
            return ranked;
        }
    };
    let mut reranked: Vec<(IndexHit, f32)> = ranked
        .into_iter()
        .zip(scores)
        .map(|((m, _), relevance)| (m, relevance))
        .collect();
    reranked.sort_by(|a, b| b.1.total_cmp(&a.1));
    reranked
}

/// Pair each match with the relevance it ranks by: the cosine similarity for
/// vector search alone, the fused score when other rankings (BM25, the
/// retrieval graph) have candidates.
pub fn rank<const N: usize>(
    vector: Vec<IndexHit>,
    others: [Option<Vec<IndexHit>>; N],
) -> Vec<(IndexHit, f32)> {
    let others: Vec<Vec<IndexHit>> = others
        .into_iter()
        .flatten()
        .filter(|list| !list.is_empty())
        .collect();
    if others.is_empty() {
        return vector
            .into_iter()
            .map(|m| {
                let similarity = m.similarity;
                (m, similarity)
            })
            .collect();
    }
    fuse(std::iter::once(vector).chain(others).collect())
}

/// Reciprocal rank fusion of the rankings, best first. Records are matched
/// by (session, content id); the copy from the earliest list is kept (the
/// vector one first). Adjacent records with the same text share a rank.
/// Scores are normalised so that ranking first in every list gives 1.0.
pub fn fuse(lists: Vec<Vec<IndexHit>>) -> Vec<(IndexHit, f32)> {
    let best = lists.len() as f32 / (RRF_K + 1.0);
    let mut fused: HashMap<(String, String), (IndexHit, f32)> = HashMap::new();
    for list in lists {
        let mut position = 0usize;
        let mut rank = 0usize;
        let mut previous_text: Option<Arc<str>> = None;
        let mut seen = std::collections::HashSet::new();
        for m in list {
            let key = (m.meta.source.clone(), m.meta.id.clone());
            if !seen.insert(key.clone()) {
                continue;
            }
            // Records with the same text score the same in every ranking;
            // they share a rank so the tie survives fusion.
            if previous_text.as_deref() != Some(&*m.meta.text) {
                rank = position;
                previous_text = Some(Arc::clone(&m.meta.text));
            }
            position += 1;
            let score = 1.0 / (RRF_K + rank as f32 + 1.0);
            fused
                .entry(key)
                .and_modify(|(_, s)| *s += score)
                .or_insert((m, score));
        }
    }
    let mut out: Vec<(IndexHit, f32)> = fused.into_values().map(|(m, s)| (m, s / best)).collect();
    // Equal scores come back in one order, whatever the map's.
    out.sort_by(|a, b| {
        b.1.total_cmp(&a.1)
            .then_with(|| a.0.meta.source.cmp(&b.0.meta.source))
            .then_with(|| a.0.meta.id.cmp(&b.0.meta.id))
    });
    out
}
