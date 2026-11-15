// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Walks over the graph. Every walk takes names through
//! [`SimpleEntityGraph::resolve`] and edges through
//! [`SimpleEntityGraph::relationships_of`].

use crate::core::context_update::{EntityData, EntityRelationship, RelationType};
use crate::graph::entity_graph::SimpleEntityGraph;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashSet, VecDeque};

impl SimpleEntityGraph {
    /// Names of the entities directly related to `entity_name` (either
    /// direction), sorted. Empty when the name resolves to no entity.
    pub fn find_related_entities(&self, entity_name: &str) -> Vec<String> {
        let Some(name) = self.resolve(entity_name) else {
            return Vec::new();
        };
        let related: BTreeSet<&str> = self
            .relationships_of(name)
            .into_iter()
            .map(|rel| if rel.from == name { rel.to } else { rel.from })
            .collect();
        related.into_iter().map(str::to_string).collect()
    }

    /// Readable description of an entity: type, counts, description,
    /// related entities and each relation with its context.
    pub fn get_entity_context(&self, entity_name: &str) -> Option<String> {
        let entity = self.entities.get(self.resolve(entity_name)?)?;
        let related = self.find_related_entities(&entity.name);

        let mut context = format!(
            "Entity: {} (Type: {}, Mentions: {}, Importance: {:.2})",
            entity.name,
            entity.entity_type.stored_name(),
            entity.mention_count,
            entity.importance_score
        );
        if let Some(desc) = &entity.description {
            context.push_str(&format!("\nDescription: {}", desc));
        }
        if !related.is_empty() {
            context.push_str(&format!("\nRelated entities: {}", related.join(", ")));
        }
        let relationships = self.get_entity_relationships(&entity.name);
        if !relationships.is_empty() {
            context.push_str("\nRelationships:");
            for (related_entity, relation_type, relation_context) in relationships {
                context.push_str(&format!(
                    "\n  - {} ({}): {}",
                    related_entity,
                    relation_type.stored_name(),
                    relation_context
                ));
            }
        }
        Some(context)
    }

    /// `(other entity, relation type, context)` of every relation of
    /// `entity_name`, outgoing first.
    pub fn get_entity_relationships(
        &self,
        entity_name: &str,
    ) -> Vec<(String, RelationType, String)> {
        let Some(name) = self.resolve(entity_name) else {
            return Vec::new();
        };
        self.relationships_of(name)
            .into_iter()
            .map(|rel| {
                let other = if rel.from == name { rel.to } else { rel.from };
                (
                    other.to_string(),
                    *rel.relation_type,
                    rel.context.to_string(),
                )
            })
            .collect()
    }

    /// Breadth-first trace from `start_entity` up to `max_depth` hops:
    /// one `(from, relation, to)` per entity reached, the relation's stored
    /// name, prefixed `inverse_` when the edge was walked backwards.
    pub fn trace_entity_relationships(
        &self,
        start_entity: &str,
        max_depth: usize,
    ) -> Vec<(String, String, String)> {
        let Some(start) = self.resolve(start_entity) else {
            return Vec::new();
        };
        let mut visited: HashSet<&str> = HashSet::from([start]);
        let mut queue: VecDeque<(&str, usize)> = VecDeque::from([(start, 0)]);
        let mut trace = Vec::new();
        while let Some((name, depth)) = queue.pop_front() {
            if depth >= max_depth {
                continue;
            }
            for rel in self.relationships_of(name) {
                let outgoing = rel.from == name;
                let other = if outgoing { rel.to } else { rel.from };
                if !visited.insert(other) {
                    continue;
                }
                queue.push_back((other, depth + 1));
                let kind = rel.relation_type.stored_name();
                trace.push(if outgoing {
                    (name.to_string(), kind.to_string(), other.to_string())
                } else {
                    (
                        other.to_string(),
                        format!("inverse_{kind}"),
                        name.to_string(),
                    )
                });
            }
        }
        trace
    }

    /// The entities within `max_depth` hops of `center_entity` and the
    /// relations between them (each once); see
    /// [`get_entity_network_bounded`](Self::get_entity_network_bounded).
    pub fn get_entity_network(&self, center_entity: &str, max_depth: usize) -> EntityNetwork {
        self.get_entity_network_bounded(center_entity, max_depth, usize::MAX, usize::MAX)
    }

    /// [`get_entity_network`](Self::get_entity_network) that stops growing
    /// once it holds `max_entities` entities and `max_relationships`
    /// relationships, so the work and the clones are bounded by the limits
    /// instead of the size of the neighbourhood.
    ///
    /// Breadth-first from `center`, so the entities kept are the closest
    /// ones; relationships keep their direction and only connect entities
    /// in the result. O(degree) per entity.
    pub fn get_entity_network_bounded(
        &self,
        center: &str,
        max_depth: usize,
        max_entities: usize,
        max_relationships: usize,
    ) -> EntityNetwork {
        let mut network = EntityNetwork {
            center: center.to_string(),
            entities: BTreeMap::new(),
            relationships: Vec::new(),
        };
        let Some((center_name, center_data)) = self
            .resolve(center)
            .and_then(|name| self.entities.get_key_value(name))
        else {
            return network;
        };
        if max_entities == 0 {
            return network;
        }
        network
            .entities
            .insert(center_name.clone(), center_data.clone());

        let mut visited: HashSet<&str> = HashSet::from([center_name.as_str()]);
        let mut seen_relationships: HashSet<(&str, &str, &RelationType)> = HashSet::new();
        let mut queue: VecDeque<(&str, usize)> = VecDeque::from([(center_name.as_str(), 0)]);

        while let Some((name, depth)) = queue.pop_front() {
            if depth >= max_depth {
                continue;
            }
            let entities_full = network.entities.len() >= max_entities;
            let relationships_full = network.relationships.len() >= max_relationships;
            if entities_full && relationships_full {
                break;
            }

            for rel in self.relationships_of(name) {
                let other = if rel.from == name { rel.to } else { rel.from };
                if network.entities.len() < max_entities
                    && !visited.contains(other)
                    && let Some((other_name, data)) = self.entities.get_key_value(other)
                {
                    visited.insert(other_name.as_str());
                    network.entities.insert(other_name.clone(), data.clone());
                    queue.push_back((other_name.as_str(), depth + 1));
                }

                if network.relationships.len() < max_relationships
                    && network.entities.contains_key(rel.from)
                    && network.entities.contains_key(rel.to)
                    && seen_relationships.insert((rel.from, rel.to, rel.relation_type))
                {
                    network.relationships.push(rel.to_relationship());
                }
            }
        }

        network
    }

    /// Shortest directed path from `from` to `to` (both included), by
    /// number of relations; among paths of one length, the first found
    /// walking each entity's edges by target name.
    pub fn find_shortest_path(&self, from: &str, to: &str) -> Option<Vec<String>> {
        let start = self.resolve(from)?;
        let goal = self.resolve(to)?;
        let mut previous: std::collections::HashMap<&str, &str> = std::collections::HashMap::new();
        let mut queue: VecDeque<&str> = VecDeque::from([start]);
        let mut seen: HashSet<&str> = HashSet::from([start]);
        while let Some(name) = queue.pop_front() {
            if name == goal {
                let mut path = vec![name.to_string()];
                let mut at = name;
                while let Some(&before) = previous.get(at) {
                    path.push(before.to_string());
                    at = before;
                }
                path.reverse();
                return Some(path);
            }
            for rel in self.relationships_of(name) {
                if rel.from == name && seen.insert(rel.to) {
                    previous.insert(rel.to, name);
                    queue.push_back(rel.to);
                }
            }
        }
        None
    }
}

/// Entity network structure - uses BTreeMap for deterministic serialization
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EntityNetwork {
    /// Name of the central entity
    pub center: String,
    /// All entities reachable within the traversal depth
    pub entities: BTreeMap<String, EntityData>,
    /// Relationships connecting the entities in this network
    pub relationships: Vec<EntityRelationship>,
}
