// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! What records say about entities: mentions are counted in one place
//! ([`SimpleEntityGraph::add_or_update_entity`] and the merges all go
//! through `record_mentions`), and each entity's record links are derived
//! from the records.

use crate::collections::SharedVec;
use crate::core::context_update::{ContextUpdate, EntityData, EntityRelationship, EntityType};
use crate::graph::entity_graph::SimpleEntityGraph;
use chrono::{DateTime, Utc};
use std::collections::HashSet;

/// Names an update mentions: its typed, created and referenced entities and
/// the endpoints of its relations (may repeat).
pub fn mentioned_names(update: &ContextUpdate) -> impl Iterator<Item = &str> {
    update
        .typed_entities
        .iter()
        .map(|e| e.name.as_str())
        .chain(update.creates_entities.iter().map(String::as_str))
        .chain(update.references_entities.iter().map(String::as_str))
        .chain(
            update
                .creates_relationships
                .iter()
                .flat_map(|r| [r.from_entity.as_str(), r.to_entity.as_str()]),
        )
}

/// Mentions of one entity from one source: how many, and when the first
/// and the last were.
#[derive(Clone, Copy, Debug)]
pub struct MentionSpan {
    pub first: DateTime<Utc>,
    pub last: DateTime<Utc>,
    pub count: u32,
}

impl MentionSpan {
    pub fn once(at: DateTime<Utc>) -> Self {
        Self {
            first: at,
            last: at,
            count: 1,
        }
    }

    pub fn of(entity: &EntityData) -> Self {
        Self {
            first: entity.first_mentioned,
            last: entity.last_mentioned,
            count: entity.mention_count,
        }
    }
}

impl SimpleEntityGraph {
    /// Add what `update` says to the graph: its entities (typed ones as
    /// given, else the untyped names as concepts), its relations, and the
    /// links from them to the record.
    pub fn apply_update(&mut self, update: &ContextUpdate) {
        if update.typed_entities.is_empty() {
            for name in &update.creates_entities {
                self.add_or_update_entity(
                    name.clone(),
                    EntityType::Concept,
                    update.timestamp,
                    &format!("From update: {}", update.content.title),
                );
            }
        } else {
            for typed in &update.typed_entities {
                self.add_or_update_entity(
                    typed.name.clone(),
                    typed.entity_type,
                    update.timestamp,
                    &format!("Provided by caller: {}", update.content.title),
                );
            }
        }
        for rel in &update.creates_relationships {
            self.add_relationship(rel.clone(), update.timestamp);
        }
        self.link_mentions(update);
    }

    /// What applying `update` changed in this graph, as it is now: the
    /// entities the update names (typed and created ones and the endpoints
    /// of its relations, resolved) and the edges it states. Nothing else
    /// changes ([`Self::apply_update`] touches no other entity or edge, and
    /// record links are derived at load), so a save of the update writes
    /// these rows only.
    pub fn changed_by(
        &self,
        update: &ContextUpdate,
    ) -> (Vec<&EntityData>, Vec<EntityRelationship>) {
        let named = update
            .typed_entities
            .iter()
            .map(|e| e.name.as_str())
            .chain(update.creates_entities.iter().map(String::as_str))
            .chain(
                update
                    .creates_relationships
                    .iter()
                    .flat_map(|r| [r.from_entity.as_str(), r.to_entity.as_str()]),
            );
        let mut seen = HashSet::new();
        let entities = named
            .filter_map(|name| self.resolve(name))
            .filter(|name| seen.insert(*name))
            .filter_map(|name| self.entities.get(name))
            .collect();
        let edges = update
            .creates_relationships
            .iter()
            .filter_map(|rel| {
                let from = self.resolve(&rel.from_entity)?;
                let to = self.resolve(&rel.to_entity)?;
                self.relationships_of(from)
                    .into_iter()
                    .find(|e| e.from == from && e.to == to && *e.relation_type == rel.relation_type)
                    .map(|e| e.to_relationship())
            })
            .collect();
        (entities, edges)
    }

    /// Link `update` to every known entity it mentions (see
    /// [`mentioned_names`]), once per entity. Counts are not touched: the
    /// mention itself was counted when the entity was added.
    fn link_mentions(&mut self, update: &ContextUpdate) {
        let mut linked: HashSet<String> = HashSet::new();
        for name in mentioned_names(update) {
            let Some(canonical) = self.resolve(name).map(str::to_string) else {
                continue;
            };
            if linked.insert(canonical.clone()) {
                self.entity_mentions
                    .upsert(canonical, SharedVec::new, |ids| {
                        if !ids.iter().any(|id| *id == update.id) {
                            ids.push(update.id);
                        }
                    });
            }
        }
    }

    /// Rebuild every entity's mention list from `updates` (oldest first).
    /// Mentions are derived from the records, so storage need not keep
    /// them: both backends call this on load.
    pub fn relink_mentions<'a>(&mut self, updates: impl IntoIterator<Item = &'a ContextUpdate>) {
        self.entity_mentions.clear();
        for update in updates {
            for name in mentioned_names(update) {
                let Some(canonical) = self.resolve(name).map(str::to_string) else {
                    continue;
                };
                self.entity_mentions
                    .upsert(canonical, SharedVec::new, |ids| {
                        if ids.last() != Some(&update.id) && !ids.iter().any(|id| *id == update.id)
                        {
                            ids.push(update.id);
                        }
                    });
            }
        }
    }

    /// Record a mention of an entity: create it (mention count 1) or bump
    /// its count and widen its first/last-mention times.
    pub fn add_or_update_entity(
        &mut self,
        name: String,
        entity_type: EntityType,
        timestamp: DateTime<Utc>,
        description: &str,
    ) {
        self.record_mentions(
            name,
            entity_type,
            MentionSpan::once(timestamp),
            description,
            None,
        );
    }

    /// The latest mention of any entity, if there is one.
    fn latest_mention(&self) -> Option<DateTime<Utc>> {
        self.entities
            .values_unordered()
            .map(|e| e.last_mentioned)
            .max()
    }

    /// Merge all entities and relationships from another graph into this one.
    ///
    /// Each entity's mentions are added as they are (counts summed, first
    /// and last mention widened, the higher importance kept), so merging
    /// the graphs of several sessions counts what they count together.
    /// Edges are copied as-is.
    pub fn merge_from(&mut self, other: &SimpleEntityGraph) {
        // Merge entities first so all nodes exist before we add edges
        for (name, data) in other.entities.sorted() {
            self.record_mentions(
                name.clone(),
                data.entity_type,
                MentionSpan::of(data),
                data.description.as_deref().unwrap_or(""),
                Some(data.importance_score),
            );
            for alias in &data.aliases {
                self.note_alias(name, alias);
            }
        }

        // Merge entity mention lists (HashSet dedupe: O(m) per entity, not O(m²))
        for (name, mentions) in other.entity_mentions.sorted() {
            // The name may be an alias here (spelt differently elsewhere).
            let name = self.resolve(name).unwrap_or(name).to_string();
            self.entity_mentions.upsert(name, SharedVec::new, |entry| {
                let mut seen: HashSet<uuid::Uuid> = entry.iter().copied().collect();
                for id in mentions.iter() {
                    if seen.insert(*id) {
                        entry.push(*id);
                    }
                }
            });
        }

        // Merge edges
        for edge in other.relationships() {
            let (from_name, to_name) = (edge.from.to_string(), edge.to.to_string());
            // Endpoints were merged above; one without entity data (only
            // in a corrupt graph) is stamped with the latest mention.
            let at = other
                .entities
                .get(&from_name)
                .or_else(|| other.entities.get(&to_name))
                .map(|e| e.last_mentioned)
                .or_else(|| other.latest_mention())
                .unwrap_or(DateTime::<Utc>::UNIX_EPOCH);
            self.add_relationship(
                EntityRelationship {
                    from_entity: from_name,
                    to_entity: to_name,
                    relation_type: *edge.relation_type,
                    context: edge.context.to_string(),
                },
                at,
            );
        }
    }
}
