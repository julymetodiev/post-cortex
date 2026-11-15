// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Search results as the app shows them: every hit with its ranking trace
//! and its neighbours grouped under it, every record and turn found in full.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use post_cortex_core::core::message::{self, Message};
use uuid::Uuid;

use crate::content_vectorizer::{ContentKind, SemanticSearchResult, Signals};
use crate::graph_views::Index;
use crate::graph_views::detail::conversation::{DrawnFrom, TurnDetail, turn};
use crate::graph_views::detail::types::RecordDetail;

/// What a search result is, in full.
#[derive(Clone, Debug)]
pub enum Found {
    /// A record of the scope.
    Record(Box<RecordDetail>),
    /// A captured turn.
    Turn(Box<TurnDetail>),
}

/// A record, a turn or an entity description found by search.
#[derive(Clone, Debug)]
pub struct FoundItem {
    /// Entry id, `message:<seq>`, or the id of an entity description.
    pub content_id: String,
    /// Its session.
    pub session_id: Uuid,
    /// What it is.
    pub content_type: ContentKind,
    /// The text that was matched.
    pub text: String,
    /// When it was written.
    pub timestamp: DateTime<Utc>,
    /// Who wrote it, when known.
    pub author: Option<String>,
    /// The record or turn in full; `None` for an entity description, or a
    /// turn no longer stored.
    pub found: Option<Found>,
}

/// One hit with its trace and its neighbours.
#[derive(Clone, Debug)]
pub struct SearchHitDetail {
    /// The hit.
    pub item: FoundItem,
    /// Final score.
    pub score: f32,
    /// Similarity to the query.
    pub similarity: f32,
    /// Importance.
    pub importance: f32,
    /// Its place in each ranking and the scores.
    pub trace: Option<Signals>,
    /// Neighbours before it in its session or stream, in order.
    pub before: Vec<FoundItem>,
    /// Neighbours after it, in order.
    pub after: Vec<FoundItem>,
    /// Title of the record that ended it, if one did.
    pub ended_by_title: Option<String>,
}

/// The turns behind message results, by (session, position).
pub type Turns = HashMap<(Uuid, u64), Message>;

impl Index<'_> {
    /// `results` (in rank order, neighbours after the hit they were found
    /// for) as hits: each neighbour goes under its hit, before it when it
    /// came first in the session's order. The app's search shows records
    /// and turns: a document hit (a module's to show) is left out.
    pub fn search_hits(
        &self,
        results: Vec<SemanticSearchResult>,
        turns: &Turns,
    ) -> Vec<SearchHitDetail> {
        let drawn = self.drawn_from();
        let mut hits: Vec<SearchHitDetail> = Vec::new();
        let mut at: HashMap<String, usize> = HashMap::new();
        let mut waiting: HashMap<String, Vec<FoundItem>> = HashMap::new();
        for r in results {
            let Some(session_id) = r.source.session() else {
                continue;
            };
            let item = self.found_item(&r, session_id, turns, &drawn);
            match &r.neighbour_of {
                Some(of) => match at.get(of) {
                    Some(&i) => hits[i].after.push(item),
                    None => waiting.entry(of.clone()).or_default().push(item),
                },
                None => {
                    let ended_by_title = r
                        .validity
                        .as_ref()
                        .and_then(|v| v.ended_by)
                        .and_then(|id| self.records.get(&id))
                        .map(|(_, u)| u.content.title.clone());
                    at.insert(r.content_id.clone(), hits.len());
                    hits.push(SearchHitDetail {
                        before: waiting.remove(&r.content_id).unwrap_or_default(),
                        item,
                        score: r.combined_score,
                        similarity: r.similarity_score,
                        importance: r.importance_score,
                        trace: r.signals,
                        after: Vec::new(),
                        ended_by_title,
                    });
                }
            }
        }
        hits
    }

    fn found_item(
        &self,
        r: &SemanticSearchResult,
        session_id: Uuid,
        turns: &Turns,
        drawn: &DrawnFrom,
    ) -> FoundItem {
        let found = match message::seq_of_content_id(&r.content_id) {
            Some(seq) => turns
                .get(&(session_id, seq))
                .map(|m| Found::Turn(Box::new(turn(m.clone(), drawn)))),
            // Entity descriptions have ids that are not record ids: they
            // have no record to show.
            None => match Uuid::parse_str(&r.content_id) {
                Ok(id) => self
                    .record_detail(id)
                    .map(|record| Found::Record(Box::new(record))),
                Err(_) => None,
            },
        };
        FoundItem {
            content_id: r.content_id.clone(),
            session_id,
            content_type: r.content_type,
            text: r.text_content.clone(),
            timestamp: r.timestamp,
            author: r.author.clone(),
            found,
        }
    }
}
