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

//! [`Storage`] trait implementation for [`SurrealDBStorage`].
//!
//! Covers session lifecycle, context-update batches, checkpoints, workspaces,
//! and stats — every method that the in-memory layer reaches for during
//! ordinary read/write traffic.

use anyhow::Result;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use tracing::{debug, info};
use uuid::Uuid;

use post_cortex_core::core::context_update::ContextUpdate;
use post_cortex_core::graph::entity_graph::SimpleEntityGraph;
use post_cortex_core::session::active_session::{ActiveSession, UserPreferences};
use crate::rocksdb_storage::{SessionCheckpoint, StoredWorkspace};
use crate::traits::{GraphStorage, Storage};
use post_cortex_core::workspace::SessionRole;

use super::SurrealDBStorage;
use super::records::{
    CheckpointRecord, ContextUpdateRecord, EntityRecord, SessionRecord, WorkspaceRecord,
    WorkspaceSessionRecord,
};

#[async_trait]
impl Storage for SurrealDBStorage {
    async fn save_session(&self, session: &ActiveSession) -> Result<()> {
        debug!(
            "SurrealDBStorage: Saving session with ID: {} (normalized, batched)",
            session.id()
        );

        let session_id = session.id();
        let all_updates: Vec<ContextUpdate> = session.hot_context.iter();
        let entities = session.entity_graph.get_all_entities();
        let relationships = session.entity_graph.get_all_relationships();
        let total_updates = all_updates.len() as u32;

        // Build session metadata JSON for the transaction
        let record = SessionRecord {
            session_id: session_id.to_string(),
            name: session.name().clone(),
            description: session.description().clone(),
            created_at: session.created_at().to_rfc3339(),
            last_updated: Utc::now().to_rfc3339(),
            user_preferences: serde_json::to_value(session.user_preferences())?,
            vectorized_update_ids: session
                .vectorized_update_ids
                .iter()
                .map(|id| id.to_string())
                .collect(),
            total_updates,
        };

        // Build a single batched SurrealQL transaction
        let mut query_parts: Vec<String> =
            Vec::with_capacity(1 + all_updates.len() + entities.len() + relationships.len());

        // 1. Session metadata upsert
        let session_json = serde_json::to_string(&record)?;
        query_parts.push(format!(
            "UPSERT session:`{}` CONTENT {};",
            session_id, session_json
        ));

        // 2. Context updates
        for update in &all_updates {
            let update_record = ContextUpdateRecord {
                update_id: update.id.to_string(),
                session_id: session_id.to_string(),
                timestamp: update.timestamp.to_rfc3339(),
                update_type: format!("{:?}", update.update_type),
                update_data: serde_json::to_value(update)?,
            };
            let record_json = serde_json::to_string(&update_record)?;
            query_parts.push(format!(
                "UPSERT context_update:`{}` CONTENT {};",
                update.id, record_json
            ));
        }

        // 3. Entities
        for entity in &entities {
            let entity_id = Self::entity_id(session_id, &entity.name);
            let entity_record = EntityRecord {
                session_id: session_id.to_string(),
                name: entity.name.clone(),
                entity_type: format!("{:?}", entity.entity_type),
                first_mentioned: entity.first_mentioned.to_rfc3339(),
                last_mentioned: entity.last_mentioned.to_rfc3339(),
                mention_count: entity.mention_count,
                importance_score: entity.importance_score,
                description: entity.description.clone(),
            };
            let record_json = serde_json::to_string(&entity_record)?;
            query_parts.push(format!(
                "UPSERT entity:`{}` CONTENT {};",
                entity_id, record_json
            ));
        }

        // 4. Relationships
        for rel in &relationships {
            let from_id = Self::entity_id(session_id, &rel.from_entity);
            let to_id = Self::entity_id(session_id, &rel.to_entity);
            let table = Self::relation_table_name(&rel.relation_type);
            let rel_id = Self::relation_id(
                session_id,
                &rel.from_entity,
                &rel.to_entity,
                &rel.relation_type,
            );
            let context_json = serde_json::to_string(&rel.context)?;
            query_parts.push(format!(
                "UPSERT {}:`{}` SET in = entity:`{}`, out = entity:`{}`, context = {}, session_id = '{}';",
                table, rel_id, from_id, to_id, context_json, session_id
            ));
        }

        // Execute all statements in a single multi-statement query (1 network round-trip)
        // No BEGIN/COMMIT — each statement runs in its own implicit transaction,
        // so individual failures don't cascade.
        let full_query = query_parts.join(" ");

        debug!(
            "SurrealDBStorage: Executing batched query - {} statements",
            query_parts.len()
        );

        let response = self.db.query(&full_query).await?;
        // Don't use response.check() — individual statement errors are non-fatal
        // (matches original behavior where each upsert was independently try/catch'd)
        if let Err(e) = response.check() {
            debug!(
                "SurrealDBStorage: Some statements had errors (non-fatal): {}",
                e
            );
        }

        debug!(
            "SurrealDBStorage: Session saved (batched) - {} updates, {} entities, {} rels",
            all_updates.len(),
            entities.len(),
            relationships.len()
        );

        Ok(())
    }

    async fn load_session(&self, session_id: Uuid) -> Result<ActiveSession> {
        debug!(
            "SurrealDBStorage: Loading session with ID: {} (normalized)",
            session_id
        );

        // Load session metadata
        let record: Option<SessionRecord> =
            self.db.select(("session", session_id.to_string())).await?;

        let r = record.ok_or_else(|| anyhow::anyhow!("Session not found"))?;

        // Load ALL context updates from normalized table (source of truth!)
        let all_updates = self
            .load_session_updates(session_id)
            .await
            .unwrap_or_default();
        debug!(
            "SurrealDBStorage: Loaded {} updates from context_update table",
            all_updates.len()
        );

        // Hot context = last 50 updates (same as HotContext::MAX_SIZE)
        let hot_context: Vec<ContextUpdate> =
            all_updates.iter().rev().take(50).rev().cloned().collect();

        // Load entities from native graph table
        let entities = self.list_entities(session_id).await.unwrap_or_default();
        let mut entity_graph = SimpleEntityGraph::new();
        for entity in &entities {
            entity_graph.add_or_update_entity(
                entity.name.clone(),
                entity.entity_type.clone(),
                entity.last_mentioned,
                entity.description.as_deref().unwrap_or(""),
            );
        }

        // Load relationships from native graph and add to entity_graph
        let relationships = self
            .load_all_relationships(session_id)
            .await
            .unwrap_or_default();
        for rel in relationships {
            entity_graph.add_relationship(rel);
        }
        debug!(
            "SurrealDBStorage: Loaded {} entities, graph rebuilt",
            entities.len()
        );

        // Rebuild StructuredContext from updates
        let current_state = Self::rebuild_structured_context(&all_updates);

        // Extract code_references and change_history from updates
        let code_references = Self::extract_code_references(&all_updates);
        let change_history = Self::extract_change_history(&all_updates);

        // Parse user preferences
        let user_preferences: UserPreferences = serde_json::from_value(r.user_preferences)
            .unwrap_or_else(|_| UserPreferences {
                auto_save_enabled: true,
                context_retention_days: 30,
                max_hot_context_size: 50,
                auto_summary_threshold: 100,
                important_keywords: Vec::new(),
            });

        // Parse vectorized_update_ids
        let vectorized_ids: Vec<Uuid> = r
            .vectorized_update_ids
            .iter()
            .filter_map(|s| Uuid::parse_str(s).ok())
            .collect();

        // Reconstruct ActiveSession from normalized data
        let session = ActiveSession::from_components(
            session_id,
            r.name,
            r.description,
            DateTime::parse_from_rfc3339(&r.created_at)
                .map(|dt| dt.with_timezone(&Utc))
                .unwrap_or_else(|_| Utc::now()),
            DateTime::parse_from_rfc3339(&r.last_updated)
                .map(|dt| dt.with_timezone(&Utc))
                .unwrap_or_else(|_| Utc::now()),
            user_preferences,
            hot_context,
            Vec::new(), // warm_context - compressed from older updates if needed
            Vec::new(), // cold_context - summaries generated on demand
            current_state,
            all_updates, // All updates from normalized table
            code_references,
            change_history,
            entity_graph,
            vectorized_ids,
        );

        debug!(
            "SurrealDBStorage: Session loaded (normalized) - {} updates, {} entities",
            session.hot_context.len(),
            entities.len()
        );
        Ok(session)
    }

    async fn delete_session(&self, session_id: Uuid) -> Result<()> {
        debug!("SurrealDBStorage: Deleting session with ID: {}", session_id);

        // Delete session
        let _: Option<SessionRecord> =
            self.db.delete(("session", session_id.to_string())).await?;

        // Delete all context updates for this session
        self.db
            .query("DELETE context_update WHERE session_id = $session_id")
            .bind(("session_id", session_id.to_string()))
            .await?;

        // Delete all entities for this session
        self.db
            .query("DELETE entity WHERE session_id = $session_id")
            .bind(("session_id", session_id.to_string()))
            .await?;

        // Delete all embeddings for this session
        self.db
            .query("DELETE embedding WHERE session_id = $session_id")
            .bind(("session_id", session_id.to_string()))
            .await?;

        // Delete all relations for this session
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
            self.db
                .query(format!("DELETE {} WHERE session_id = $session_id", table))
                .bind(("session_id", session_id.to_string()))
                .await?;
        }

        debug!("SurrealDBStorage: Session deleted successfully");

        Ok(())
    }

    async fn clear_session_entities(&self, session_id: Uuid) -> Result<()> {
        debug!(
            "SurrealDBStorage: Clearing all entities and relationships for session {}",
            session_id
        );

        self.db
            .query("DELETE entity WHERE session_id = $session_id")
            .bind(("session_id", session_id.to_string()))
            .await?;

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
            self.db
                .query(format!("DELETE {} WHERE session_id = $session_id", table))
                .bind(("session_id", session_id.to_string()))
                .await?;
        }

        info!(
            "SurrealDBStorage: Cleared entities and relationships for session {}",
            session_id
        );
        Ok(())
    }

    async fn list_sessions(&self) -> Result<Vec<Uuid>> {
        debug!("SurrealDBStorage: Listing sessions");

        // Raw query + `response.take(0)` routes through serde (honours
        // `deserialize_surreal_option` on the Option<String> fields).
        // `db.select(table)` goes through SurrealValue conversion which
        // rejects NONE for Option<String> and crashed any list with
        // sessions that had `name = null` in storage.
        let mut response = self.db.query("SELECT session_id FROM session").await?;
        let ids: Vec<String> = response.take("session_id")?;

        let sessions: Vec<Uuid> = ids
            .into_iter()
            .filter_map(|s| Uuid::parse_str(&s).ok())
            .collect();

        debug!("SurrealDBStorage: Found {} sessions", sessions.len());

        Ok(sessions)
    }

    async fn session_exists(&self, session_id: Uuid) -> Result<bool> {
        let record: Option<SessionRecord> =
            self.db.select(("session", session_id.to_string())).await?;

        Ok(record.is_some())
    }

    async fn batch_save_updates(
        &self,
        session_id: Uuid,
        updates: Vec<ContextUpdate>,
    ) -> Result<()> {
        if updates.is_empty() {
            return Ok(());
        }

        debug!(
            "SurrealDBStorage: Batch saving {} updates for session {} (single transaction)",
            updates.len(),
            session_id
        );

        // Build all upserts into a single transaction
        let mut query_parts: Vec<String> = Vec::with_capacity(updates.len());

        for update in &updates {
            let record = ContextUpdateRecord {
                update_id: update.id.to_string(),
                session_id: session_id.to_string(),
                timestamp: update.timestamp.to_rfc3339(),
                update_type: format!("{:?}", update.update_type),
                update_data: serde_json::to_value(update)?,
            };
            let record_json = serde_json::to_string(&record)?;
            query_parts.push(format!(
                "UPSERT context_update:`{}` CONTENT {};",
                update.id, record_json
            ));
        }

        let full_query = query_parts.join(" ");
        let response = self.db.query(&full_query).await?;
        if let Err(e) = response.check() {
            debug!(
                "SurrealDBStorage: Some batch_save statements had errors (non-fatal): {}",
                e
            );
        }

        debug!(
            "SurrealDBStorage: Batch save completed - {} updates in 1 query",
            updates.len()
        );

        Ok(())
    }

    async fn load_session_updates(&self, session_id: Uuid) -> Result<Vec<ContextUpdate>> {
        debug!(
            "SurrealDBStorage: Loading updates for session {}",
            session_id
        );
        info!(
            "SurrealDBStorage: Fetching updates for session {}...",
            session_id
        );

        let mut updates = Vec::new();
        let mut start = 0;
        let limit = 1000;

        loop {
            let mut response = self
                .db
                .query("SELECT * FROM context_update WHERE session_id = $session_id ORDER BY timestamp LIMIT $limit START $start")
                .bind(("session_id", session_id.to_string()))
                .bind(("limit", limit))
                .bind(("start", start))
                .await?;

            let records: Vec<ContextUpdateRecord> = response.take(0)?;
            let count = records.len();

            if count == 0 {
                break;
            }

            // Parse update_data JSON back to ContextUpdate
            for r in records {
                if let Ok(update) = serde_json::from_value(r.update_data) {
                    updates.push(update);
                }
            }

            if updates.len() % 1000 == 0 {
                info!(
                    "SurrealDBStorage: Loaded {} updates for session {}...",
                    updates.len(),
                    session_id
                );
            }

            if count < limit {
                break;
            }

            start += count;
        }

        debug!("SurrealDBStorage: Loaded {} updates", updates.len());

        Ok(updates)
    }

    async fn save_checkpoint(&self, checkpoint: &SessionCheckpoint) -> Result<()> {
        debug!(
            "SurrealDBStorage: Saving checkpoint with ID: {}",
            checkpoint.id
        );

        let record = CheckpointRecord {
            checkpoint_id: checkpoint.id.to_string(),
            session_id: checkpoint.session_id.to_string(),
            created_at: checkpoint.created_at.to_rfc3339(),
            structured_context: serde_json::to_value(&checkpoint.structured_context)?,
            recent_updates: serde_json::to_value(&checkpoint.recent_updates)?,
            code_references: serde_json::to_value(&checkpoint.code_references)?,
            change_history: serde_json::to_value(&checkpoint.change_history)?,
            total_updates: checkpoint.total_updates as u32,
            context_quality_score: checkpoint.context_quality_score,
            compression_ratio: checkpoint.compression_ratio,
        };

        let _: Option<CheckpointRecord> = self
            .db
            .upsert(("checkpoint", checkpoint.id.to_string()))
            .content(record)
            .await?;

        debug!("SurrealDBStorage: Checkpoint saved successfully");

        Ok(())
    }

    async fn load_checkpoint(&self, checkpoint_id: Uuid) -> Result<SessionCheckpoint> {
        debug!(
            "SurrealDBStorage: Loading checkpoint with ID: {}",
            checkpoint_id
        );

        let record: Option<CheckpointRecord> = self
            .db
            .select(("checkpoint", checkpoint_id.to_string()))
            .await?;

        match record {
            Some(r) => {
                let checkpoint = SessionCheckpoint {
                    id: Uuid::parse_str(&r.checkpoint_id)?,
                    session_id: Uuid::parse_str(&r.session_id)?,
                    created_at: DateTime::parse_from_rfc3339(&r.created_at)
                        .map(|dt| dt.with_timezone(&Utc))
                        .unwrap_or_else(|_| Utc::now()),
                    structured_context: serde_json::from_value(r.structured_context)?,
                    recent_updates: serde_json::from_value(r.recent_updates)?,
                    code_references: serde_json::from_value(r.code_references)?,
                    change_history: serde_json::from_value(r.change_history)?,
                    total_updates: r.total_updates as usize,
                    context_quality_score: r.context_quality_score,
                    compression_ratio: r.compression_ratio,
                };
                Ok(checkpoint)
            }
            None => Err(anyhow::anyhow!("Checkpoint not found")),
        }
    }

    async fn list_checkpoints(&self) -> Result<Vec<SessionCheckpoint>> {
        debug!("SurrealDBStorage: Listing checkpoints");

        let records: Vec<CheckpointRecord> = self.select_all("checkpoint").await?;

        let checkpoints: Vec<SessionCheckpoint> = records
            .into_iter()
            .filter_map(|r| {
                Some(SessionCheckpoint {
                    id: Uuid::parse_str(&r.checkpoint_id).ok()?,
                    session_id: Uuid::parse_str(&r.session_id).ok()?,
                    created_at: DateTime::parse_from_rfc3339(&r.created_at)
                        .map(|dt| dt.with_timezone(&Utc))
                        .unwrap_or_else(|_| Utc::now()),
                    structured_context: serde_json::from_value(r.structured_context).ok()?,
                    recent_updates: serde_json::from_value(r.recent_updates).ok()?,
                    code_references: serde_json::from_value(r.code_references).ok()?,
                    change_history: serde_json::from_value(r.change_history).ok()?,
                    total_updates: r.total_updates as usize,
                    context_quality_score: r.context_quality_score,
                    compression_ratio: r.compression_ratio,
                })
            })
            .collect();

        debug!(
            "SurrealDBStorage: Listed {} checkpoints",
            checkpoints.len()
        );

        Ok(checkpoints)
    }

    async fn save_workspace_metadata(
        &self,
        workspace_id: Uuid,
        name: &str,
        description: &str,
        session_ids: &[Uuid],
    ) -> Result<()> {
        debug!(
            "SurrealDBStorage: Saving workspace {} ({})",
            name, workspace_id
        );

        let created_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let record = WorkspaceRecord {
            workspace_id: workspace_id.to_string(),
            name: name.to_string(),
            description: description.to_string(),
            created_at,
        };

        let _: Option<WorkspaceRecord> = self
            .db
            .upsert(("workspace", workspace_id.to_string()))
            .content(record)
            .await?;

        // Add session associations
        for session_id in session_ids {
            self.add_session_to_workspace(workspace_id, *session_id, SessionRole::Primary)
                .await?;
        }

        debug!("SurrealDBStorage: Workspace saved successfully");

        Ok(())
    }

    async fn delete_workspace(&self, workspace_id: Uuid) -> Result<()> {
        debug!("SurrealDBStorage: Deleting workspace {}", workspace_id);

        // Delete workspace
        let _: Option<WorkspaceRecord> = self
            .db
            .delete(("workspace", workspace_id.to_string()))
            .await?;

        // Delete all workspace-session associations
        self.db
            .query("DELETE workspace_session WHERE workspace_id = $workspace_id")
            .bind(("workspace_id", workspace_id.to_string()))
            .await?;

        debug!("SurrealDBStorage: Workspace deleted successfully");

        Ok(())
    }

    async fn list_workspaces(&self) -> Result<Vec<StoredWorkspace>> {
        debug!("SurrealDBStorage: Listing workspaces");

        let records: Vec<WorkspaceRecord> = self.select_all("workspace").await?;

        let mut workspaces = Vec::new();
        for record in records {
            if let Ok(workspace_id) = Uuid::parse_str(&record.workspace_id) {
                // Get sessions for this workspace
                let mut response = self
                    .db
                    .query("SELECT * FROM workspace_session WHERE workspace_id = $workspace_id")
                    .bind(("workspace_id", record.workspace_id.clone()))
                    .await?;

                let session_records: Vec<WorkspaceSessionRecord> = response.take(0)?;

                let sessions: Vec<(Uuid, SessionRole)> = session_records
                    .into_iter()
                    .filter_map(|s| {
                        Uuid::parse_str(&s.session_id).ok().map(|id| {
                            let role = match s.role.as_str() {
                                "Primary" => SessionRole::Primary,
                                "Related" => SessionRole::Related,
                                "Dependency" => SessionRole::Dependency,
                                "Shared" => SessionRole::Shared,
                                _ => SessionRole::Primary,
                            };
                            (id, role)
                        })
                    })
                    .collect();

                workspaces.push(StoredWorkspace {
                    id: workspace_id,
                    name: record.name,
                    description: record.description,
                    sessions,
                    created_at: record.created_at,
                });
            }
        }

        debug!("SurrealDBStorage: Listed {} workspaces", workspaces.len());

        Ok(workspaces)
    }

    async fn add_session_to_workspace(
        &self,
        workspace_id: Uuid,
        session_id: Uuid,
        role: SessionRole,
    ) -> Result<()> {
        debug!(
            "SurrealDBStorage: Adding session {} to workspace {} with role {:?}",
            session_id, workspace_id, role
        );

        let added_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let record = WorkspaceSessionRecord {
            workspace_id: workspace_id.to_string(),
            session_id: session_id.to_string(),
            role: format!("{:?}", role),
            added_at,
        };

        let key = format!("{}_{}", workspace_id, session_id);
        let _: Option<WorkspaceSessionRecord> = self
            .db
            .upsert(("workspace_session", key))
            .content(record)
            .await?;

        Ok(())
    }

    async fn remove_session_from_workspace(
        &self,
        workspace_id: Uuid,
        session_id: Uuid,
    ) -> Result<()> {
        debug!(
            "SurrealDBStorage: Removing session {} from workspace {}",
            session_id, workspace_id
        );

        let key = format!("{}_{}", workspace_id, session_id);
        let _: Option<WorkspaceSessionRecord> = self.delete("workspace_session", &key).await?;

        Ok(())
    }

    async fn compact(&self) -> Result<()> {
        debug!("SurrealDBStorage: Compact requested (no-op for SurrealDB)");
        // SurrealDB handles compaction internally
        Ok(())
    }

    async fn get_key_count(&self) -> Result<usize> {
        let mut response = self
            .db
            .query(
                r#"
                RETURN (SELECT count() FROM session GROUP ALL).count +
                       (SELECT count() FROM context_update GROUP ALL).count +
                       (SELECT count() FROM entity GROUP ALL).count +
                       (SELECT count() FROM embedding GROUP ALL).count +
                       (SELECT count() FROM workspace GROUP ALL).count +
                       (SELECT count() FROM checkpoint GROUP ALL).count
            "#,
            )
            .await?;

        let count: Option<i64> = response.take(0)?;

        Ok(count.unwrap_or(0) as usize)
    }

    async fn get_stats(&self) -> Result<String> {
        let mut response = self
            .db
            .query(
                r#"
                RETURN {
                    sessions: (SELECT count() FROM session GROUP ALL).count,
                    updates: (SELECT count() FROM context_update GROUP ALL).count,
                    entities: (SELECT count() FROM entity GROUP ALL).count,
                    embeddings: (SELECT count() FROM embedding GROUP ALL).count,
                    workspaces: (SELECT count() FROM workspace GROUP ALL).count,
                    checkpoints: (SELECT count() FROM checkpoint GROUP ALL).count
                }
            "#,
            )
            .await?;

        let stats: Option<serde_json::Value> = response.take(0)?;

        Ok(serde_json::to_string_pretty(&stats.unwrap_or_default())?)
    }
}

