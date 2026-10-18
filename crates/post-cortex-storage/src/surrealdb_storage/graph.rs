// Copyright (c) 2025, 2026 Julius ML
//
// Permission is hereby granted, free of charge, to any person obtaining a copy
// of this software and associated documentation files (the "Software"), to deal
// in the Software without restriction, including without limitation the rights
// to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
// copies of the Software, and to permit persons to whom the Software is
// furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in all
// copies or substantial portions of the Software.

//! [`GraphStorage`] trait implementation for [`SurrealDBStorage`] plus the
//! private helpers used by `load_session` to rebuild derived structures
//! (`StructuredContext`, `code_references`, `change_history`).
//!
//! Entity names are user data, so record ids are always bound as
//! [`RecordId`] parameters, never spliced into SurrealQL.

use anyhow::Result;
use async_trait::async_trait;
use serde::Deserialize;
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use surrealdb::types::{RecordId, SurrealValue};
use tracing::debug;
use uuid::Uuid;

use crate::traits::GraphStorage;
use post_cortex_core::core::context_update::{
    ContextUpdate, EntityData, EntityRelationship, RelationType,
};
use post_cortex_core::core::structured_context::StructuredContext;
use post_cortex_core::graph::entity_graph::EntityNetwork;
use post_cortex_core::session::active_session::{ChangeRecord, CodeReference};

use super::records::EntityRecord;
use super::{RELATION_TABLES, SurrealDBStorage, per_relation_table};

/// Edge row as selected from a relation table. Endpoint names are optional
/// because an edge can outlive its entity.
#[derive(Deserialize, SurrealValue)]
struct RelRecord {
    #[serde(default)]
    from_entity: Option<String>,
    #[serde(default)]
    to_entity: Option<String>,
    #[serde(default)]
    context: Option<String>,
}

impl SurrealDBStorage {
    /// Record id of an entity node.
    pub(super) fn entity_record_id(session_id: Uuid, name: &str) -> RecordId {
        RecordId::new("entity", Self::entity_id(session_id, name))
    }

    /// Persisted form of an entity.
    pub(super) fn entity_record(session_id: Uuid, entity: &EntityData) -> EntityRecord {
        EntityRecord {
            session_id: session_id.to_string(),
            name: entity.name.clone(),
            entity_type: format!("{:?}", entity.entity_type),
            first_mentioned: entity.first_mentioned.to_rfc3339(),
            last_mentioned: entity.last_mentioned.to_rfc3339(),
            mention_count: entity.mention_count,
            importance_score: entity.importance_score,
            description: entity.description.clone(),
        }
    }

    fn entity_from_record(r: EntityRecord) -> EntityData {
        EntityData {
            name: r.name,
            entity_type: Self::parse_entity_type(&r.entity_type),
            first_mentioned: Self::parse_datetime(&r.first_mentioned),
            last_mentioned: Self::parse_datetime(&r.last_mentioned),
            mention_count: r.mention_count,
            importance_score: r.importance_score,
            description: r.description,
        }
    }

    /// Names of the entities directly connected to `entity_name` through
    /// the given relation tables, in either direction, deduplicated.
    async fn neighbours(
        &self,
        session_id: Uuid,
        entity_name: &str,
        tables: &[&str],
    ) -> Result<Vec<String>> {
        let query = tables
            .iter()
            .map(|table| {
                format!(
                    "SELECT VALUE out.name FROM {table} WHERE in = $e AND session_id = $session_id; \
                     SELECT VALUE in.name FROM {table} WHERE out = $e AND session_id = $session_id;"
                )
            })
            .collect::<Vec<_>>()
            .join(" ");
        let mut response = self
            .db
            .query(query)
            .bind(("e", Self::entity_record_id(session_id, entity_name)))
            .bind(("session_id", session_id.to_string()))
            .await?;

        let mut seen = HashSet::new();
        let mut related = Vec::new();
        for index in 0..tables.len() * 2 {
            let names: Vec<Option<String>> = response.take(index)?;
            for name in names.into_iter().flatten() {
                if seen.insert(name.clone()) {
                    related.push(name);
                }
            }
        }
        related.sort();
        Ok(related)
    }

    /// Entities of a session with the given names, in one query.
    async fn entities_named(
        &self,
        session_id: Uuid,
        names: Vec<String>,
    ) -> Result<BTreeMap<String, EntityData>> {
        let mut response = self
            .db
            .query("SELECT * FROM entity WHERE session_id = $session_id AND name IN $names")
            .bind(("session_id", session_id.to_string()))
            .bind(("names", names))
            .await?;
        let records: Vec<EntityRecord> = response.take(0)?;
        Ok(records
            .into_iter()
            .map(|r| (r.name.clone(), Self::entity_from_record(r)))
            .collect())
    }
}

/// Undirected adjacency over relationships.
fn adjacency(relationships: &[EntityRelationship]) -> HashMap<&str, Vec<&str>> {
    let mut adjacency: HashMap<&str, Vec<&str>> = HashMap::new();
    for rel in relationships {
        adjacency
            .entry(rel.from_entity.as_str())
            .or_default()
            .push(rel.to_entity.as_str());
        adjacency
            .entry(rel.to_entity.as_str())
            .or_default()
            .push(rel.from_entity.as_str());
    }
    adjacency
}

#[async_trait]
impl GraphStorage for SurrealDBStorage {
    async fn upsert_entity(&self, session_id: Uuid, entity: &EntityData) -> Result<()> {
        debug!(
            "SurrealDBStorage: Upserting entity '{}' for session {}",
            entity.name, session_id
        );

        self.db
            .query("UPSERT $rid CONTENT $content")
            .bind(("rid", Self::entity_record_id(session_id, &entity.name)))
            .bind(("content", Self::entity_record(session_id, entity)))
            .await?
            .check()?;
        self.note_entity_written(session_id, entity).await;
        Ok(())
    }

    async fn get_entity(&self, session_id: Uuid, name: &str) -> Result<Option<EntityData>> {
        let entity_id = Self::entity_id(session_id, name);
        let record: Option<EntityRecord> = self.select_one("entity", &entity_id).await?;
        Ok(record.map(Self::entity_from_record))
    }

    async fn list_entities(&self, session_id: Uuid) -> Result<Vec<EntityData>> {
        let mut all_entities = Vec::new();
        let mut start = 0;
        let limit = 1000;

        loop {
            // `name` breaks importance ties so pages never overlap or skip.
            let mut response = self
                .db
                .query(
                    "SELECT * FROM entity WHERE session_id = $session_id \
                     ORDER BY importance_score DESC, name ASC LIMIT $limit START $start",
                )
                .bind(("session_id", session_id.to_string()))
                .bind(("limit", limit))
                .bind(("start", start))
                .await?;

            let records: Vec<EntityRecord> = response.take(0)?;
            let count = records.len();
            all_entities.extend(records.into_iter().map(Self::entity_from_record));

            if count < limit {
                break;
            }
            start += count;
        }

        debug!(
            "SurrealDBStorage: Loaded {} entities for session {}",
            all_entities.len(),
            session_id
        );
        Ok(all_entities)
    }

    async fn delete_entity(&self, session_id: Uuid, name: &str) -> Result<()> {
        // Cascade: drop every edge that references this entity in any of
        // the 8 relation tables, then the node, in one round trip. Without
        // this, deleted entities leave orphan edges (in/out pointing at
        // non-existent nodes).
        let query = format!(
            "{} DELETE $e;",
            per_relation_table(
                "DELETE {table} WHERE session_id = $session_id AND (in = $e OR out = $e);"
            )
        );
        self.db
            .query(query)
            .bind(("e", Self::entity_record_id(session_id, name)))
            .bind(("session_id", session_id.to_string()))
            .await?
            .check()?;
        self.forget_graph_state(session_id).await;
        Ok(())
    }

    async fn create_relationship(
        &self,
        session_id: Uuid,
        relationship: &EntityRelationship,
    ) -> Result<()> {
        debug!(
            "SurrealDBStorage: Creating relationship {} -> {} ({:?})",
            relationship.from_entity, relationship.to_entity, relationship.relation_type
        );

        let table = Self::relation_table_name(&relationship.relation_type);
        let rel_id = Self::relation_id(
            session_id,
            &relationship.from_entity,
            &relationship.to_entity,
            &relationship.relation_type,
        );

        // UPSERT with a deterministic id so re-creating an edge is idempotent.
        self.db
            .query(
                "UPSERT $rel SET in = $from, out = $to, context = $context, \
                 session_id = $session_id",
            )
            .bind(("rel", RecordId::new(table, rel_id)))
            .bind((
                "from",
                Self::entity_record_id(session_id, &relationship.from_entity),
            ))
            .bind((
                "to",
                Self::entity_record_id(session_id, &relationship.to_entity),
            ))
            .bind(("context", relationship.context.clone()))
            .bind(("session_id", session_id.to_string()))
            .await?
            .check()?;
        self.note_relationship_written(session_id, relationship)
            .await;
        Ok(())
    }

    async fn find_related_entities(
        &self,
        session_id: Uuid,
        entity_name: &str,
    ) -> Result<Vec<String>> {
        let tables: Vec<&str> = RELATION_TABLES.iter().map(|(t, _)| *t).collect();
        self.neighbours(session_id, entity_name, &tables).await
    }

    async fn find_related_by_type(
        &self,
        session_id: Uuid,
        entity_name: &str,
        relation_type: &RelationType,
    ) -> Result<Vec<String>> {
        let table = Self::relation_table_name(relation_type);
        self.neighbours(session_id, entity_name, &[table]).await
    }

    /// Shortest undirected path (BFS over all of the session's edges, one
    /// query), like the RocksDB backend.
    async fn find_shortest_path(
        &self,
        session_id: Uuid,
        from: &str,
        to: &str,
    ) -> Result<Option<Vec<String>>> {
        if from == to {
            return Ok(Some(vec![from.to_string()]));
        }
        let relationships = self.load_all_relationships(session_id).await?;
        let adjacency = adjacency(&relationships);

        let mut parent: HashMap<&str, &str> = HashMap::new();
        let mut queue = VecDeque::from([from]);
        let mut visited = HashSet::from([from]);
        while let Some(current) = queue.pop_front() {
            for &next in adjacency.get(current).into_iter().flatten() {
                if !visited.insert(next) {
                    continue;
                }
                parent.insert(next, current);
                if next == to {
                    let mut path = vec![to.to_string()];
                    let mut node = to;
                    while let Some(&prev) = parent.get(node) {
                        path.push(prev.to_string());
                        node = prev;
                    }
                    path.reverse();
                    return Ok(Some(path));
                }
                queue.push_back(next);
            }
        }
        Ok(None)
    }

    /// Entities within `max_depth` undirected hops of `center` plus the
    /// edges among them (two queries).
    async fn get_entity_network(
        &self,
        session_id: Uuid,
        center: &str,
        max_depth: usize,
    ) -> Result<EntityNetwork> {
        let all_relationships = self.load_all_relationships(session_id).await?;
        let adjacency = adjacency(&all_relationships);

        let mut visited: HashSet<&str> = HashSet::from([center]);
        let mut frontier = vec![center];
        for _ in 0..max_depth {
            let mut next = Vec::new();
            for node in &frontier {
                for &neighbour in adjacency.get(node).into_iter().flatten() {
                    if visited.insert(neighbour) {
                        next.push(neighbour);
                    }
                }
            }
            if next.is_empty() {
                break;
            }
            frontier = next;
        }

        let names: Vec<String> = visited.iter().map(|s| s.to_string()).collect();
        let entities = self.entities_named(session_id, names).await?;
        let relationships = all_relationships
            .iter()
            .filter(|r| {
                entities.contains_key(&r.from_entity) && entities.contains_key(&r.to_entity)
            })
            .cloned()
            .collect();

        Ok(EntityNetwork {
            center: center.to_string(),
            entities,
            relationships,
        })
    }
}

// ============================================================================
// Helper Methods for SurrealDB
// ============================================================================

impl SurrealDBStorage {
    /// Load all relationships for a session from the relation tables, in
    /// one round trip (one statement per table).
    pub(super) async fn load_all_relationships(
        &self,
        session_id: Uuid,
    ) -> Result<Vec<EntityRelationship>> {
        let query = per_relation_table(
            "SELECT in.name AS from_entity, out.name AS to_entity, context \
             FROM {table} WHERE session_id = $session_id;",
        );
        let mut response = self
            .db
            .query(query)
            .bind(("session_id", session_id.to_string()))
            .await?;

        let mut all_relationships = Vec::new();
        for (index, (_, rel_type)) in RELATION_TABLES.iter().enumerate() {
            let records: Vec<RelRecord> = response.take(index)?;
            all_relationships.extend(records.into_iter().filter_map(|r| {
                Some(EntityRelationship {
                    from_entity: r.from_entity?,
                    to_entity: r.to_entity?,
                    relation_type: rel_type.clone(),
                    context: r.context.unwrap_or_default(),
                })
            }));
        }

        debug!(
            "SurrealDBStorage: Loaded {} relationships for session {}",
            all_relationships.len(),
            session_id
        );
        Ok(all_relationships)
    }

    /// Rebuild StructuredContext from context updates
    pub(super) fn rebuild_structured_context(updates: &[ContextUpdate]) -> StructuredContext {
        use post_cortex_core::core::context_update::UpdateType;
        use post_cortex_core::core::structured_context::{
            ConceptItem, DecisionItem, FlowItem, QuestionItem, QuestionStatus,
        };

        let mut context = StructuredContext::default();

        for update in updates {
            // Extract key decisions
            if update.update_type == UpdateType::DecisionMade {
                context.key_decisions.push(DecisionItem {
                    description: update.content.title.clone(),
                    context: update.content.description.clone(),
                    alternatives: update.content.details.clone(),
                    confidence: if update.user_marked_important {
                        0.9
                    } else {
                        0.7
                    },
                    timestamp: update.timestamp,
                });
            }

            // Extract questions/answers
            if update.update_type == UpdateType::QuestionAnswered {
                context.open_questions.push(QuestionItem {
                    question: update.content.title.clone(),
                    context: update.content.description.clone(),
                    status: QuestionStatus::Answered,
                    timestamp: update.timestamp,
                    last_updated: update.timestamp,
                });
            }

            // Add to conversation flow (limit to recent 500)
            if context.conversation_flow.len() < 500 {
                context.add_flow_item(FlowItem {
                    step_description: format!(
                        "{}: {}",
                        format!("{:?}", update.update_type),
                        update.content.title
                    ),
                    timestamp: update.timestamp,
                    related_updates: vec![update.id],
                    outcome: if update.content.description.is_empty() {
                        None
                    } else {
                        Some(update.content.description.clone())
                    },
                });
            }

            // Extract key concepts from entities
            for entity in &update.creates_entities {
                let already_exists = context.key_concepts.iter().any(|c| c.name == *entity);
                if !already_exists && context.key_concepts.len() < 50 {
                    context.key_concepts.push(ConceptItem {
                        name: entity.clone(),
                        definition: String::new(),
                        examples: Vec::new(),
                        related_concepts: update.references_entities.clone(),
                        timestamp: update.timestamp,
                    });
                }
            }
        }

        context
    }

    /// Extract code_references from context updates
    /// Returns HashMap<file_path, Vec<CodeReference>> - multiple references per file
    pub(super) fn extract_code_references(
        updates: &[ContextUpdate],
    ) -> HashMap<String, Vec<CodeReference>> {
        let mut refs: HashMap<String, Vec<CodeReference>> = HashMap::new();

        for update in updates {
            if let Some(code_ref) = &update.related_code {
                // Convert from core::context_update::CodeReference to session::active_session::CodeReference
                let session_ref = CodeReference {
                    file_path: code_ref.file_path.clone(),
                    start_line: code_ref.start_line,
                    end_line: code_ref.end_line,
                    code_snippet: code_ref.code_snippet.clone(),
                    commit_hash: code_ref.commit_hash.clone(),
                    branch: code_ref.branch.clone(),
                    change_description: code_ref.change_description.clone(),
                };
                // Collect all references for each file path
                refs.entry(code_ref.file_path.clone())
                    .or_default()
                    .push(session_ref);
            }
        }

        refs
    }

    /// Extract change_history from context updates
    /// Creates ChangeRecord for each CodeChanged update
    pub(super) fn extract_change_history(updates: &[ContextUpdate]) -> Vec<ChangeRecord> {
        use post_cortex_core::core::context_update::UpdateType;

        updates
            .iter()
            .filter(|u| u.update_type == UpdateType::CodeChanged)
            .map(|u| ChangeRecord {
                id: u.id,
                timestamp: u.timestamp,
                change_type: "CodeChanged".to_string(),
                description: u.content.description.clone(),
                related_update_id: u.parent_update,
            })
            .collect()
    }
}
