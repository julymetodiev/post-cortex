// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The session's entity graph: entities, the typed relations between them
//! and the records that mention them, in collections a commit shares with
//! the version before ([`crate::collections`]).
//!
//! Split by concern: this module holds the graph itself (nodes, edges, the
//! one edge walker [`SimpleEntityGraph::relationships_of`]);
//! [`resolution`] maps names to entities (the only name entry point is
//! [`SimpleEntityGraph::resolve`]); [`mentions`] records what records say
//! about entities; [`traversal`] walks the graph; [`ranking`] scores it;
//! [`matching`] finds the graph's entities in a text; `codec` is the
//! serialised form.

mod codec;
mod edges;
mod matching;
mod mentions;
mod ranking;
mod resolution;
mod traversal;

#[cfg(test)]
mod tests;

pub use edges::{EdgeData, RelationRef};
pub use matching::{entities_in, name_keys_in, name_words};
pub use mentions::mentioned_names;
pub use ranking::EntityAnalysis;
pub use resolution::entity_key;
pub use traversal::EntityNetwork;

use crate::collections::{SharedMap, SharedVec};
use crate::core::context_update::{EntityData, EntityRelationship, EntityType, RelationType};
use crate::graph::entity_graph::mentions::MentionSpan;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// The `limit` smallest elements of `items` under `cmp`, sorted.
/// O(n + k log k) instead of a full O(n log n) sort. Not stable: give `cmp`
/// a total tie-break when the order of equal elements matters.
pub(crate) fn top_k_by<T, F>(mut items: Vec<T>, limit: usize, mut cmp: F) -> Vec<T>
where
    F: FnMut(&T, &T) -> std::cmp::Ordering,
{
    if limit == 0 {
        return Vec::new();
    }
    if items.len() > limit {
        items.select_nth_unstable_by(limit - 1, &mut cmp);
        items.truncate(limit);
    }
    items.sort_by(cmp);
    items
}

/// The other end and the kind of an edge, as its endpoint's adjacency
/// keys it.
type EdgeKey = (String, RelationType);

/// The entity graph of a session. Every collection is shared between the
/// versions of a session ([`SharedMap`], [`SharedVec`]): a commit copies
/// what it changes, not the graph.
#[derive(Clone, Debug)]
pub struct SimpleEntityGraph {
    /// Entities by name.
    pub entities: SharedMap<String, EntityData>,

    /// The records that mention each entity, oldest first.
    pub entity_mentions: SharedMap<String, SharedVec<uuid::Uuid>>,

    /// Edges by source: target and relation type -> context.
    outgoing: SharedMap<String, SharedMap<EdgeKey, String>>,

    /// Edges by target: the source and relation type of each.
    incoming: SharedMap<String, SharedMap<EdgeKey, ()>>,

    /// Number of edges.
    edges: usize,

    /// [`entity_key`] of every entity name and alias -> the entity's name.
    /// Rebuilt after deserialization.
    alias_index: SharedMap<String, String>,
}

impl Default for SimpleEntityGraph {
    fn default() -> Self {
        Self::new()
    }
}

impl SimpleEntityGraph {
    /// Creates a new empty entity graph
    pub fn new() -> Self {
        Self {
            entities: SharedMap::new(),
            entity_mentions: SharedMap::new(),
            outgoing: SharedMap::new(),
            incoming: SharedMap::new(),
            edges: 0,
            alias_index: SharedMap::new(),
        }
    }

    /// Clear all entities, relationships, and graph data
    pub fn clear(&mut self) {
        *self = Self::new();
    }

    /// Remove a single entity and all its incident edges. Returns true if
    /// the entity existed and was removed.
    pub fn remove_entity(&mut self, name: &str) -> bool {
        if self.entities.remove(name).is_none() {
            return false;
        }
        if let Some(out) = self.outgoing.remove(name) {
            for ((to, relation_type), _) in out.iter_unordered() {
                self.edges -= 1;
                if to != name
                    && let Some(back) = self.incoming.get_mut(to.as_str())
                {
                    back.remove(&(name.to_string(), *relation_type));
                }
            }
        }
        if let Some(inc) = self.incoming.remove(name) {
            for ((from, relation_type), _) in inc.iter_unordered() {
                if from == name {
                    continue;
                }
                self.edges -= 1;
                if let Some(forward) = self.outgoing.get_mut(from.as_str()) {
                    forward.remove(&(name.to_string(), *relation_type));
                }
            }
        }
        self.entity_mentions.remove(name);
        self.alias_index.retain(|_, canonical| canonical != name);
        true
    }

    /// Check if entity exists in the graph
    pub fn has_entity(&self, name: &str) -> bool {
        self.entities.contains_key(name)
    }

    /// Get the number of entities in the graph
    pub fn entity_count(&self) -> usize {
        self.entities.len()
    }

    /// The entity `name` resolves to, created (one mention at `at`) when it
    /// is unknown.
    fn entity_for(&mut self, name: &str, at: DateTime<Utc>) -> String {
        if let Some(known) = self.resolve(name) {
            return known.to_string();
        }
        self.add_or_update_entity(name.to_string(), EntityType::Concept, at, "");
        self.resolve(name)
            .map_or_else(|| name.to_string(), str::to_string)
    }

    /// Add relationship - idempotent on (from, to, relation_type): the same
    /// triple again updates the context. An endpoint that is not yet an
    /// entity is created as a concept mentioned once at `at` (the time of
    /// the record that states the relation), so replays are deterministic.
    pub fn add_relationship(&mut self, relationship: EntityRelationship, at: DateTime<Utc>) {
        let from = self.entity_for(&relationship.from_entity, at);
        let to = self.entity_for(&relationship.to_entity, at);
        if from == to && relationship.from_entity != relationship.to_entity {
            // Two spellings of one entity: not a relation.
            return;
        }
        self.put_edge(from, to, relationship.relation_type, relationship.context);
    }

    /// Store the edge `from -> to` of `relation_type` with `context`
    /// (replacing the context of the same edge).
    fn put_edge(&mut self, from: String, to: String, relation_type: RelationType, context: String) {
        let key = (to.clone(), relation_type);
        let mut added = false;
        self.outgoing.upsert(from.clone(), SharedMap::new, |out| {
            added = !out.contains_key(&key);
            out.insert(key, context);
        });
        if added {
            self.edges += 1;
            self.incoming.upsert(to, SharedMap::new, |inc| {
                inc.insert((from, relation_type), ())
            });
        }
    }

    /// Get entities by type, by name.
    pub fn get_entities_by_type(&self, entity_type: &EntityType) -> Vec<&EntityData> {
        self.entities
            .sorted()
            .into_iter()
            .map(|(_, e)| e)
            .filter(|entity| entity.entity_type == *entity_type)
            .collect()
    }

    /// Get graph statistics
    pub fn get_graph_stats(&self) -> GraphStats {
        let nodes = self.entities.len();
        GraphStats {
            node_count: nodes,
            edge_count: self.edges,
            entity_count: nodes,
            avg_degree: if nodes > 0 {
                (self.edges * 2) as f64 / nodes as f64
            } else {
                0.0
            },
        }
    }

    /// Every relationship, owned, by (from, to, relation type).
    pub fn get_all_relationships(&self) -> Vec<EntityRelationship> {
        self.relationships().map(|r| r.to_relationship()).collect()
    }

    /// Every entity, owned, by name.
    pub fn get_all_entities(&self) -> Vec<EntityData> {
        self.entities
            .sorted()
            .into_iter()
            .map(|(_, e)| e.clone())
            .collect()
    }

    /// Number of relationship edges (O(1)).
    pub fn edge_count(&self) -> usize {
        self.edges
    }

    /// Every relationship in the graph, borrowed, by (from, to, relation
    /// type).
    pub fn relationships(&self) -> impl Iterator<Item = RelationRef<'_>> {
        let mut all: Vec<RelationRef<'_>> = Vec::with_capacity(self.edges);
        for (from, out) in self.outgoing.iter_unordered() {
            all.extend(
                out.iter_unordered()
                    .map(|((to, relation_type), context)| RelationRef {
                        from,
                        to,
                        relation_type,
                        context,
                    }),
            );
        }
        all.sort_unstable_by(|a, b| {
            (a.from, a.to, a.relation_type).cmp(&(b.from, b.to, b.relation_type))
        });
        all.into_iter()
    }

    /// Append the edges `from -> *` to `found`, by (to, relation type).
    fn push_outgoing<'a>(&'a self, from: &'a str, found: &mut Vec<RelationRef<'a>>) {
        let Some((from, out)) = self.outgoing.get_key_value(from) else {
            return;
        };
        let start = found.len();
        found.extend(
            out.iter_unordered()
                .map(|((to, relation_type), context)| RelationRef {
                    from,
                    to,
                    relation_type,
                    context,
                }),
        );
        found[start..]
            .sort_unstable_by(|a, b| (a.to, a.relation_type).cmp(&(b.to, b.relation_type)));
    }

    /// Append the edges `* -> to` other than self-loops to `found`, by
    /// (from, relation type).
    fn push_incoming<'a>(&'a self, to: &'a str, found: &mut Vec<RelationRef<'a>>) {
        let Some((to, inc)) = self.incoming.get_key_value(to) else {
            return;
        };
        let start = found.len();
        found.extend(
            inc.keys_unordered()
                .filter(|(from, _)| from != to)
                .filter_map(|(from, relation_type)| {
                    let context = self
                        .outgoing
                        .get(from.as_str())?
                        .get(&(to.clone(), *relation_type))?;
                    Some(RelationRef {
                        from,
                        to,
                        relation_type,
                        context,
                    })
                }),
        );
        found[start..]
            .sort_unstable_by(|a, b| (a.from, a.relation_type).cmp(&(b.from, b.relation_type)));
    }

    /// All relationships touching the entity `entity` resolves to (outgoing
    /// first, then incoming, each by the other end and relation type),
    /// borrowed. O(degree log degree). A self-loop is reported once. This
    /// is the one edge walker: every traversal goes through it.
    pub fn relationships_of(&self, entity: &str) -> Vec<RelationRef<'_>> {
        let Some(name) = self.resolve(entity) else {
            return Vec::new();
        };
        let mut found = Vec::new();
        self.push_outgoing(name, &mut found);
        self.push_incoming(name, &mut found);
        found
    }

    /// Relationships directly connecting the entities `a` and `b` resolve to,
    /// in either direction (`a -> b` edges first, then `b -> a`), borrowed.
    /// O(degree(a) + degree(b)).
    pub fn relation_between(&self, a: &str, b: &str) -> Vec<RelationRef<'_>> {
        let (Some(na), Some(nb)) = (self.resolve(a), self.resolve(b)) else {
            return Vec::new();
        };
        let mut found = Vec::new();
        self.push_outgoing(na, &mut found);
        found.retain(|r| r.to == nb);
        if na != nb {
            let mut back = Vec::new();
            self.push_outgoing(nb, &mut back);
            found.extend(back.into_iter().filter(|r| r.to == na));
        }
        found
    }

    /// The one place mentions are counted: `span` is added to the entity
    /// `written` resolves to (count summed, first = min, last = max, a
    /// missing description filled, `written` kept as an alias), or it is
    /// created from `span`. `importance` is the source's score: a new
    /// entity takes it (default 1.0), an existing one keeps the higher.
    fn record_mentions(
        &mut self,
        written: String,
        entity_type: EntityType,
        span: MentionSpan,
        description: &str,
        importance: Option<f32>,
    ) {
        let name = self
            .resolve(&written)
            .map(str::to_string)
            .unwrap_or_else(|| written.clone());
        match self.entities.get_mut(&name) {
            Some(entity) => {
                entity.first_mentioned = entity.first_mentioned.min(span.first);
                entity.last_mentioned = entity.last_mentioned.max(span.last);
                entity.mention_count = entity.mention_count.saturating_add(span.count);
                if let Some(importance) = importance {
                    entity.importance_score = entity.importance_score.max(importance);
                }
                if entity.description.is_none() && !description.is_empty() {
                    entity.description = Some(description.to_string());
                }
                if written != entity.name && !entity.aliases.contains(&written) {
                    entity.aliases.push(written);
                }
            }
            None => {
                self.entities.insert(
                    name.clone(),
                    EntityData {
                        aliases: Vec::new(),
                        name: name.clone(),
                        entity_type,
                        first_mentioned: span.first,
                        last_mentioned: span.last,
                        mention_count: span.count,
                        importance_score: importance.unwrap_or(1.0),
                        description: (!description.is_empty()).then(|| description.to_string()),
                    },
                );
                self.index_names(&name);
            }
        }
    }

    /// Record `alias` as another name of the entity `name` resolves to.
    fn note_alias(&mut self, name: &str, alias: &str) {
        let Some(canonical) = self.resolve(name).map(str::to_string) else {
            return;
        };
        if let Some(entity) = self.entities.get_mut(&canonical)
            && alias != entity.name
            && !entity.aliases.iter().any(|a| a == alias)
        {
            entity.aliases.push(alias.to_string());
        }
        let key = entity_key(alias);
        if !self.alias_index.contains_key(&key) {
            self.alias_index.insert(key, canonical);
        }
    }

    /// Index `canonical` and its aliases; keys already taken by another
    /// entity keep pointing there.
    fn index_names(&mut self, canonical: &str) {
        let Some(entity) = self.entities.get(canonical) else {
            return;
        };
        let keys: Vec<String> = std::iter::once(&entity.name)
            .chain(&entity.aliases)
            .map(|n| entity_key(n))
            .collect();
        for key in keys {
            if !self.alias_index.contains_key(&key) {
                self.alias_index.insert(key, canonical.to_string());
            }
        }
    }

    /// Rebuild the alias index from the entities, merging entities whose
    /// names share a key into the earliest mentioned one (ties: by name).
    fn rebuild_alias_index(&mut self) {
        self.alias_index.clear();
        let mut order: Vec<(DateTime<Utc>, String)> = self
            .entities
            .values_unordered()
            .map(|e| (e.first_mentioned, e.name.clone()))
            .collect();
        order.sort();
        let mut merges = Vec::new();
        for (_, name) in &order {
            let keys: Vec<String> = {
                let entity = &self.entities[name];
                std::iter::once(&entity.name)
                    .chain(&entity.aliases)
                    .map(|n| entity_key(n))
                    .collect()
            };
            for key in keys {
                match self.alias_index.get(&key) {
                    None => self.alias_index.insert(key, name.clone()),
                    Some(held) if held != name => merges.push((name.clone(), held.clone())),
                    Some(_) => {}
                }
            }
        }
        for (alias, into) in merges {
            self.merge_entities(&alias, &into);
        }
    }
}

/// Summary statistics for the entity graph
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GraphStats {
    /// Number of nodes (entities)
    pub node_count: usize,
    /// Number of directed edges
    pub edge_count: usize,
    /// Number of entities in the metadata map
    pub entity_count: usize,
    /// Average degree (edges × 2 / nodes)
    pub avg_degree: f64,
}
