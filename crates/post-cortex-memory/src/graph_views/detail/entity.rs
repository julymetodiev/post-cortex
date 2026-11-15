// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The entity page, topics, radar and file view in full: the agents' views
//! of this module's parent, computed on the same index, with every record
//! they list in full and every node and edge with its provenance.

use chrono::{DateTime, Utc};
use post_cortex_core::core::dismissal::RadarDismissal;

use crate::graph_views::Index;
use crate::graph_views::detail::types::{
    EntityDetail, FileDetail, ImpactDetail, LineageDetail, LineageStepDetail, NodeDetail,
    RadarDetail, RecordDetail, TopicDetail, TopicsDetail,
};
use crate::graph_views::types::{GraphNode, RecordView};

impl Index<'_> {
    fn full(&self, views: &[RecordView]) -> Vec<RecordDetail> {
        self.record_details(views.iter().map(|r| r.id))
    }

    fn node_details(&self, nodes: Vec<GraphNode>) -> Vec<NodeDetail> {
        nodes.into_iter().map(|n| self.node_detail(n)).collect()
    }

    /// An entity (by name or alias) with its relations, records (newest
    /// first, at most `limit`), decisions, impact `impact_depth` hops out,
    /// causal chain `lineage_depth` hops either way, files, and its rank and
    /// topic among the scope's entities.
    pub fn entity_detail(
        &self,
        name: &str,
        limit: usize,
        impact_depth: usize,
        lineage_depth: usize,
    ) -> Option<EntityDetail> {
        let view = self.entity_view(name, limit)?;
        let canonical = view.entity.name.clone();
        let impact = self.impact_view(&canonical, impact_depth, limit)?;
        let lineage = self.lineage_links(&canonical, lineage_depth)?;

        // Place by importance, ordered as the graph orders nodes.
        let mine = view.entity.importance;
        let rank = 1 + self
            .graph
            .entities
            .keys_unordered()
            .filter(|n| {
                let theirs = self.importance.get(*n).copied().unwrap_or(0.0);
                theirs > mine || (theirs == mine && n.as_str() < canonical.as_str())
            })
            .count();
        let topic_size = view.entity.community.map_or(0, |c| {
            self.communities
                .values()
                .filter(|other| **other == c)
                .count()
        });
        let files = self.file_touches(
            self.graph
                .entity_mentions
                .get(&canonical)
                .into_iter()
                .flatten(),
        );
        Some(EntityDetail {
            rank,
            ranked: self.graph.entities.len(),
            topic_size,
            relations: view
                .relations
                .into_iter()
                .map(|e| self.edge_detail(e))
                .collect(),
            records: self.full(&view.records),
            total_records: view.total_records,
            decisions: self.full(&view.decisions),
            impact: ImpactDetail {
                entity: impact.entity,
                depth: impact_depth,
                dependents: impact.dependents,
                risks: self.full(&impact.risks),
            },
            lineage: LineageDetail {
                entity: lineage.entity,
                chain: lineage.chain,
                steps: lineage
                    .links
                    .into_iter()
                    .filter_map(|link| {
                        Some(LineageStepDetail {
                            record: self.record_detail(link.record)?,
                            relation: link.relation,
                        })
                    })
                    .collect(),
            },
            files,
            entity: self.node_detail(view.entity),
        })
    }

    /// Topic communities with their central entities and key decisions.
    pub fn topics_detail(&self, limit: usize, per_topic: usize) -> TopicsDetail {
        let view = self.topics_view(limit, per_topic);
        TopicsDetail {
            topics: view
                .topics
                .into_iter()
                .map(|t| TopicDetail {
                    id: t.id,
                    size: t.size,
                    key_decisions: self.full(&t.key_decisions),
                    central: self.node_details(t.central),
                    records: t.records,
                    first: t.first,
                    last: t.last,
                })
                .collect(),
            unclustered: view.unclustered,
        }
    }

    /// What needs attention at `now`, without the items `dismissals` name
    /// unless `include_dismissed` (see [`Self::radar_view_with`]).
    pub fn radar_detail(
        &self,
        now: DateTime<Utc>,
        limit: usize,
        dismissals: &[RadarDismissal],
        include_dismissed: bool,
    ) -> RadarDetail {
        let view = self.radar_view_with(now, limit, dismissals, include_dismissed);
        RadarDetail {
            decisions_without_rationale: self.full(&view.decisions_without_rationale),
            unsolved_problems: self.node_details(view.unsolved_problems),
            open_questions: view.open_questions,
            stale_entities: self.node_details(view.stale_entities),
            alias_candidates: view.alias_candidates,
            dismissed: view.dismissed,
        }
    }

    /// What the records say about the source file `path`.
    pub fn file_detail(&self, path: &str, depth: usize, limit: usize) -> FileDetail {
        let view = self.file_view(path, depth, limit);
        FileDetail {
            path: view.path,
            records: self.full(&view.records),
            problems: self.full(&view.problems),
            decisions: self.full(&view.decisions),
            entities: self.node_details(view.entities),
            dependents: view.dependents,
        }
    }
}
