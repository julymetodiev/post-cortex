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
//!
//! The `context_update` table is the source of truth for a session's
//! updates; the `session` row holds metadata only. Writes send only what
//! differs from [`PersistedState`], i.e. from what this process last read
//! or wrote for the session, in one multi-statement query with bound
//! parameters.

use anyhow::Result;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use surrealdb::types::{RecordId, SurrealValue};
use tracing::{debug, warn};
use uuid::Uuid;

use crate::error::session_not_found;
use crate::rocksdb_storage::{SessionCheckpoint, StoredWorkspace};
use crate::traits::{GraphStorage, SessionSummary, Storage};
use post_cortex_core::core::context_update::{ContextUpdate, EntityData, EntityRelationship};
use post_cortex_core::graph::entity_graph::SimpleEntityGraph;
use post_cortex_core::session::active_session::{ActiveSession, UserPreferences};
use post_cortex_core::workspace::SessionRole;

use super::records::{
    CheckpointRecord, ContextUpdateRecord, EntityRecord, SessionRecord, WorkspaceRecord,
    WorkspaceSessionRecord, deserialize_surreal_option,
};
use super::{SurrealDBStorage, per_relation_table};

/// Sessions tracked in [`PersistedStates`]; past it one entry is dropped per
/// new session, which only costs that session one full write later.
const MAX_TRACKED_SESSIONS: usize = 4096;

/// What this process knows to be stored for one session.
#[derive(Default)]
pub(crate) struct PersistedState {
    /// Updates known to have rows (as of the last load/save). Updates not
    /// in here get their row written; rows are only deleted when a save
    /// names them as removed.
    update_ids: HashSet<Uuid>,
    /// Fingerprint per entity name as last written or read.
    entities: HashMap<String, u64>,
    /// Fingerprint per relation record key as last written or read.
    relations: HashMap<String, u64>,
}

/// Per-session [`PersistedState`]; the mutex also serialises saves of a
/// session.
pub(crate) type PersistedStates = DashMap<Uuid, Arc<tokio::sync::Mutex<Option<PersistedState>>>>;

fn entity_fingerprint(entity: &EntityData) -> u64 {
    let mut h = DefaultHasher::new();
    entity.name.hash(&mut h);
    entity.entity_type.hash(&mut h);
    entity.first_mentioned.hash(&mut h);
    entity.last_mentioned.hash(&mut h);
    entity.mention_count.hash(&mut h);
    entity.importance_score.to_bits().hash(&mut h);
    entity.description.hash(&mut h);
    entity.aliases.hash(&mut h);
    h.finish()
}

fn relation_fingerprint(rel: &EntityRelationship) -> u64 {
    let mut h = DefaultHasher::new();
    rel.from_entity.hash(&mut h);
    rel.to_entity.hash(&mut h);
    rel.relation_type.hash(&mut h);
    rel.context.hash(&mut h);
    h.finish()
}

/// Entity row for the batched upsert.
#[derive(Serialize, SurrealValue)]
struct KeyedEntity {
    key: String,
    record: EntityRecord,
}

/// Relation row for the batched upsert.
#[derive(Serialize, SurrealValue)]
struct RelationRow {
    table: String,
    key: String,
    from_key: String,
    to_key: String,
    context: String,
}

/// Workspace membership row for the batched upsert.
#[derive(Serialize, SurrealValue)]
struct KeyedMembership {
    key: String,
    record: WorkspaceSessionRecord,
}

/// Session listing row.
#[derive(Deserialize, SurrealValue)]
struct SessionSummaryRow {
    session_id: String,
    #[serde(default, deserialize_with = "deserialize_surreal_option")]
    name: Option<String>,
    #[serde(default, deserialize_with = "deserialize_surreal_option")]
    description: Option<String>,
    created_at: String,
    last_updated: String,
}

#[derive(Deserialize, SurrealValue)]
struct UpdateCountRow {
    session_id: String,
    n: i64,
}

fn update_record(session_id: Uuid, update: &ContextUpdate) -> Result<ContextUpdateRecord> {
    Ok(ContextUpdateRecord {
        update_id: update.id.to_string(),
        session_id: session_id.to_string(),
        timestamp: update.timestamp.to_rfc3339(),
        update_type: format!("{:?}", update.update_type),
        update_data: serde_json::to_value(update)?,
    })
}

fn parse_rfc3339(s: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(s)
        .map(|dt| dt.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now())
}

/// Statement indices of the persist query.
const STMT_SESSION: usize = 0;
const STMT_UPDATES: usize = 1;
const STMT_REMOVED: usize = 2;
const STMT_ENTITIES: usize = 3;
const STMT_RELATIONS: usize = 4;

const PERSIST_QUERY: &str = "\
    UPSERT $session_rid CONTENT $session_row; \
    FOR $u IN $updates { UPSERT type::record('context_update', $u.update_id) CONTENT $u; }; \
    FOR $id IN $removed { DELETE type::record('context_update', $id); }; \
    FOR $e IN $entities { UPSERT type::record('entity', $e.key) CONTENT $e.record; }; \
    FOR $r IN $relations { \
        UPSERT type::record($r.table, $r.key) SET \
            in = type::record('entity', $r.from_key), \
            out = type::record('entity', $r.to_key), \
            context = $r.context, session_id = $session_id; \
    };";

impl SurrealDBStorage {
    fn persisted_slot(&self, session_id: Uuid) -> Arc<tokio::sync::Mutex<Option<PersistedState>>> {
        if self.persisted.len() >= MAX_TRACKED_SESSIONS && !self.persisted.contains_key(&session_id)
        {
            // Drop one other session's knowledge, not everyone's.
            let victim = self.persisted.iter().map(|e| *e.key()).next();
            if let Some(victim) = victim {
                self.persisted.remove(&victim);
            }
        }
        self.persisted.entry(session_id).or_default().clone()
    }

    /// Record that an entity was written outside a session save.
    pub(super) async fn note_entity_written(&self, session_id: Uuid, entity: &EntityData) {
        if let Some(slot) = self.persisted.get(&session_id).map(|s| s.clone())
            && let Some(state) = slot.lock().await.as_mut()
        {
            state
                .entities
                .insert(entity.name.clone(), entity_fingerprint(entity));
        }
    }

    /// Record that a relationship was written outside a session save.
    pub(super) async fn note_relationship_written(
        &self,
        session_id: Uuid,
        rel: &EntityRelationship,
    ) {
        if let Some(slot) = self.persisted.get(&session_id).map(|s| s.clone())
            && let Some(state) = slot.lock().await.as_mut()
        {
            let key = Self::relation_id(
                session_id,
                &rel.from_entity,
                &rel.to_entity,
                &rel.relation_type,
            );
            state.relations.insert(key, relation_fingerprint(rel));
        }
    }

    /// Forget what is known about a session's stored graph, so the next
    /// save writes all of it.
    pub(super) async fn forget_graph_state(&self, session_id: Uuid) {
        if let Some(slot) = self.persisted.get(&session_id).map(|s| s.clone())
            && let Some(state) = slot.lock().await.as_mut()
        {
            state.entities.clear();
            state.relations.clear();
        }
    }

    /// Ids of the session's stored update rows.
    async fn stored_update_ids(&self, session_id: Uuid) -> Result<HashSet<Uuid>> {
        let mut response = self
            .db
            .query("SELECT VALUE update_id FROM context_update WHERE session_id = $session_id")
            .bind(("session_id", session_id.to_string()))
            .await?;
        let ids: Vec<String> = response.take(0)?;
        Ok(ids.iter().filter_map(|s| Uuid::parse_str(s).ok()).collect())
    }

    /// Write `session` (metadata row), the `explicit` updates, session
    /// updates without a row, and changed entities/edges (all of them when
    /// `full_graph`); delete the rows of the `removed` updates. Rows are
    /// never deleted because a snapshot lacks them: it may predate them.
    async fn persist(
        &self,
        session: &ActiveSession,
        explicit: Vec<ContextUpdate>,
        removed_ids: Vec<Uuid>,
        full_graph: bool,
    ) -> Result<()> {
        let session_id = session.id();
        let slot = self.persisted_slot(session_id);
        let mut guard = slot.lock().await;

        // Borrowed, not taken: a failed save keeps what was known before.
        let fresh;
        let (known_ids, known_entities, known_relations) = match guard.as_ref() {
            Some(state) => (&state.update_ids, &state.entities, &state.relations),
            None => {
                fresh = PersistedState {
                    update_ids: self.stored_update_ids(session_id).await?,
                    ..Default::default()
                };
                (&fresh.update_ids, &fresh.entities, &fresh.relations)
            }
        };

        // Updates.
        let removed_set: HashSet<Uuid> = removed_ids.iter().copied().collect();
        let explicit_ids: HashSet<Uuid> = explicit.iter().map(|u| u.id).collect();
        let mut updates = Vec::with_capacity(explicit.len());
        let mut written_ids = Vec::with_capacity(explicit.len());
        for update in explicit.iter().filter(|u| !removed_set.contains(&u.id)) {
            updates.push(update_record(session_id, update)?);
            written_ids.push(update.id);
        }
        for update in session.incremental_updates.iter() {
            if !known_ids.contains(&update.id)
                && !explicit_ids.contains(&update.id)
                && !removed_set.contains(&update.id)
            {
                updates.push(update_record(session_id, update)?);
                written_ids.push(update.id);
            }
        }
        let removed: Vec<String> = removed_ids.iter().map(Uuid::to_string).collect();

        // Graph.
        let graph = &session.entity_graph;
        let mut entity_fps = HashMap::with_capacity(graph.entities.len());
        let mut entities = Vec::new();
        for (name, entity) in &graph.entities {
            let fp = entity_fingerprint(entity);
            if full_graph || known_entities.get(name) != Some(&fp) {
                entities.push(KeyedEntity {
                    key: Self::entity_id(session_id, name),
                    record: Self::entity_record(session_id, entity),
                });
            }
            entity_fps.insert(name.clone(), fp);
        }
        let mut relation_fps = HashMap::new();
        let mut relations = Vec::new();
        for rel in graph.get_all_relationships() {
            let key = Self::relation_id(
                session_id,
                &rel.from_entity,
                &rel.to_entity,
                &rel.relation_type,
            );
            let fp = relation_fingerprint(&rel);
            if full_graph || known_relations.get(&key) != Some(&fp) {
                relations.push(RelationRow {
                    table: Self::relation_table_name(&rel.relation_type).to_string(),
                    key: key.clone(),
                    from_key: Self::entity_id(session_id, &rel.from_entity),
                    to_key: Self::entity_id(session_id, &rel.to_entity),
                    context: rel.context,
                });
            }
            relation_fps.insert(key, fp);
        }

        let record = SessionRecord {
            session_id: session_id.to_string(),
            name: session.name(),
            description: session.description(),
            created_at: session.created_at().to_rfc3339(),
            last_updated: Utc::now().to_rfc3339(),
            user_preferences: serde_json::to_value(session.user_preferences())?,
            vectorized_update_ids: session
                .vectorized_update_ids
                .iter()
                .map(|id| id.to_string())
                .collect(),
            total_updates: session.incremental_updates.len() as u32,
        };

        let (n_updates, n_removed, n_entities, n_relations) = (
            updates.len(),
            removed.len(),
            entities.len(),
            relations.len(),
        );
        let mut response = self
            .db
            .query(PERSIST_QUERY)
            .bind((
                "session_rid",
                RecordId::new("session", session_id.to_string()),
            ))
            .bind(("session_row", record))
            .bind(("updates", updates))
            .bind(("removed", removed))
            .bind(("entities", entities))
            .bind(("relations", relations))
            .bind(("session_id", session_id.to_string()))
            .await?;

        let errors = response.take_errors();
        for (index, error) in &errors {
            warn!(
                "SurrealDBStorage: save of session {} - statement {} failed: {}",
                session_id, index, error
            );
        }
        if [STMT_SESSION, STMT_UPDATES, STMT_REMOVED]
            .iter()
            .any(|i| errors.contains_key(i))
        {
            // Rows of this save may be missing: forget the session's state
            // so the next save re-reads which rows exist.
            *guard = None;
            return Err(anyhow::anyhow!(
                "Failed to save session {}: {} statement(s) failed",
                session_id,
                errors.len()
            ));
        }
        let graph_ok =
            !errors.contains_key(&STMT_ENTITIES) && !errors.contains_key(&STMT_RELATIONS);
        let mut update_ids = known_ids.clone();
        update_ids.extend(written_ids);
        for id in &removed_set {
            update_ids.remove(id);
        }
        *guard = Some(PersistedState {
            update_ids,
            entities: if graph_ok { entity_fps } else { HashMap::new() },
            relations: if graph_ok {
                relation_fps
            } else {
                HashMap::new()
            },
        });

        debug!(
            "SurrealDBStorage: Saved session {} - {} update rows written, {} removed, {} entities, {} edges",
            session_id, n_updates, n_removed, n_entities, n_relations
        );
        Ok(())
    }

    /// Remember what `load_session` returned as the stored state.
    async fn remember_loaded(&self, session: &ActiveSession) {
        let graph: &SimpleEntityGraph = &session.entity_graph;
        let session_id = session.id();
        let state = PersistedState {
            update_ids: session.incremental_updates.iter().map(|u| u.id).collect(),
            entities: graph
                .entities
                .iter()
                .map(|(name, e)| (name.clone(), entity_fingerprint(e)))
                .collect(),
            relations: graph
                .get_all_relationships()
                .iter()
                .map(|r| {
                    (
                        Self::relation_id(
                            session_id,
                            &r.from_entity,
                            &r.to_entity,
                            &r.relation_type,
                        ),
                        relation_fingerprint(r),
                    )
                })
                .collect(),
        };
        *self.persisted_slot(session_id).lock().await = Some(state);
    }
}

#[async_trait]
impl Storage for SurrealDBStorage {
    /// Full save: metadata, every update without a row, and the whole graph.
    async fn save_session(&self, session: &ActiveSession) -> Result<()> {
        self.persist(session, Vec::new(), Vec::new(), true).await
    }

    /// Incremental save: metadata, `updates`, any other update without a
    /// row, and only the entities/edges that changed since the last
    /// load/save in this process.
    async fn save_session_with_updates(
        &self,
        session: &ActiveSession,
        session_id: Uuid,
        updates: Vec<ContextUpdate>,
    ) -> Result<()> {
        if session.id() != session_id {
            return Err(anyhow::anyhow!(
                "Session id mismatch: blob {} vs updates for {}",
                session.id(),
                session_id
            ));
        }
        self.persist(session, updates, Vec::new(), false).await
    }

    /// Incremental save that also deletes the `removed` updates' rows.
    async fn save_session_with_changes(
        &self,
        session: &ActiveSession,
        session_id: Uuid,
        updates: Vec<ContextUpdate>,
        removed: Vec<Uuid>,
    ) -> Result<()> {
        if session.id() != session_id {
            return Err(anyhow::anyhow!(
                "Session id mismatch: blob {} vs updates for {}",
                session.id(),
                session_id
            ));
        }
        self.persist(session, updates, removed, false).await
    }

    async fn load_session(&self, session_id: Uuid) -> Result<ActiveSession> {
        debug!(
            "SurrealDBStorage: Loading session with ID: {} (normalized)",
            session_id
        );

        // Load session metadata
        let record = self.select_session_record(session_id).await?;

        let r = record.ok_or_else(|| session_not_found(session_id))?;

        // Load ALL context updates from normalized table (source of truth!)
        let all_updates = self.load_session_updates(session_id).await?;

        // Hot context = last 50 updates (same as HotContext::MAX_SIZE)
        let hot_context: Vec<ContextUpdate> =
            all_updates.iter().rev().take(50).rev().cloned().collect();

        // Load entities from native graph table. The graph is derived data:
        // a failure here degrades the session instead of hiding it.
        let entities = self.list_entities(session_id).await.unwrap_or_else(|e| {
            warn!(
                "SurrealDBStorage: Loading entities of session {} failed: {:#}",
                session_id, e
            );
            Vec::new()
        });
        let entity_count = entities.len();
        let mut entity_graph = SimpleEntityGraph::new();
        // As stored: re-adding them as mentions would reset mention_count,
        // importance_score and first_mentioned.
        for entity in entities {
            entity_graph.insert_entity(entity);
        }

        // Load relationships from native graph and add to entity_graph
        let relationships = self
            .load_all_relationships(session_id)
            .await
            .unwrap_or_else(|e| {
                warn!(
                    "SurrealDBStorage: Loading relationships of session {} failed: {:#}",
                    session_id, e
                );
                Vec::new()
            });
        for rel in relationships {
            entity_graph.add_relationship(rel);
        }
        debug!(
            "SurrealDBStorage: Loaded {} entities, graph rebuilt",
            entity_count
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
            parse_rfc3339(&r.created_at),
            parse_rfc3339(&r.last_updated),
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
        self.remember_loaded(&session).await;

        debug!(
            "SurrealDBStorage: Session loaded (normalized) - {} updates, {} entities",
            session.incremental_updates.len(),
            entity_count
        );
        Ok(session)
    }

    async fn delete_session(&self, session_id: Uuid) -> Result<()> {
        debug!("SurrealDBStorage: Deleting session with ID: {}", session_id);

        // Raw DELETE: a typed delete deserializes the removed row, and rows
        // with NULL name/description (see select_session_record) cannot be.
        let query = format!(
            "DELETE type::record('session', $id); \
             DELETE context_update WHERE session_id = $session_id; \
             DELETE entity WHERE session_id = $session_id; \
             DELETE embedding WHERE session_id = $session_id; \
             DELETE message WHERE session_id = $session_id; \
             DELETE type::record('message_next', $session_id); \
             DELETE workspace_session WHERE session_id = $session_id; {}",
            per_relation_table("DELETE {table} WHERE session_id = $session_id;")
        );
        self.db
            .query(query)
            .bind(("id", session_id.to_string()))
            .bind(("session_id", session_id.to_string()))
            .await?
            .check()?;
        self.persisted.remove(&session_id);

        debug!("SurrealDBStorage: Session deleted successfully");
        Ok(())
    }

    async fn clear_session_entities(&self, session_id: Uuid) -> Result<()> {
        let query = format!(
            "DELETE entity WHERE session_id = $session_id; {}",
            per_relation_table("DELETE {table} WHERE session_id = $session_id;")
        );
        self.db
            .query(query)
            .bind(("session_id", session_id.to_string()))
            .await?
            .check()?;
        self.forget_graph_state(session_id).await;

        debug!(
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

    /// Session rows plus per-session update counts, in one query.
    async fn list_session_summaries(&self) -> Result<Vec<SessionSummary>> {
        let mut response = self
            .db
            .query(
                "SELECT session_id, (name ?? NONE) AS name, \
                 (description ?? NONE) AS description, created_at, last_updated \
                 FROM session; \
                 SELECT session_id, count() AS n FROM context_update GROUP BY session_id;",
            )
            .await?;
        let rows: Vec<SessionSummaryRow> = response.take(0)?;
        let counts: Vec<UpdateCountRow> = response.take(1)?;
        let counts: HashMap<String, i64> =
            counts.into_iter().map(|c| (c.session_id, c.n)).collect();

        Ok(rows
            .into_iter()
            .filter_map(|row| {
                let id = Uuid::parse_str(&row.session_id).ok()?;
                Some(SessionSummary {
                    id,
                    update_count: counts.get(&row.session_id).copied().unwrap_or(0).max(0) as usize,
                    name: row.name,
                    description: row.description,
                    created_at: parse_rfc3339(&row.created_at),
                    last_updated: parse_rfc3339(&row.last_updated),
                })
            })
            .collect())
    }

    async fn session_exists(&self, session_id: Uuid) -> Result<bool> {
        Ok(self.select_session_record(session_id).await?.is_some())
    }

    async fn batch_save_updates(
        &self,
        session_id: Uuid,
        updates: Vec<ContextUpdate>,
    ) -> Result<()> {
        if updates.is_empty() {
            return Ok(());
        }

        let records = updates
            .iter()
            .map(|u| update_record(session_id, u))
            .collect::<Result<Vec<_>>>()?;
        let count = records.len();
        self.db
            .query(
                "FOR $u IN $updates { \
                 UPSERT type::record('context_update', $u.update_id) CONTENT $u; };",
            )
            .bind(("updates", records))
            .await?
            .check()?;

        debug!(
            "SurrealDBStorage: Batch saved {} updates for session {} in 1 query",
            count, session_id
        );
        Ok(())
    }

    async fn load_session_updates(&self, session_id: Uuid) -> Result<Vec<ContextUpdate>> {
        debug!(
            "SurrealDBStorage: Loading updates for session {}",
            session_id
        );

        let mut updates = Vec::new();
        let mut start = 0;
        let limit = 1000;

        loop {
            // `update_id` breaks timestamp ties so pages are stable.
            let mut response = self
                .db
                .query(
                    "SELECT * FROM context_update WHERE session_id = $session_id \
                     ORDER BY timestamp ASC, update_id ASC LIMIT $limit START $start",
                )
                .bind(("session_id", session_id.to_string()))
                .bind(("limit", limit))
                .bind(("start", start))
                .await?;

            let records: Vec<ContextUpdateRecord> = response.take(0)?;
            let count = records.len();

            // Parse update_data JSON back to ContextUpdate
            for r in records {
                match serde_json::from_value(r.update_data) {
                    Ok(update) => updates.push(update),
                    Err(e) => warn!(
                        "SurrealDBStorage: Skipping undecodable update {} of session {}: {}",
                        r.update_id, session_id, e
                    ),
                }
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
                    created_at: parse_rfc3339(&r.created_at),
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
                    created_at: parse_rfc3339(&r.created_at),
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

        debug!("SurrealDBStorage: Listed {} checkpoints", checkpoints.len());

        Ok(checkpoints)
    }

    /// Workspace row and all memberships in one query.
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

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let record = WorkspaceRecord {
            workspace_id: workspace_id.to_string(),
            name: name.to_string(),
            description: description.to_string(),
            created_at: now,
        };
        let members: Vec<KeyedMembership> = session_ids
            .iter()
            .map(|session_id| KeyedMembership {
                key: format!("{}_{}", workspace_id, session_id),
                record: WorkspaceSessionRecord {
                    workspace_id: workspace_id.to_string(),
                    session_id: session_id.to_string(),
                    role: format!("{:?}", SessionRole::Primary),
                    added_at: now,
                },
            })
            .collect();

        self.db
            .query(
                "UPSERT type::record('workspace', $workspace_id) CONTENT $record; \
                 FOR $m IN $members { \
                 UPSERT type::record('workspace_session', $m.key) CONTENT $m.record; };",
            )
            .bind(("workspace_id", workspace_id.to_string()))
            .bind(("record", record))
            .bind(("members", members))
            .await?
            .check()?;

        debug!("SurrealDBStorage: Workspace saved successfully");

        Ok(())
    }

    async fn delete_workspace(&self, workspace_id: Uuid) -> Result<()> {
        debug!("SurrealDBStorage: Deleting workspace {}", workspace_id);

        self.db
            .query(
                "DELETE type::record('workspace', $workspace_id); \
                 DELETE workspace_session WHERE workspace_id = $workspace_id; \
                 DELETE card WHERE scope = $card_scope;",
            )
            .bind(("workspace_id", workspace_id.to_string()))
            .bind((
                "card_scope",
                post_cortex_core::core::card::CardScope::Workspace(workspace_id).key(),
            ))
            .await?
            .check()?;

        debug!("SurrealDBStorage: Workspace deleted successfully");

        Ok(())
    }

    /// Workspaces and memberships in one query.
    async fn list_workspaces(&self) -> Result<Vec<StoredWorkspace>> {
        debug!("SurrealDBStorage: Listing workspaces");

        let mut response = self
            .db
            .query("SELECT * FROM workspace; SELECT * FROM workspace_session;")
            .await?;
        let records: Vec<WorkspaceRecord> = response.take(0)?;
        let memberships: Vec<WorkspaceSessionRecord> = response.take(1)?;

        let mut sessions_by_workspace: HashMap<String, Vec<(Uuid, SessionRole)>> = HashMap::new();
        for m in memberships {
            let Ok(session_id) = Uuid::parse_str(&m.session_id) else {
                continue;
            };
            let role = match m.role.as_str() {
                "Primary" => SessionRole::Primary,
                "Related" => SessionRole::Related,
                "Dependency" => SessionRole::Dependency,
                "Shared" => SessionRole::Shared,
                _ => SessionRole::Primary,
            };
            sessions_by_workspace
                .entry(m.workspace_id)
                .or_default()
                .push((session_id, role));
        }

        let workspaces: Vec<StoredWorkspace> = records
            .into_iter()
            .filter_map(|record| {
                let id = Uuid::parse_str(&record.workspace_id).ok()?;
                Some(StoredWorkspace {
                    id,
                    sessions: sessions_by_workspace
                        .remove(&record.workspace_id)
                        .unwrap_or_default(),
                    name: record.name,
                    description: record.description,
                    created_at: record.created_at,
                })
            })
            .collect();

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
