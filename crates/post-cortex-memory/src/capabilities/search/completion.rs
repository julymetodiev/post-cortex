// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Completing search hits from their sessions, message streams and
//! documents: full text, validity, and the neighbouring records and turns.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use post_cortex_core::core::document;
use post_cortex_core::core::message::{self, Message};
use post_cortex_core::session::active_session::ActiveSession;
use tracing::warn;
use uuid::Uuid;

use crate::content_vectorizer::{ContentKind, DocumentHit, HitSource, SemanticSearchResult};
use crate::memory_system::MemoryRuntime;
use crate::memory_system::complete_results;

/// Completes search hits from their sessions and message streams.
#[derive(Clone)]
pub struct Completion {
    rt: Arc<MemoryRuntime>,
}

impl Completion {
    /// Completion over the runtime's sessions and streams.
    pub fn new(rt: Arc<MemoryRuntime>) -> Self {
        Self { rt }
    }

    /// Complete `results` from their sessions (`loaded` holds those already
    /// loaded; the others are loaded here): full text for long record and
    /// message hits and, with `neighbours`, the neighbours of every record
    /// and message hit (`SystemConfig::neighbour_radius`), and each
    /// document hit from its latest version. Sessions that fail to load
    /// keep their results as they are.
    pub async fn complete(
        &self,
        results: Vec<SemanticSearchResult>,
        mut loaded: HashMap<Uuid, Arc<ActiveSession>>,
        neighbours: bool,
    ) -> Vec<SemanticSearchResult> {
        if results.is_empty() {
            return results;
        }
        self.load_sessions(&results, &mut loaded).await;
        let radius = if neighbours {
            self.rt.config.neighbour_radius
        } else {
            0
        };
        let results = complete_results(results, &loaded, radius, &self.rt.session_manager.validity);
        let results = self.complete_message_results(results, radius).await;
        self.complete_document_results(results).await
    }

    /// Complete the document hits of a search from their latest versions:
    /// title, whole text, author, kind and version. A document deleted
    /// since its entry was found is left out; one that cannot be read keeps
    /// its hit as it is (logged).
    pub async fn complete_document_results(
        &self,
        results: Vec<SemanticSearchResult>,
    ) -> Vec<SemanticSearchResult> {
        let mut out = Vec::with_capacity(results.len());
        for mut hit in results {
            let (HitSource::Workspace(workspace_id), Some(id)) =
                (hit.source, document::of_content_id(&hit.content_id))
            else {
                out.push(hit);
                continue;
            };
            match self
                .rt
                .stores
                .documents
                .get_document(workspace_id, id, None)
                .await
            {
                Ok(Some(latest)) => {
                    hit.title = Some(latest.title);
                    hit.text_content = latest.text;
                    hit.author = latest.author;
                    hit.timestamp = latest.updated_at;
                    hit.document = Some(DocumentHit {
                        kind: latest.kind.to_string(),
                        version: latest.version,
                    });
                    out.push(hit);
                }
                Ok(None) => {}
                Err(e) => {
                    warn!("Document {id} of workspace {workspace_id} left incomplete: {e}");
                    out.push(hit);
                }
            }
        }
        out
    }

    /// Load the sessions of `results` that `loaded` does not hold yet. A
    /// session that fails to load is left out (its results stay as they
    /// are) and logged.
    pub async fn load_sessions(
        &self,
        results: &[SemanticSearchResult],
        loaded: &mut HashMap<Uuid, Arc<ActiveSession>>,
    ) {
        let ids: HashSet<Uuid> = results.iter().filter_map(|r| r.source.session()).collect();
        for id in ids {
            if loaded.contains_key(&id) {
                continue;
            }
            match self.rt.get_session(id).await {
                Ok(session) => {
                    loaded.insert(id, session.load_full());
                }
                Err(e) => warn!("Results of session {} left incomplete: {}", id, e),
            }
        }
    }

    /// Complete the message hits of a search: each comes back with its
    /// turn's whole text (a hit may be one chunk of a long turn) and is
    /// followed by up to `radius` turns on each side of it in its stream,
    /// read from storage. Results that are not messages pass
    /// through. A turn already returned is not repeated; one that is also a
    /// hit keeps its own scores. Neighbours carry their hit's
    /// `combined_score`, a `similarity_score` of 0 and `neighbour_of`. If a
    /// stream cannot be read, its hits stay as they are.
    pub async fn complete_message_results(
        &self,
        results: Vec<SemanticSearchResult>,
        radius: usize,
    ) -> Vec<SemanticSearchResult> {
        let seq_of = |r: &SemanticSearchResult| {
            let session = r.source.session()?;
            let seq = (r.content_type == ContentKind::Message)
                .then(|| message::seq_of_content_id(&r.content_id))
                .flatten()?;
            Some((session, seq))
        };
        if !results.iter().any(|r| seq_of(r).is_some()) {
            return results;
        }
        let radius = radius as u64;
        let hits: HashMap<(HitSource, String), usize> = results
            .iter()
            .enumerate()
            .map(|(i, r)| ((r.source, r.content_id.clone()), i))
            .collect();
        let mut seen: HashSet<(HitSource, String)> = HashSet::new();
        let mut out = Vec::with_capacity(results.len());
        for hit in &results {
            let Some((session_id, seq)) = seq_of(hit) else {
                if seen.insert((hit.source, hit.content_id.clone())) {
                    out.push(hit.clone());
                }
                continue;
            };
            let window = seq.saturating_sub(radius)..seq.saturating_add(radius + 1);
            let turns = match self
                .rt
                .stores
                .messages
                .load_messages(session_id, window)
                .await
            {
                Ok(turns) => turns,
                Err(e) => {
                    warn!(
                        "Messages around {} of session {} not loaded: {}",
                        hit.content_id, session_id, e
                    );
                    Vec::new()
                }
            };
            // The hit's own turn unreadable (or already expired): keep the hit.
            if !turns.iter().any(|t| t.seq == seq)
                && seen.insert((hit.source, hit.content_id.clone()))
            {
                out.push(hit.clone());
            }
            for turn in &turns {
                let id = message::content_id(turn.seq);
                if !seen.insert((hit.source, id.clone())) {
                    continue;
                }
                let mut item = match hits.get(&(hit.source, id)) {
                    Some(&i) => results[i].clone(),
                    None => message_neighbour(turn, hit),
                };
                item.text_content = message_text(turn);
                out.push(item);
            }
        }
        out
    }
}

/// A turn as search returns it: who said it, then what.
fn message_text(message: &Message) -> String {
    format!("{}: {}", message.role.as_str(), message.text)
}

fn message_neighbour(message: &Message, hit: &SemanticSearchResult) -> SemanticSearchResult {
    SemanticSearchResult {
        validity: None,
        signals: None,
        confidence: None,
        content_id: message::content_id(message.seq),
        source: HitSource::Session(message.session_id),
        content_type: ContentKind::Message,
        text_content: message_text(message),
        similarity_score: 0.0,
        importance_score: ContentKind::Message.importance_weight(),
        timestamp: message.timestamp,
        combined_score: hit.combined_score,
        neighbour_of: Some(hit.content_id.clone()),
        author: message.author.clone(),
        title: None,
        derived_from: Vec::new(),
        document: None,
        full_chars: None,
    }
}

#[cfg(test)]
mod tests;
