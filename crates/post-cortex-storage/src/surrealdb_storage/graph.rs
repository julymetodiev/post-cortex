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

use anyhow::Result;
use async_trait::async_trait;
use serde::Deserialize;
use std::collections::{BTreeMap, HashMap};
use surrealdb::types::SurrealValue;
use tracing::{debug, info};
use uuid::Uuid;

use post_cortex_core::core::context_update::{ContextUpdate, EntityData, EntityRelationship, RelationType};
use post_cortex_core::core::structured_context::StructuredContext;
use post_cortex_core::graph::entity_graph::EntityNetwork;
use post_cortex_core::session::active_session::{ChangeRecord, CodeReference};
use crate::traits::GraphStorage;

use super::SurrealDBStorage;
use super::records::EntityRecord;

#[async_trait]
impl GraphStorage for SurrealDBStorage {
    async fn upsert_entity(&self, session_id: Uuid, entity: &EntityData) -> Result<()> {
        debug!(
            "SurrealDBStorage: Upserting entity '{}' for session {}",
            entity.name, session_id
        );

        let entity_id = Self::entity_id(session_id, &entity.name);

        let record = EntityRecord {
            session_id: session_id.to_string(),
            name: entity.name.clone(),
            entity_type: format!("{:?}", entity.entity_type),
            first_mentioned: entity.first_mentioned.to_rfc3339(),
            last_mentioned: entity.last_mentioned.to_rfc3339(),
            mention_count: entity.mention_count,
            importance_score: entity.importance_score,
            description: entity.description.clone(),
        };

        // Use query with backticks for IDs containing special chars (UUIDs have dashes)
        let query = format!("UPSERT entity:`{}` CONTENT $content", entity_id);
        self.db.query(query).bind(("content", record)).await?;

        Ok(())
    }

    async fn get_entity(&self, session_id: Uuid, name: &str) -> Result<Option<EntityData>> {
        let entity_id = Self::entity_id(session_id, name);

        let record: Option<EntityRecord> = self.select_one("entity", &entity_id).await?;

        Ok(record.map(|r| EntityData {
            name: r.name,
            entity_type: Self::parse_entity_type(&r.entity_type),
            first_mentioned: Self::parse_datetime(&r.first_mentioned),
            last_mentioned: Self::parse_datetime(&r.last_mentioned),
            mention_count: r.mention_count,
            importance_score: r.importance_score,
            description: r.description,
        }))
    }

    async fn list_entities(&self, session_id: Uuid) -> Result<Vec<EntityData>> {
        info!(
            "SurrealDBStorage: Fetching entities for session {}...",
            session_id
        );
        let mut all_entities = Vec::new();
        let mut start = 0;
        let limit = 1000;

        loop {
            let mut response = self
                .db
                .query("SELECT * FROM entity WHERE session_id = $session_id ORDER BY importance_score DESC LIMIT $limit START $start")
                .bind(("session_id", session_id.to_string()))
                .bind(("limit", limit))
                .bind(("start", start))
                .await?;

            let records: Vec<EntityRecord> = response.take(0)?;
            let count = records.len();

            if count == 0 {
                break;
            }

            for r in records {
                all_entities.push(EntityData {
                    name: r.name,
                    entity_type: Self::parse_entity_type(&r.entity_type),
                    first_mentioned: Self::parse_datetime(&r.first_mentioned),
                    last_mentioned: Self::parse_datetime(&r.last_mentioned),
                    mention_count: r.mention_count,
                    importance_score: r.importance_score,
                    description: r.description,
                });
            }

            if all_entities.len() % 1000 == 0 {
                info!(
                    "SurrealDBStorage: Loaded {} entities for session {}...",
                    all_entities.len(),
                    session_id
                );
            }

            if count < limit {
                break;
            }

            start += count;
        }

        Ok(all_entities)
    }

    async fn delete_entity(&self, session_id: Uuid, name: &str) -> Result<()> {
        let entity_id = Self::entity_id(session_id, name);

        // Cascade: drop every edge that references this entity in any of
        // the 8 relation tables. Without this, deleted entities leave
        // orphan edges (in/out pointing at non-existent nodes), which is
        // what `pcx-inspect.py junk` had to clean up separately.
        let entity_thing = format!("entity:`{}`", entity_id);
        for table in [
            "required_by",
            "leads_to",
            "related_to",
            "conflicts_with",
            "depends_on",
            "implements",
            "caused_by",
            "solves",
        ] {
            let query = format!(
                "DELETE {} WHERE session_id = $session_id AND (in = {} OR out = {})",
                table, entity_thing, entity_thing,
            );
            self.db
                .query(query)
                .bind(("session_id", session_id.to_string()))
                .await?;
        }

        let _: Option<EntityRecord> = self.delete("entity", &entity_id).await?;
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

        let from_id = Self::entity_id(session_id, &relationship.from_entity);
        let to_id = Self::entity_id(session_id, &relationship.to_entity);
        let table = Self::relation_table_name(&relationship.relation_type);
        let rel_id = Self::relation_id(
            session_id,
            &relationship.from_entity,
            &relationship.to_entity,
            &relationship.relation_type,
        );

        // Use UPSERT for the relationship table with deterministic ID
        // Note: In SurrealDB, for graph edges, you can also UPSERT the edge table directly
        let query = format!(
            "UPSERT {}:`{}` SET in = entity:`{}`, out = entity:`{}`, context = $context, session_id = $session_id",
            table, rel_id, from_id, to_id
        );

        self.db
            .query(query)
            .bind(("context", relationship.context.clone()))
            .bind(("session_id", session_id.to_string()))
            .await?;

        Ok(())
    }

    async fn find_related_entities(
        &self,
        session_id: Uuid,
        entity_name: &str,
    ) -> Result<Vec<String>> {
        let entity_id = Self::entity_id(session_id, entity_name);

        // Query all outgoing and incoming relations (backticks for IDs with special chars)
        let query = format!(
            r#"
            SELECT array::distinct(array::flatten([
                (SELECT VALUE out.name FROM entity:`{}`->*->entity WHERE session_id = $session_id),
                (SELECT VALUE in.name FROM entity:`{}`<-*<-entity WHERE session_id = $session_id)
            ])) AS related
            "#,
            entity_id, entity_id
        );

        let mut response = self
            .db
            .query(query)
            .bind(("session_id", session_id.to_string()))
            .await?;

        #[derive(Deserialize, SurrealValue)]
        struct RelatedResult {
            related: Vec<String>,
        }

        let results: Option<RelatedResult> = response.take(0)?;

        Ok(results.map(|r| r.related).unwrap_or_default())
    }

    async fn find_related_by_type(
        &self,
        session_id: Uuid,
        entity_name: &str,
        relation_type: &RelationType,
    ) -> Result<Vec<String>> {
        let entity_id = Self::entity_id(session_id, entity_name);
        let table = Self::relation_table_name(relation_type);

        let query = format!(
            r#"
            SELECT array::distinct(array::flatten([
                (SELECT VALUE out.name FROM entity:`{}`->{table}->entity WHERE session_id = $session_id),
                (SELECT VALUE in.name FROM entity:`{}`<-{table}<-entity WHERE session_id = $session_id)
            ])) AS related
            "#,
            entity_id, entity_id
        );

        let mut response = self
            .db
            .query(query)
            .bind(("session_id", session_id.to_string()))
            .await?;

        #[derive(Deserialize, SurrealValue)]
        struct RelatedResult {
            related: Vec<String>,
        }

        let results: Option<RelatedResult> = response.take(0)?;

        Ok(results.map(|r| r.related).unwrap_or_default())
    }

    async fn find_shortest_path(
        &self,
        session_id: Uuid,
        from: &str,
        to: &str,
    ) -> Result<Option<Vec<String>>> {
        let from_id = Self::entity_id(session_id, from);
        let to_id = Self::entity_id(session_id, to);

        // Use SurrealDB's graph traversal (backticks for IDs with special chars)
        let query = format!(
            r#"
            SELECT VALUE array::flatten([
                entity:`{}`.name,
                (SELECT VALUE name FROM entity:`{}`->*..5->entity:`{}` WHERE session_id = $session_id),
                entity:`{}`.name
            ])
            "#,
            from_id, from_id, to_id, to_id
        );

        let mut response = self
            .db
            .query(query)
            .bind(("session_id", session_id.to_string()))
            .await?;

        let path: Option<Vec<String>> = response.take(0)?;

        Ok(path.filter(|p| !p.is_empty()))
    }

    async fn get_entity_network(
        &self,
        session_id: Uuid,
        center: &str,
        max_depth: usize,
    ) -> Result<EntityNetwork> {
        let entity_id = Self::entity_id(session_id, center);

        // Get entities within max_depth hops (backticks for IDs with special chars)
        let entity_query = format!(
            r#"
            SELECT name, entity_type, first_mentioned, last_mentioned,
                   mention_count, importance_score, description
            FROM entity:`{}`<->*..{}->entity WHERE session_id = $session_id
            "#,
            entity_id, max_depth
        );

        let mut response = self
            .db
            .query(entity_query)
            .bind(("session_id", session_id.to_string()))
            .await?;

        let entity_records: Vec<EntityRecord> = response.take(0)?;

        let mut entities: BTreeMap<String, EntityData> = entity_records
            .into_iter()
            .map(|r| {
                (
                    r.name.clone(),
                    EntityData {
                        name: r.name,
                        entity_type: Self::parse_entity_type(&r.entity_type),
                        first_mentioned: Self::parse_datetime(&r.first_mentioned),
                        last_mentioned: Self::parse_datetime(&r.last_mentioned),
                        mention_count: r.mention_count,
                        importance_score: r.importance_score,
                        description: r.description,
                    },
                )
            })
            .collect();

        // Add center entity if not already included
        if let Some(center_entity) = self.get_entity(session_id, center).await? {
            entities.insert(center.to_string(), center_entity);
        }

        // Get relationships (simplified - just get direct connections from center)
        let mut relationships = Vec::new();

        // Query each relation type
        for (table, rel_type) in [
            ("required_by", RelationType::RequiredBy),
            ("leads_to", RelationType::LeadsTo),
            ("related_to", RelationType::RelatedTo),
            ("conflicts_with", RelationType::ConflictsWith),
            ("depends_on", RelationType::DependsOn),
            ("implements", RelationType::Implements),
            ("caused_by", RelationType::CausedBy),
            ("solves", RelationType::Solves),
        ] {
            let rel_query = format!(
                r#"
                SELECT in.name AS from_entity, out.name AS to_entity, context
                FROM {}
                WHERE session_id = $session_id
                "#,
                table
            );

            let mut rel_response = self
                .db
                .query(rel_query)
                .bind(("session_id", session_id.to_string()))
                .await?;

            #[derive(Deserialize, SurrealValue)]
            struct RelRecord {
                from_entity: String,
                to_entity: String,
                context: String,
            }

            let rel_records: Vec<RelRecord> = rel_response.take(0).unwrap_or_default();

            for record in rel_records {
                if entities.contains_key(&record.from_entity)
                    && entities.contains_key(&record.to_entity)
                {
                    relationships.push(EntityRelationship {
                        from_entity: record.from_entity,
                        to_entity: record.to_entity,
                        relation_type: rel_type.clone(),
                        context: record.context,
                    });
                }
            }
        }

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
    /// Load all relationships for a session from native graph
    pub(super) async fn load_all_relationships(
        &self,
        session_id: Uuid,
    ) -> Result<Vec<EntityRelationship>> {
        info!(
            "SurrealDBStorage: Fetching relationships for session {}...",
            session_id
        );
        let mut all_relationships = Vec::new();

        // Query each relation type table
        let relation_tables = [
            ("required_by", RelationType::RequiredBy),
            ("leads_to", RelationType::LeadsTo),
            ("related_to", RelationType::RelatedTo),
            ("conflicts_with", RelationType::ConflictsWith),
            ("depends_on", RelationType::DependsOn),
            ("implements", RelationType::Implements),
            ("caused_by", RelationType::CausedBy),
            ("solves", RelationType::Solves),
        ];

        for (table, rel_type) in relation_tables {
            debug!(
                "SurrealDBStorage: Processing relationship table '{}'...",
                table
            );
            let mut start = 0;
            let limit = 1000;

            loop {
                let query = format!(
                    r#"
                    SELECT
                        in.name AS from_entity,
                        out.name AS to_entity,
                        context
                    FROM {}
                    WHERE session_id = $session_id
                    LIMIT $limit START $start
                    "#,
                    table
                );

                let mut response = self
                    .db
                    .query(query)
                    .bind(("session_id", session_id.to_string()))
                    .bind(("limit", limit))
                    .bind(("start", start))
                    .await?;

                #[derive(Deserialize, SurrealValue)]
                struct RelRecord {
                    from_entity: String,
                    to_entity: String,
                    context: String,
                }

                let records: Vec<RelRecord> = response.take(0).unwrap_or_default();
                let count = records.len();

                if count == 0 {
                    break;
                }

                if count > 0 {
                    info!(
                        "SurrealDBStorage: Loaded {} relationships from '{}' (start: {})...",
                        count, table, start
                    );
                }

                for record in records {
                    all_relationships.push(EntityRelationship {
                        from_entity: record.from_entity,
                        to_entity: record.to_entity,
                        relation_type: rel_type.clone(),
                        context: record.context,
                    });
                }

                if !all_relationships.is_empty() && all_relationships.len() % 1000 == 0 {
                    debug!(
                        "SurrealDBStorage: Loaded {} relationships...",
                        all_relationships.len()
                    );
                }

                if count < limit {
                    break;
                }

                start += count;
            }
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
