// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Completing search results from the sessions they come from:
//! - neighbour expansion: each hit that is a conversation update comes back
//!   with the updates right before and after it in its session. An answer
//!   often spans adjacent turns (the question in one, the fact in the next);
//!   on LoCoMo, ±1 neighbours raise all@50 from 0.710 to 0.789.
//! - full text: a long update is embedded (and so returned) as a summary;
//!   hits get the update's whole text back.
//! - validity and trust: every record says
//!   whether it still holds and how sure its writer was; records a later
//!   record or `forget` ended explicitly, and records written with low
//!   confidence, rank lower but stay in the results as history.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use uuid::Uuid;

use post_cortex_core::collections::SharedVec;
use post_cortex_core::core::context_update::ContextUpdate;
use post_cortex_core::graph::temporal::{self, EndReason, Validity};
use post_cortex_core::session::active_session::ActiveSession;

use crate::content_vectorizer::vector_text::{determine_content_type, extract_text_from_update};
use crate::content_vectorizer::{HitSource, SemanticSearchResult};

/// Complete `hits` from their sessions. Each update hit gets the update's
/// full text back when its vector text is a summary, and is followed by up
/// to `radius` updates on each side of it in session order (the order
/// updates were added). A record already returned, as a hit or a
/// neighbour, is not repeated; a neighbour that is also a hit keeps its own
/// scores. Neighbours carry their hit's `combined_score`, a
/// `similarity_score` of 0 and `neighbour_of` set to the hit's `content_id`.
/// Hits that are not updates (entity descriptions) or whose session is not
/// in `sessions` are returned unchanged.
pub fn complete_results(
    mut hits: Vec<SemanticSearchResult>,
    sessions: &HashMap<Uuid, Arc<ActiveSession>>,
    radius: usize,
    validity: &ValidityCache,
) -> Vec<SemanticSearchResult> {
    weigh_by_validity(&mut hits, sessions, validity);
    let hit_at: HashMap<(HitSource, &str), usize> = hits
        .iter()
        .enumerate()
        .map(|(i, h)| ((h.source, h.content_id.as_str()), i))
        .collect();
    // Update id -> position, built once per session that has a hit.
    let mut positions: HashMap<Uuid, HashMap<Uuid, usize>> = HashMap::new();
    let mut seen: HashSet<(HitSource, String)> = HashSet::new();
    let mut out = Vec::with_capacity(hits.len() * (2 * radius + 1));

    for hit in &hits {
        let session_of = hit.source.session();
        let located = session_of.and_then(|sid| {
            let session = sessions.get(&sid)?;
            let id = Uuid::parse_str(&hit.content_id).ok()?;
            let index = positions.entry(sid).or_insert_with(|| {
                session
                    .records()
                    .iter()
                    .enumerate()
                    .map(|(i, u)| (u.id, i))
                    .collect()
            });
            Some((session, *index.get(&id)?))
        });
        let Some((session, pos)) = located else {
            if seen.insert((hit.source, hit.content_id.clone())) {
                out.push(hit.clone());
            }
            continue;
        };

        let updates = session.records();
        let first = pos.saturating_sub(radius);
        let last = (pos + radius).min(updates.len() - 1);
        for update in updates.iter_from(first).take(last + 1 - first) {
            let id = update.id.to_string();
            if !seen.insert((hit.source, id.clone())) {
                continue;
            }
            match hit_at.get(&(hit.source, id.as_str())) {
                Some(&i) => out.push(with_full_text(hits[i].clone(), update)),
                None => {
                    let mut n = neighbour(update, hit);
                    n.validity = validity.ended(session, &update.id);
                    n.confidence = update.meta.confidence;
                    out.push(n);
                }
            }
        }
    }
    out
}

/// Score factor of a record a later record or `forget` ended explicitly.
pub(crate) const ENDED_FACTOR: f32 = 0.5;

/// Validity of the records of sessions, computed once per version of a
/// session's records and graph.
#[derive(Default)]
pub(crate) struct ValidityCache {
    entries: dashmap::DashMap<Uuid, CachedValidity>,
}

/// Holding the source `Arc`s keeps their addresses from being reused, so
/// pointer equality means the same records and graph.
struct CachedValidity {
    updates: SharedVec<Arc<ContextUpdate>>,
    graph: Arc<post_cortex_core::graph::entity_graph::SimpleEntityGraph>,
    validity: Arc<HashMap<Uuid, Validity>>,
}

impl ValidityCache {
    /// Validity of every record of `session`.
    pub(crate) fn of(&self, session: &ActiveSession) -> Arc<HashMap<Uuid, Validity>> {
        if let Some(hit) = self.entries.get(&session.id())
            && hit.updates.ptr_eq(session.records())
            && Arc::ptr_eq(&hit.graph, session.entity_graph_shared())
        {
            return Arc::clone(&hit.validity);
        }
        let validity = Arc::new(temporal::validity(
            session.records().iter().map(|u| &**u),
            session.entity_graph(),
        ));
        self.entries.insert(
            session.id(),
            CachedValidity {
                updates: session.records().clone(),
                graph: Arc::clone(session.entity_graph_shared()),
                validity: Arc::clone(&validity),
            },
        );
        validity
    }

    /// Validity of record `id` of `session` if it no longer holds.
    fn ended(&self, session: &ActiveSession, id: &Uuid) -> Option<Validity> {
        self.of(session)
            .get(id)
            .filter(|v| !v.is_current())
            .cloned()
    }

    /// Sessions with a cached validity.
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }

    /// Keep only the sessions `keep` names (the cached ones).
    pub(crate) fn retain(&self, keep: impl Fn(&Uuid) -> bool) {
        self.entries.retain(|id, _| keep(id));
    }
}

/// Mark record hits that no longer hold and their writers' confidence, and
/// lower the scores of explicitly ended and less certain records (by
/// [`ENDED_FACTOR`], and by half of the missing confidence), keeping the
/// hits ordered by score. Records ended by the heuristics (restated,
/// duplicate) are marked only.
fn weigh_by_validity(
    hits: &mut [SemanticSearchResult],
    sessions: &HashMap<Uuid, Arc<ActiveSession>>,
    cache: &ValidityCache,
) {
    let mut changed = false;
    for hit in hits.iter_mut() {
        let Some(session) = hit.source.session().and_then(|sid| sessions.get(&sid)) else {
            continue;
        };
        let Ok(id) = Uuid::parse_str(&hit.content_id) else {
            continue;
        };
        let Some(update) = session.records().iter().find(|u| u.id == id) else {
            continue;
        };
        hit.confidence = update.meta.confidence;
        if let Some(c) = hit.confidence {
            hit.combined_score *= 0.5 + 0.5 * c.clamp(0.0, 1.0);
            changed = true;
        }
        hit.validity = cache.ended(session, &id);
        if hit.validity.as_ref().is_some_and(|v| {
            matches!(
                v.reason,
                Some(EndReason::Superseded | EndReason::Contradicted | EndReason::Forgotten)
            )
        }) {
            hit.combined_score *= ENDED_FACTOR;
            changed = true;
        }
    }
    if changed {
        hits.sort_by(|a, b| b.combined_score.total_cmp(&a.combined_score));
    }
}

/// `hit` with the update's full text when its own text (the vector text, a
/// summary for long updates) is shorter. Longer text, e.g. graph-enriched,
/// is kept.
fn with_full_text(mut hit: SemanticSearchResult, update: &ContextUpdate) -> SemanticSearchResult {
    hit.derived_from = update.derived_from.clone();
    hit.title = Some(update.content.title.clone());
    let full = extract_text_from_update(update);
    if full.len() > hit.text_content.len() {
        hit.text_content = full;
    }
    hit
}

fn neighbour(update: &ContextUpdate, hit: &SemanticSearchResult) -> SemanticSearchResult {
    let content_type = determine_content_type(update);
    SemanticSearchResult {
        validity: None,
        signals: None,
        confidence: None,
        content_id: update.id.to_string(),
        source: hit.source,
        importance_score: content_type.importance_weight(),
        content_type,
        text_content: extract_text_from_update(update),
        similarity_score: 0.0,
        timestamp: update.timestamp,
        combined_score: hit.combined_score,
        neighbour_of: Some(hit.content_id.clone()),
        author: update.author.clone(),
        title: Some(update.content.title.clone()),
        derived_from: update.derived_from.clone(),
        document: None,
        full_chars: None,
    }
}

#[cfg(test)]
mod tests;
