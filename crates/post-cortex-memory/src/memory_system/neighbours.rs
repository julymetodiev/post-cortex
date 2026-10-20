// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Completing search results from the sessions they come from:
//! - neighbour expansion: each hit that is a conversation update comes back
//!   with the updates right before and after it in its session. An answer
//!   often spans adjacent turns (the question in one, the fact in the next);
//!   on LoCoMo, ±1 neighbours raise all@50 from 0.710 to 0.789.
//! - full text: a long update is embedded (and so returned) as a summary;
//!   hits get the update's whole text back.
//! - validity and trust (ROADMAP Phase 3 items 3, 7, 8): every record says
//!   whether it still holds and how sure its writer was; records a later
//!   record or `forget` ended explicitly, and records written with low
//!   confidence, rank lower but stay in the results as history.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use uuid::Uuid;

use post_cortex_core::core::context_update::ContextUpdate;
use post_cortex_core::graph::temporal::{self, EndReason, Validity};
use post_cortex_core::session::active_session::ActiveSession;

use crate::content_vectorizer::SemanticSearchResult;
use crate::content_vectorizer::ingestion::{determine_content_type, extract_text_from_update};

/// Complete `hits` from their sessions. Each update hit gets the update's
/// full text back when its vector text is a summary, and is followed by up
/// to `radius` updates on each side of it in session order (the order
/// updates were added). A record already returned, as a hit or a
/// neighbour, is not repeated; a neighbour that is also a hit keeps its own
/// scores. Neighbours carry their hit's `combined_score`, a
/// `similarity_score` of 0 and `neighbour_of` set to the hit's `content_id`.
/// Hits that are not updates (entity descriptions) or whose session is not
/// in `sessions` are returned unchanged.
pub(crate) fn complete_results(
    mut hits: Vec<SemanticSearchResult>,
    sessions: &HashMap<Uuid, Arc<ActiveSession>>,
    radius: usize,
    validity: &ValidityCache,
) -> Vec<SemanticSearchResult> {
    weigh_by_validity(&mut hits, sessions, validity);
    let hit_at: HashMap<(Uuid, &str), usize> = hits
        .iter()
        .enumerate()
        .map(|(i, h)| ((h.session_id, h.content_id.as_str()), i))
        .collect();
    // Update id -> position, built once per session that has a hit.
    let mut positions: HashMap<Uuid, HashMap<Uuid, usize>> = HashMap::new();
    let mut seen: HashSet<(Uuid, String)> = HashSet::new();
    let mut out = Vec::with_capacity(hits.len() * (2 * radius + 1));

    for hit in &hits {
        let located = sessions.get(&hit.session_id).and_then(|session| {
            let id = Uuid::parse_str(&hit.content_id).ok()?;
            let index = positions.entry(hit.session_id).or_insert_with(|| {
                session
                    .incremental_updates
                    .iter()
                    .enumerate()
                    .map(|(i, u)| (u.id, i))
                    .collect()
            });
            Some((session, *index.get(&id)?))
        });
        let Some((session, pos)) = located else {
            if seen.insert((hit.session_id, hit.content_id.clone())) {
                out.push(hit.clone());
            }
            continue;
        };

        let updates = &session.incremental_updates;
        let window = pos.saturating_sub(radius)..=(pos + radius).min(updates.len() - 1);
        for update in &updates[window] {
            let id = update.id.to_string();
            if !seen.insert((hit.session_id, id.clone())) {
                continue;
            }
            match hit_at.get(&(hit.session_id, id.as_str())) {
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
    updates: Arc<Vec<Arc<ContextUpdate>>>,
    graph: Arc<post_cortex_core::graph::entity_graph::SimpleEntityGraph>,
    validity: Arc<HashMap<Uuid, Validity>>,
}

impl ValidityCache {
    /// Validity of every record of `session`.
    pub(crate) fn of(&self, session: &ActiveSession) -> Arc<HashMap<Uuid, Validity>> {
        if let Some(hit) = self.entries.get(&session.id())
            && Arc::ptr_eq(&hit.updates, &session.incremental_updates)
            && Arc::ptr_eq(&hit.graph, &session.entity_graph)
        {
            return Arc::clone(&hit.validity);
        }
        let validity = Arc::new(temporal::validity(
            session.incremental_updates.iter().map(|u| &**u),
            &session.entity_graph,
        ));
        self.entries.insert(
            session.id(),
            CachedValidity {
                updates: Arc::clone(&session.incremental_updates),
                graph: Arc::clone(&session.entity_graph),
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

    /// Forget a session (deleted).
    pub(crate) fn remove(&self, session_id: &Uuid) {
        self.entries.remove(session_id);
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
        let Some(session) = sessions.get(&hit.session_id) else {
            continue;
        };
        let Ok(id) = Uuid::parse_str(&hit.content_id) else {
            continue;
        };
        let Some(update) = session.incremental_updates.iter().find(|u| u.id == id) else {
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
        session_id: hit.session_id,
        importance_score: content_type.importance_weight(),
        content_type,
        text_content: extract_text_from_update(update),
        similarity_score: 0.0,
        timestamp: update.timestamp,
        combined_score: hit.combined_score,
        neighbour_of: Some(hit.content_id.clone()),
        author: update.author.clone(),
        derived_from: update.derived_from.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content_vectorizer::ContentType;
    use post_cortex_core::core::context_update::{UpdateContent, UpdateType};

    fn update(title: &str) -> ContextUpdate {
        ContextUpdate {
            meta: Default::default(),
            id: Uuid::new_v4(),
            update_type: UpdateType::QuestionAnswered,
            content: UpdateContent {
                title: title.to_string(),
                description: format!("{title} said something"),
                details: Vec::new(),
                examples: Vec::new(),
                implications: Vec::new(),
            },
            timestamp: chrono::Utc::now(),
            related_code: None,
            parent_update: None,
            user_marked_important: false,
            creates_entities: Vec::new(),
            creates_relationships: Vec::new(),
            references_entities: Vec::new(),
            typed_entities: Vec::new(),
            author: None,
            derived_from: Vec::new(),
        }
    }

    async fn session(n: usize) -> Arc<ActiveSession> {
        let mut s = ActiveSession::new(Uuid::new_v4(), None, None);
        for i in 0..n {
            s.add_incremental_update(update(&format!("turn {i}")))
                .await
                .unwrap();
        }
        Arc::new(s)
    }

    fn hit(s: &ActiveSession, i: usize, score: f32) -> SemanticSearchResult {
        let u = &s.incremental_updates[i];
        SemanticSearchResult {
            validity: None,
            signals: None,
            confidence: None,
            content_id: u.id.to_string(),
            session_id: s.id(),
            content_type: ContentType::UserMessage,
            text_content: u.content.title.clone(),
            similarity_score: score,
            importance_score: 0.8,
            timestamp: u.timestamp,
            combined_score: score,
            neighbour_of: None,
            author: None,
            derived_from: Vec::new(),
        }
    }

    fn titles(rs: &[SemanticSearchResult]) -> Vec<String> {
        rs.iter()
            .map(|r| {
                let t = r
                    .text_content
                    .split_whitespace()
                    .take(2)
                    .collect::<Vec<_>>()
                    .join(" ");
                match &r.neighbour_of {
                    Some(_) => format!("({t})"),
                    None => t,
                }
            })
            .collect()
    }

    #[tokio::test]
    async fn hits_come_with_adjacent_updates_once() {
        let s = session(10).await;
        let sessions = HashMap::from([(s.id(), Arc::clone(&s))]);
        // Hits at positions 5, 0 and 6: 6 is also 5's neighbour, so it keeps
        // its hit scores and is not repeated; 0 has no left neighbour.
        let hits = vec![hit(&s, 5, 0.9), hit(&s, 0, 0.8), hit(&s, 6, 0.7)];
        let out = complete_results(hits, &sessions, 1, &ValidityCache::default());
        assert_eq!(
            titles(&out),
            [
                "(turn 4)", "turn 5", "turn 6", "turn 0", "(turn 1)", "(turn 7)"
            ]
        );
        let n4 = &out[0];
        assert_eq!(n4.neighbour_of.as_deref(), Some(out[1].content_id.as_str()));
        assert_eq!(n4.combined_score, 0.9);
        assert_eq!(n4.similarity_score, 0.0);
        assert_eq!(out[2].similarity_score, 0.7, "hit 6 keeps its own scores");
        assert!(out[0].text_content.contains("turn 4 said something"));
    }

    #[tokio::test]
    async fn long_hits_get_their_full_text_back() {
        let mut s = ActiveSession::new(Uuid::new_v4(), None, None);
        let mut long = update("long");
        long.content.description = "word ".repeat(1000);
        s.add_incremental_update(long).await.unwrap();
        let s = Arc::new(s);
        let sessions = HashMap::from([(s.id(), Arc::clone(&s))]);

        let mut hits = vec![hit(&s, 0, 0.9)];
        hits[0].text_content = "long | word word ...".into(); // the embedded summary
        let hits = complete_results(hits, &sessions, 0, &ValidityCache::default());
        assert_eq!(hits[0].text_content.len(), "long ".len() + 5000);

        // Text already longer than the update's (e.g. graph enrichment) stays.
        let mut enriched = vec![hit(&s, 0, 0.9)];
        enriched[0].text_content = "x".repeat(6000);
        let enriched = complete_results(enriched, &sessions, 0, &ValidityCache::default());
        assert_eq!(enriched[0].text_content.len(), 6000);
    }

    #[tokio::test]
    async fn radius_zero_unknown_sessions_and_entities_pass_through() {
        let s = session(3).await;
        let sessions = HashMap::from([(s.id(), Arc::clone(&s))]);
        let hits = vec![hit(&s, 1, 0.9)];
        assert_eq!(
            complete_results(hits.clone(), &sessions, 0, &ValidityCache::default()).len(),
            1
        );
        assert_eq!(
            complete_results(hits.clone(), &HashMap::new(), 1, &ValidityCache::default()).len(),
            1
        );

        let mut entity = hit(&s, 1, 0.5);
        entity.content_id = "entity:rust".into();
        entity.content_type = ContentType::EntityDescription;
        let out = complete_results(vec![entity], &sessions, 1, &ValidityCache::default());
        assert_eq!(out.len(), 1);
        assert!(out[0].neighbour_of.is_none());
    }

    /// Records a later record superseded, or written with low confidence,
    /// rank lower and say so; the heuristically restated one is only marked.
    #[tokio::test]
    async fn ended_and_uncertain_records_rank_lower() {
        let mut s = ActiveSession::new(Uuid::new_v4(), None, None);
        let old = update("old plan");
        let mut new = update("new plan");
        new.meta.supersedes = vec![old.id];
        let mut unsure = update("maybe later");
        unsure.meta.confidence = Some(0.5);
        for u in [old.clone(), new.clone(), unsure.clone()] {
            s.add_incremental_update(u).await.unwrap();
        }
        let s = Arc::new(s);
        let sessions = HashMap::from([(s.id(), Arc::clone(&s))]);
        let hits = vec![hit(&s, 0, 0.9), hit(&s, 2, 0.85), hit(&s, 1, 0.6)];
        let out = complete_results(hits, &sessions, 0, &ValidityCache::default());
        assert_eq!(titles(&out), ["maybe later", "new plan", "old plan"]);
        let old_hit = &out[2];
        let v = old_hit.validity.as_ref().expect("ended");
        assert_eq!(v.ended_by, Some(new.id));
        assert_eq!(v.reason, Some(EndReason::Superseded));
        assert!((old_hit.combined_score - 0.45).abs() < 1e-6);
        assert_eq!(out[0].confidence, Some(0.5));
        assert!((out[0].combined_score - 0.6375).abs() < 1e-6);
        assert!(out[1].validity.is_none());
    }
}
