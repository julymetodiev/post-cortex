// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Names to entities: the resolution key, aliases and explicit merges.

use crate::collections::SharedVec;
use crate::core::context_update::{EntityData, EntityRelationship};
use crate::graph::entity_graph::mentions::MentionSpan;
use crate::graph::entity_graph::{RelationRef, SimpleEntityGraph};
use std::collections::HashSet;

/// Resolution key of an entity name: lowercase, without whitespace, `-` and
/// `_`, so "SurrealDB", "surreal-db" and "Surreal DB" are one entity. Other
/// characters stay significant ("C++" and "C#" differ from "C", paths keep
/// their dots and slashes); merges beyond this are explicit
/// ([`SimpleEntityGraph::merge_entities`]).
pub fn entity_key(name: &str) -> String {
    let key: String = name
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '-' && *c != '_')
        .flat_map(char::to_lowercase)
        .collect();
    if key.is_empty() {
        name.to_lowercase()
    } else {
        key
    }
}

impl SimpleEntityGraph {
    /// The name of the entity `name` refers to: itself, or the entity it is
    /// an alias of (same [`entity_key`], or merged explicitly).
    pub fn resolve(&self, name: &str) -> Option<&str> {
        if let Some((stored, _)) = self.entities.get_key_value(name) {
            return Some(stored);
        }
        self.alias_index.get(&entity_key(name)).map(String::as_str)
    }

    /// Fold entity `alias` into `into`: its mentions, counts, aliases and
    /// relations move over, and its name becomes an alias of `into`
    /// (relations between the two are dropped). Both names may be aliases
    /// themselves. False if either is unknown or both are the same entity.
    pub fn merge_entities(&mut self, alias: &str, into: &str) -> bool {
        let (Some(alias), Some(into)) = (
            self.resolve(alias).map(str::to_string),
            self.resolve(into).map(str::to_string),
        ) else {
            return false;
        };
        if alias == into {
            return false;
        }
        let Some(gone) = self.entities.get(&alias).cloned() else {
            return false;
        };
        let relations: Vec<EntityRelationship> = self
            .relationships_of(&alias)
            .iter()
            .map(RelationRef::to_relationship)
            .collect();
        let mentions = self
            .entity_mentions
            .get(&alias)
            .cloned()
            .unwrap_or_default();
        self.remove_entity(&alias);

        self.record_mentions(
            into.clone(),
            gone.entity_type,
            MentionSpan::of(&gone),
            gone.description.as_deref().unwrap_or(""),
            Some(gone.importance_score),
        );
        if let Some(target) = self.entities.get_mut(&into) {
            for name in std::iter::once(gone.name.clone()).chain(gone.aliases.iter().cloned()) {
                if name != target.name && !target.aliases.contains(&name) {
                    target.aliases.push(name);
                }
            }
        }
        for key in self
            .entities
            .get(&into)
            .map(|e| {
                std::iter::once(&e.name)
                    .chain(&e.aliases)
                    .map(|n| entity_key(n))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
        {
            self.alias_index.insert(key, into.clone());
        }

        if !mentions.is_empty() {
            self.entity_mentions
                .upsert(into.clone(), SharedVec::new, |ids| {
                    let mut seen: HashSet<uuid::Uuid> = ids.iter().copied().collect();
                    for id in mentions.iter() {
                        if seen.insert(*id) {
                            ids.push(*id);
                        }
                    }
                });
        }
        for mut rel in relations {
            if rel.from_entity == alias {
                rel.from_entity = into.clone();
            }
            if rel.to_entity == alias {
                rel.to_entity = into.clone();
            }
            if rel.from_entity != rel.to_entity {
                self.add_relationship(rel, gone.last_mentioned);
            }
        }
        true
    }

    /// (alias, entity) pairs merged explicitly: aliases whose key differs
    /// from their entity's, which replaying the records would not merge.
    pub fn explicit_aliases(&self) -> Vec<(String, String)> {
        self.entities
            .sorted()
            .into_iter()
            .flat_map(|(_, e)| {
                let key = entity_key(&e.name);
                e.aliases
                    .iter()
                    .filter(move |a| entity_key(a) != key)
                    .map(move |a| (a.clone(), e.name.clone()))
            })
            .collect()
    }

    /// Insert (or replace) an entity exactly as given, e.g. when rebuilding
    /// a graph from storage: unlike
    /// [`add_or_update_entity`](Self::add_or_update_entity) it keeps the
    /// stored mention count, importance score and first-mention time.
    ///
    /// An entity whose name resolves to another one (same [`entity_key`],
    /// e.g. two spellings stored before resolution existed) is merged with
    /// it, into the one mentioned first.
    pub fn insert_entity(&mut self, entity: EntityData) {
        let name = entity.name.clone();
        let other = std::iter::once(&entity.name)
            .chain(&entity.aliases)
            .find_map(|n| self.resolve(n).filter(|other| *other != name))
            .map(str::to_string);
        self.entities.insert(name.clone(), entity);
        self.index_names(&name);
        if let Some(other) = other {
            let mine = (self.entities[&name].first_mentioned, &name);
            let theirs = (self.entities[&other].first_mentioned, &other);
            let (alias, into) = if mine < theirs {
                (other.clone(), name.clone())
            } else {
                (name.clone(), other.clone())
            };
            self.merge_entities(&alias, &into);
        }
    }
}
