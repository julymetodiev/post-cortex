// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The index a view reads: every record of the scope by id, their validity,
//! the records asserting each edge, who ended what, centrality and
//! communities, and lookups over the merged graph. Built once per call
//! ([`Index::of`]); every view of the call reads it.

use crate::graph_views::detail::Provenance;
use crate::graph_views::types::{GraphEdge, GraphNode, RecordView};
use crate::graph_views::{DESCRIPTION_CHARS, GraphScope};
use chrono::{DateTime, Utc};
use post_cortex_core::core::context_update::{ContextUpdate, EntityData, RelationType};
use post_cortex_core::graph::analytics;
use post_cortex_core::graph::entity_graph::SimpleEntityGraph;
use post_cortex_core::graph::temporal::{self, Validity};
use post_cortex_core::session::active_session::ActiveSession;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;
use uuid::Uuid;

/// What a scope's records say, indexed once per call: every view of the
/// call reads the same index.
pub struct Index<'a> {
    /// The scope's sessions.
    pub sessions: &'a [Arc<ActiveSession>],
    /// The scope's graph (one session's, or the members' merged).
    pub graph: &'a SimpleEntityGraph,
    /// Record id -> (its session, the record).
    pub records: HashMap<Uuid, (Uuid, &'a ContextUpdate)>,
    /// When each record held (`graph::temporal`).
    pub validity: HashMap<Uuid, Validity>,
    /// (from, to, relation) -> records asserting it, oldest first.
    pub edge_records: HashMap<(String, String, RelationType), Vec<Uuid>>,
    /// Record id -> the records whose validity it ended, oldest first.
    pub ended: HashMap<Uuid, Vec<Uuid>>,
    /// Session id -> display name (empty when unnamed).
    pub names: HashMap<Uuid, String>,
    /// Centrality-based importance of each entity.
    pub importance: BTreeMap<String, f32>,
    /// Topic community of each entity that shares one.
    pub communities: BTreeMap<String, usize>,
}

pub fn sort_by_importance(nodes: &mut [GraphNode]) {
    nodes.sort_by(|a, b| {
        b.importance
            .total_cmp(&a.importance)
            .then_with(|| a.name.cmp(&b.name))
    });
}

pub fn sort_edges(edges: &mut [GraphEdge]) {
    edges.sort_by(|a, b| {
        (&a.from, &a.to, &a.relation_type).cmp(&(&b.from, &b.to, &b.relation_type))
    });
}

impl<'a> Index<'a> {
    /// The index of `scope`'s graph and records.
    pub fn of(scope: &'a GraphScope) -> Self {
        let graph: &SimpleEntityGraph = &scope.graph;
        let mut records = HashMap::new();
        let mut names = HashMap::new();
        for session in &scope.sessions {
            names.insert(session.id(), session.name().unwrap_or_default());
            for update in session.records().iter() {
                records.insert(update.id, (session.id(), &**update));
            }
        }
        let validity = temporal::validity(records.values().map(|(_, u)| *u), graph);
        let mut ordered: Vec<&ContextUpdate> = records.values().map(|(_, u)| *u).collect();
        ordered.sort_by_key(|u| (u.timestamp, u.id));
        let mut edge_records: HashMap<_, Vec<Uuid>> = HashMap::new();
        for update in ordered {
            for rel in &update.creates_relationships {
                let resolve = |n: &str| graph.resolve(n).unwrap_or(n).to_string();
                let key = (
                    resolve(&rel.from_entity),
                    resolve(&rel.to_entity),
                    rel.relation_type,
                );
                let ids = edge_records.entry(key).or_default();
                if ids.last() != Some(&update.id) {
                    ids.push(update.id);
                }
            }
        }
        let mut ended: HashMap<Uuid, Vec<Uuid>> = HashMap::new();
        for (id, v) in &validity {
            if let Some(by) = v.ended_by {
                ended.entry(by).or_default().push(*id);
            }
        }
        for ids in ended.values_mut() {
            ids.sort_by_key(|id| (records.get(id).map(|(_, u)| u.timestamp), *id));
        }
        Index {
            sessions: &scope.sessions,
            graph,
            records,
            validity,
            edge_records,
            ended,
            names,
            importance: analytics::importance(graph),
            communities: analytics::communities(graph),
        }
    }
}

impl Index<'_> {
    /// The entity named `name` (canonical or alias), falling back to a
    /// case-insensitive match on canonical names.
    pub fn entity(&self, name: &str) -> Option<&EntityData> {
        self.graph
            .resolve(name)
            .or_else(|| {
                // Fall back to a case-insensitive match on exact names.
                let lower = name.to_lowercase();
                self.graph
                    .entities
                    .sorted()
                    .into_iter()
                    .map(|(k, _)| k)
                    .find(|k| k.to_lowercase() == lower)
                    .map(String::as_str)
            })
            .and_then(|n| self.graph.entities.get(n))
    }

    /// `e` as a node of this scope (importance, records, community).
    pub fn node(&self, e: &EntityData) -> GraphNode {
        GraphNode {
            name: e.name.clone(),
            entity_type: e.entity_type.as_str().to_string(),
            aliases: e.aliases.clone(),
            description: e.description.clone(),
            importance: self.importance.get(&e.name).copied().unwrap_or(0.0),
            mention_count: e.mention_count,
            records: self.graph.entity_mentions.get(&e.name).map_or(0, |ids| {
                ids.iter()
                    .filter(|id| self.records.contains_key(id))
                    .count()
            }),
            first_seen: e.first_mentioned,
            last_seen: e.last_mentioned,
            community: self.communities.get(&e.name).copied(),
        }
    }

    /// Validity of an edge asserted by `ids`: from the earliest start, until
    /// the last of them ends (open while any holds).
    pub fn edge_validity(&self, ids: &[Uuid]) -> (Option<DateTime<Utc>>, Option<DateTime<Utc>>) {
        let intervals: Vec<&Validity> = ids.iter().filter_map(|id| self.validity.get(id)).collect();
        let start = intervals.iter().map(|v| v.valid_from).min();
        let end = if intervals.iter().any(|v| v.is_current()) {
            None
        } else {
            intervals.iter().filter_map(|v| v.valid_to).max()
        };
        (start, end)
    }

    /// The edge `from` -`rel`-> `to` with the records asserting it and its
    /// validity.
    pub fn edge(&self, from: &str, to: &str, rel: &RelationType, context: &str) -> GraphEdge {
        let records = self
            .edge_records
            .get(&(from.to_string(), to.to_string(), *rel))
            .cloned()
            .unwrap_or_default();
        let (valid_from, valid_to) = self.edge_validity(&records);
        GraphEdge {
            from: from.to_string(),
            to: to.to_string(),
            relation_type: rel.as_str().to_string(),
            context: context.to_string(),
            weight: rel.weight(),
            valid_from,
            valid_to,
            records,
        }
    }

    /// Every edge in or out of `name` (canonical), sorted.
    pub fn relations_of(&self, name: &str) -> Vec<GraphEdge> {
        let mut edges: Vec<GraphEdge> = self
            .graph
            .relationships_of(name)
            .iter()
            .map(|r| self.edge(r.from, r.to, r.relation_type, r.context))
            .collect();
        sort_edges(&mut edges);
        edges
    }

    /// Record `id` as agents see it (description cut to
    /// [`DESCRIPTION_CHARS`]), or `None` outside the scope.
    pub fn record(&self, id: Uuid) -> Option<RecordView> {
        let (session_id, update) = self.records.get(&id)?;
        let mut description: String = update
            .content
            .description
            .chars()
            .take(DESCRIPTION_CHARS)
            .collect();
        if description.len() < update.content.description.len() {
            description.push('…');
        }
        Some(RecordView {
            id,
            session_id: *session_id,
            interaction_type: update.update_type.as_str().to_string(),
            title: update.content.title.clone(),
            description,
            author: update.author.clone(),
            timestamp: update.timestamp,
            confidence: update.meta.confidence,
            derived_from: update.derived_from.clone(),
            file: update.related_code.as_ref().map(|c| c.file_path.clone()),
            validity: self.validity.get(&id).cloned()?,
        })
    }

    /// Records of the scope that mention `name` (canonical).
    pub fn mentioning(&self, name: &str) -> Vec<RecordView> {
        self.graph
            .entity_mentions
            .get(name)
            .into_iter()
            .flatten()
            .filter_map(|id| self.record(*id))
            .collect()
    }

    /// The distinct authors and sessions of the records `ids` that belong to
    /// the scope, each sorted.
    pub fn provenance<'u>(&self, ids: impl IntoIterator<Item = &'u Uuid>) -> Provenance {
        let mut authors = BTreeSet::new();
        let mut sessions = BTreeSet::new();
        for (session, update) in ids.into_iter().filter_map(|id| self.records.get(id)) {
            sessions.insert(*session);
            if let Some(a) = update.author.as_deref().filter(|a| !a.is_empty()) {
                authors.insert(a.to_string());
            }
        }
        Provenance {
            authors: authors.into_iter().collect(),
            sessions: sessions.into_iter().collect(),
        }
    }
}
