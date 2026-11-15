// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The graph as nodes and edges (optionally as it stood at a time) and the
//! timeline of records and relations.

use std::collections::HashSet;

use chrono::{DateTime, Utc};
use post_cortex_core::graph::temporal::EndReason;
use uuid::Uuid;

use crate::graph_views::Index;
use crate::graph_views::detail::types::{
    EdgeDetail, EventKind, GraphDetail, NodeDetail, Timeline, TimelineEvent,
};
use crate::graph_views::index::{sort_by_importance, sort_edges};
use crate::graph_views::types::{GraphNode, RelationRef};

impl Index<'_> {
    /// The `limit` most important entities and the relations between them;
    /// with `at`, as the graph stood then (entities first seen by then,
    /// relations asserted by then). Provenance is computed only for what is
    /// returned.
    pub fn graph_detail(&self, limit: usize, at: Option<DateTime<Utc>>) -> GraphDetail {
        let (nodes, total_nodes) = self.top_nodes(limit, at);
        let shown: HashSet<&str> = nodes.iter().map(|n| n.name.as_str()).collect();

        let mut total_edges = 0;
        let mut edges = Vec::new();
        for rel in self.graph.relationships() {
            let edge = self.edge(rel.from, rel.to, rel.relation_type, rel.context);
            if at.is_some_and(|t| edge.valid_from.is_some_and(|from| from > t)) {
                continue;
            }
            total_edges += 1;
            if shown.contains(rel.from) && shown.contains(rel.to) {
                edges.push(edge);
            }
        }
        sort_edges(&mut edges);
        let nodes: Vec<NodeDetail> = nodes.into_iter().map(|n| self.node_detail(n)).collect();
        let edges: Vec<EdgeDetail> = edges.into_iter().map(|e| self.edge_detail(e)).collect();
        GraphDetail {
            nodes,
            edges,
            total_nodes,
            total_edges,
            at,
        }
    }

    /// The `limit` most important entities (as of `at`), most important
    /// first, and how many there are.
    pub fn top_nodes(&self, limit: usize, at: Option<DateTime<Utc>>) -> (Vec<GraphNode>, usize) {
        let mut nodes: Vec<GraphNode> = self
            .graph
            .entities
            .values_unordered()
            .filter(|e| at.is_none_or(|t| e.first_mentioned <= t))
            .map(|e| self.node(e))
            .collect();
        let total = nodes.len();
        sort_by_importance(&mut nodes);
        nodes.truncate(limit);
        (nodes, total)
    }

    /// Records written and ended, relations added and ended, between `from`
    /// and `to` (inclusive, open when unset), in time order; the latest
    /// `limit`. Events at one instant: endings first, then by record.
    pub fn timeline(
        &self,
        from: Option<DateTime<Utc>>,
        to: Option<DateTime<Utc>>,
        limit: usize,
    ) -> Timeline {
        let within = |at: DateTime<Utc>| from.is_none_or(|f| at >= f) && to.is_none_or(|t| at <= t);
        // Only the window's events are built in full.
        let mut found: Vec<(DateTime<Utc>, EventKind, Uuid, Option<RelationRef>)> = Vec::new();
        for (id, (_, update)) in &self.records {
            if within(update.timestamp) {
                found.push((update.timestamp, EventKind::Recorded, *id, None));
            }
            if let Some(end) = self.validity.get(id).and_then(|v| v.valid_to)
                && within(end)
            {
                found.push((end, EventKind::Ended, *id, None));
            }
        }
        for ((from_e, to_e, rel), ids) in &self.edge_records {
            let (Some(first), Some(last)) = (ids.first(), ids.last()) else {
                continue;
            };
            let relation = RelationRef {
                from: from_e.clone(),
                to: to_e.clone(),
                relation_type: *rel,
            };
            let (start, end) = self.edge_validity(ids);
            if let Some(start) = start.filter(|s| within(*s)) {
                found.push((
                    start,
                    EventKind::RelationAdded,
                    *first,
                    Some(relation.clone()),
                ));
            }
            if let Some(end) = end.filter(|e| within(*e)) {
                found.push((end, EventKind::RelationEnded, *last, Some(relation)));
            }
        }
        found.sort_by_key(|e| (e.0, e.1, e.2));
        let total = found.len();
        if found.len() > limit {
            found.drain(..found.len() - limit);
        }
        let events = found
            .into_iter()
            .filter_map(|(at, kind, record, relation)| {
                let (session_id, update) = self.records.get(&record)?;
                let reason: Option<EndReason> = match kind {
                    EventKind::Ended | EventKind::RelationEnded => {
                        self.validity.get(&record).and_then(|v| v.reason)
                    }
                    EventKind::Recorded | EventKind::RelationAdded => None,
                };
                Some(TimelineEvent {
                    at,
                    kind,
                    record,
                    session_id: *session_id,
                    title: update.content.title.clone(),
                    update_type: update.update_type,
                    relation,
                    reason,
                })
            })
            .collect();
        Timeline { events, total }
    }
}
