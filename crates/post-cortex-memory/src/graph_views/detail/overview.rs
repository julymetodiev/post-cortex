// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The overview of a scope: counts, activity, authors, the most important
//! entities, recent decisions and records, and the radar's counts. The
//! message count and the embedding queue come from storage; the Views
//! capability fills them in.

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Duration, Utc};
use post_cortex_core::core::context_update::UpdateType;
use post_cortex_core::core::dismissal::RadarDismissal;
use post_cortex_core::graph::temporal::Validity;

use crate::graph_views::detail::types::{AuthorCount, Overview, RadarCounts, SessionStat};
use crate::graph_views::{Index, MAX_LIMIT};

impl Index<'_> {
    /// The overview at `now` with the `top` most important entities and the
    /// `recent` newest decisions and records; `messages` and
    /// `embedding_queue` are 0 (see the module docs).
    pub fn overview(
        &self,
        now: DateTime<Utc>,
        top: usize,
        recent: usize,
        dismissals: &[RadarDismissal],
    ) -> Overview {
        let day = now - Duration::days(1);
        let week = now - Duration::days(7);
        let times = || self.records.values().map(|(_, u)| u.timestamp);

        let mut by_author: HashMap<&str, usize> = HashMap::new();
        for (_, u) in self.records.values() {
            if let Some(a) = u.author.as_deref() {
                *by_author.entry(a).or_default() += 1;
            }
        }
        let mut authors: Vec<AuthorCount> = by_author
            .into_iter()
            .map(|(author, records)| AuthorCount {
                author: author.to_string(),
                records,
            })
            .collect();
        authors.sort_by(|a, b| {
            b.records
                .cmp(&a.records)
                .then_with(|| a.author.cmp(&b.author))
        });

        let mut session_stats: Vec<SessionStat> = self
            .sessions
            .iter()
            .map(|s| SessionStat {
                session_id: s.id(),
                name: s.name().unwrap_or_default(),
                records: s.records().len(),
                last_record: s.records().iter().map(|u| u.timestamp).max(),
            })
            .collect();
        session_stats.sort_by(|a, b| {
            b.last_record
                .cmp(&a.last_record)
                .then_with(|| a.session_id.cmp(&b.session_id))
        });
        let active_sessions = session_stats
            .iter()
            .filter(|s| s.last_record.is_some_and(|t| t >= day))
            .count();

        let newest = self.newest_first();
        let current_decision = |id: &uuid::Uuid| {
            self.records
                .get(id)
                .is_some_and(|(_, u)| u.update_type == UpdateType::DecisionMade)
                && self.validity.get(id).is_none_or(Validity::is_current)
        };
        let recent_decisions = self.record_details(
            newest
                .iter()
                .map(|c| c.key)
                .filter(current_decision)
                .take(recent),
        );
        let recent_records = self.record_details(newest.iter().take(recent).map(|c| c.key));

        let radar = self.radar_view_with(now, MAX_LIMIT, dismissals, false);
        let topics: HashSet<usize> = self.communities.values().copied().collect();
        Overview {
            records: self.records.len(),
            records_day: times().filter(|t| *t >= day).count(),
            records_week: times().filter(|t| *t >= week).count(),
            current_records: self.validity.values().filter(|v| v.is_current()).count(),
            entities: self.graph.entities.len(),
            relations: self.graph.edge_count(),
            topics: topics.len(),
            messages: 0,
            sessions: self.sessions.len(),
            active_sessions,
            authors,
            first_activity: times().min(),
            last_activity: times().max(),
            top_entities: self
                .top_nodes(top, None)
                .0
                .into_iter()
                .map(|n| self.node_detail(n))
                .collect(),
            recent_decisions,
            recent_records,
            radar: RadarCounts {
                unsolved_problems: radar.unsolved_problems.len(),
                decisions_without_rationale: radar.decisions_without_rationale.len(),
                open_questions: radar.open_questions.len(),
                stale_entities: radar.stale_entities.len(),
                alias_candidates: radar.alias_candidates.len(),
            },
            embedding_queue: 0,
            session_stats,
        }
    }
}
