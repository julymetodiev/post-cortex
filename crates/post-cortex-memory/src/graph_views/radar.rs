// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! What needs attention: decisions without rationale, open questions,
//! stale entities and alias candidates.

use crate::graph_views::STALE_AFTER_DAYS;
use crate::graph_views::index::Index;
use crate::graph_views::index::sort_by_importance;
use crate::graph_views::types::{AliasCandidate, GraphNode, RadarView, RecordView};
use chrono::{DateTime, Duration, Utc};
use post_cortex_core::core::context_update::{EntityType, RelationType, UpdateType};
use post_cortex_core::core::dismissal::{RadarDismissal, RadarItemKind, alias_key};
use post_cortex_core::graph::entity_graph::{SimpleEntityGraph, entity_key};
use std::collections::{HashMap, HashSet};
use uuid::Uuid;

/// Entity pairs where one name's key is a prefix of the other's (at least
/// four characters, as "surreal" / "surrealdb"), most shared records first.
fn alias_candidates(graph: &SimpleEntityGraph, limit: usize) -> Vec<AliasCandidate> {
    // Sorted below.
    let mut keyed: Vec<(String, &str)> = graph
        .entities
        .keys_unordered()
        .map(|n| (entity_key(n), n.as_str()))
        .collect();
    keyed.sort();
    let mut out = Vec::new();
    for (i, (short, name)) in keyed.iter().enumerate() {
        if short.chars().count() < 4 {
            continue;
        }
        // Keys sharing the prefix follow `short` in sorted order.
        for (long, other) in keyed[i + 1..]
            .iter()
            .take_while(|(k, _)| k.starts_with(short))
        {
            if long == short {
                continue;
            }
            let mine: HashSet<&Uuid> = graph
                .entity_mentions
                .get(*name)
                .into_iter()
                .flatten()
                .collect();
            let shared = graph
                .entity_mentions
                .get(*other)
                .into_iter()
                .flatten()
                .filter(|id| mine.contains(id))
                .count();
            out.push(AliasCandidate {
                name: name.to_string(),
                of: other.to_string(),
                shared_records: shared,
            });
        }
    }
    out.sort_by(|a, b| {
        b.shared_records
            .cmp(&a.shared_records)
            .then_with(|| a.name.cmp(&b.name))
            .then_with(|| a.of.cmp(&b.of))
    });
    out.truncate(limit);
    out
}

impl Index<'_> {
    /// Unsolved problems, decisions without rationale, open questions,
    /// entities untouched since `now - 90 days`, and possible aliases.
    pub fn radar_view(&self, now: DateTime<Utc>, limit: usize) -> RadarView {
        self.radar_view_with(now, limit, &[], false)
    }

    /// [`Self::radar_view`] with the items `dismissals` name left out
    /// (unless `include_dismissed`); the dismissals that match an item are
    /// listed in [`RadarView::dismissed`]. Lists are cut to `limit` after
    /// the dismissed items are left out.
    pub fn radar_view_with(
        &self,
        now: DateTime<Utc>,
        limit: usize,
        dismissals: &[RadarDismissal],
        include_dismissed: bool,
    ) -> RadarView {
        let index = self;
        let graph = index.graph;
        let dismissed_keys: HashMap<(RadarItemKind, &str), &RadarDismissal> = dismissals
            .iter()
            .map(|d| ((d.kind, d.key.trim()), d))
            .collect();
        let mut matched: Vec<RadarDismissal> = Vec::new();
        // Keep an item unless it is dismissed (and dismissed ones are not
        // wanted); note every dismissal that matched.
        let mut keep = |kind: RadarItemKind, key: &str| -> bool {
            match dismissed_keys.get(&(kind, key.trim())) {
                Some(d) => {
                    if !matched.iter().any(|m| m.kind == d.kind && m.key == d.key) {
                        matched.push((*d).clone());
                    }
                    include_dismissed
                }
                None => true,
            }
        };

        let mut unsolved_problems: Vec<GraphNode> = graph
            .entities
            .sorted()
            .into_iter()
            .map(|(_, e)| e)
            .filter(|e| e.entity_type == EntityType::Problem)
            .filter(|e| {
                !graph
                    .relationships_of(&e.name)
                    .iter()
                    .any(|r| *r.relation_type == RelationType::Solves && r.to == e.name)
            })
            .map(|e| index.node(e))
            .collect();
        unsolved_problems.retain(|n| keep(RadarItemKind::UnsolvedProblem, &n.name));
        sort_by_importance(&mut unsolved_problems);
        unsolved_problems.truncate(limit);

        let mut decisions_without_rationale: Vec<RecordView> = index
            .records
            .values()
            .filter(|(_, u)| {
                u.update_type == UpdateType::DecisionMade
                    && u.content.description.trim().is_empty()
                    && u.content.details.is_empty()
            })
            .filter_map(|(_, u)| index.record(u.id))
            .filter(|r| r.validity.is_current())
            .collect();
        decisions_without_rationale
            .retain(|r| keep(RadarItemKind::DecisionWithoutRationale, &r.id.to_string()));
        decisions_without_rationale.sort_by_key(|r| std::cmp::Reverse(r.timestamp));
        decisions_without_rationale.truncate(limit);

        let states: Vec<_> = self.sessions.iter().map(|s| s.current_state()).collect();
        let mut open_questions: Vec<String> = states
            .iter()
            .flat_map(|state| state.answered_questions.iter())
            .filter(|q| {
                matches!(
                    q.status,
                    post_cortex_core::core::structured_context::QuestionStatus::Open
                )
            })
            .map(|q| q.question.clone())
            .collect();
        open_questions.retain(|q| keep(RadarItemKind::OpenQuestion, q));
        open_questions.truncate(limit);

        let cutoff = now - Duration::days(STALE_AFTER_DAYS);
        let mut stale_entities: Vec<GraphNode> = graph
            .entities
            .sorted()
            .into_iter()
            .map(|(_, e)| e)
            .filter(|e| e.last_mentioned < cutoff)
            .map(|e| index.node(e))
            .collect();
        stale_entities.retain(|n| keep(RadarItemKind::StaleEntity, &n.name));
        sort_by_importance(&mut stale_entities);
        stale_entities.truncate(limit);

        let mut aliases = alias_candidates(graph, usize::MAX);
        aliases.retain(|a| keep(RadarItemKind::AliasCandidate, &alias_key(&a.name, &a.of)));
        aliases.truncate(limit);

        matched.sort_by(|a, b| (a.kind, &a.key).cmp(&(b.kind, &b.key)));
        RadarView {
            unsolved_problems,
            decisions_without_rationale,
            open_questions,
            stale_entities,
            alias_candidates: aliases,
            dismissed: matched,
        }
    }
}
