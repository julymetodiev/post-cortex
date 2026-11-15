// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Records in full, and nodes and edges with their provenance, read from
//! the call's index.

use std::collections::{BTreeMap, HashSet};

use chrono::{DateTime, Utc};
use post_cortex_core::core::context_update::UpdateType;
use uuid::Uuid;

use crate::graph_views::Index;
use crate::graph_views::detail::types::{
    EdgeDetail, EntityRef, FileTouch, NodeDetail, PageCursor, RecordDetail, RecordRelation,
};
use crate::graph_views::record_files::files_of;
use crate::graph_views::types::{GraphEdge, GraphNode, RelationRef};

impl Index<'_> {
    /// Record `id` in full, or `None` outside the scope.
    pub fn record_detail(&self, id: Uuid) -> Option<RecordDetail> {
        let (session_id, u) = self.records.get(&id)?;
        let resolve = |n: &str| self.graph.resolve(n).unwrap_or(n).to_string();
        let mut seen = HashSet::new();
        let entities = u
            .typed_entities
            .iter()
            .map(|e| e.name.as_str())
            .chain(u.creates_entities.iter().map(String::as_str))
            .chain(u.references_entities.iter().map(String::as_str))
            .map(resolve)
            .filter(|n| seen.insert(n.clone()))
            .map(|name| EntityRef {
                entity_type: self.graph.entities.get(&name).map(|e| e.entity_type),
                name,
            })
            .collect();
        Some(RecordDetail {
            id,
            session_id: *session_id,
            session_name: self.names.get(session_id).cloned().unwrap_or_default(),
            update_type: u.update_type,
            title: u.content.title.clone(),
            description: u.content.description.clone(),
            details: u.content.details.clone(),
            examples: u.content.examples.clone(),
            implications: u.content.implications.clone(),
            author: u.author.clone(),
            created_at: u.timestamp,
            validity: self.validity.get(&id).cloned()?,
            confidence: u.meta.confidence,
            entities,
            relations: u
                .creates_relationships
                .iter()
                .map(|r| RecordRelation {
                    relation: RelationRef {
                        from: resolve(&r.from_entity),
                        to: resolve(&r.to_entity),
                        relation_type: r.relation_type,
                    },
                    context: r.context.clone(),
                })
                .collect(),
            code: u.related_code.clone(),
            derived_from: u.derived_from.clone(),
            supersedes: u.meta.supersedes.clone(),
            ended: self.ended.get(&id).cloned().unwrap_or_default(),
            important: u.user_marked_important,
        })
    }

    /// The records `ids` in full, in the given order, leaving out those
    /// outside the scope.
    pub fn record_details(&self, ids: impl IntoIterator<Item = Uuid>) -> Vec<RecordDetail> {
        ids.into_iter()
            .filter_map(|id| self.record_detail(id))
            .collect()
    }

    /// `node` with the provenance of the records that mention it.
    pub fn node_detail(&self, node: GraphNode) -> NodeDetail {
        let provenance = self.provenance(
            self.graph
                .entity_mentions
                .get(&node.name)
                .into_iter()
                .flatten(),
        );
        NodeDetail { node, provenance }
    }

    /// `edge` with the provenance of the records asserting it.
    pub fn edge_detail(&self, edge: GraphEdge) -> EdgeDetail {
        let provenance = self.provenance(&edge.records);
        EdgeDetail { edge, provenance }
    }

    /// The files the records `ids` name ([`files_of`]), most recently
    /// touched first (then by path, descending, as files are paged).
    pub fn file_touches<'i>(&self, ids: impl IntoIterator<Item = &'i Uuid>) -> Vec<FileTouch> {
        // path -> (records, latest time, its type)
        let mut files: BTreeMap<String, (usize, DateTime<Utc>, UpdateType)> = BTreeMap::new();
        let mut seen = HashSet::new();
        for id in ids {
            let Some((_, u)) = self.records.get(id) else {
                continue;
            };
            if !seen.insert(*id) {
                continue;
            }
            for path in files_of(u) {
                let entry = files.entry(path).or_insert((0, u.timestamp, u.update_type));
                entry.0 += 1;
                if u.timestamp > entry.1 {
                    entry.1 = u.timestamp;
                    entry.2 = u.update_type;
                }
            }
        }
        let mut out: Vec<FileTouch> = files
            .into_iter()
            .map(|(path, (records, last_at, last_update_type))| FileTouch {
                path,
                records,
                last_update_type,
                last_at,
            })
            .collect();
        out.sort_by(|a, b| (b.last_at, &b.path).cmp(&(a.last_at, &a.path)));
        out
    }

    /// Every record of the scope, newest first (then by id, descending).
    pub fn newest_first(&self) -> Vec<PageCursor<Uuid>> {
        let mut ids: Vec<PageCursor<Uuid>> = self
            .records
            .iter()
            .map(|(id, (_, u))| PageCursor {
                at: u.timestamp,
                key: *id,
            })
            .collect();
        ids.sort_by(|a, b| b.cmp(a));
        ids
    }
}
