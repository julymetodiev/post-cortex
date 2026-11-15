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

//! The [`SessionStore`](post_cortex_ports::storage::SessionStore) port on [`SurrealDBStorage`].
//!
//! The `context_update` table is the source of truth for a session's
//! updates; the `session` row holds metadata and the vectorized ids. A save
//! sends what its `SessionChanges` name, in one transaction with bound
//! parameters (see [`crate::surrealdb_storage::persist`]).

use crate::port_error::PortResult;
use crate::port_error::Result;
use crate::surrealdb_storage::persist::{SaveMode, persist};
use async_trait::async_trait;
use serde::Deserialize;
use std::collections::HashMap;
use surrealdb::types::SurrealValue;
use tracing::debug;
use uuid::Uuid;

use post_cortex_core::core::context_update::ContextUpdate;
use post_cortex_core::graph::entity_graph::SimpleEntityGraph;
use post_cortex_core::session::active_session::{ActiveSession, StoredSession, UserPreferences};
use post_cortex_core::session::session_components::SessionMetadata;
use post_cortex_ports::error::{parse_timestamp, parse_uuid, session_not_found};
use post_cortex_ports::storage::{SessionChanges, SessionStore, SessionSummary};

use crate::surrealdb_storage::SurrealDBStorage;
use crate::surrealdb_storage::ids::per_relation_table;
use crate::surrealdb_storage::records::{ContextUpdateRecord, deserialize_surreal_option};
use surrealdb::Surreal;
use surrealdb::engine::any::Any;

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

#[async_trait]
impl SessionStore for SurrealDBStorage {
    /// Save `session` with `changes`, in one transaction.
    async fn save_session(
        &self,
        session: &ActiveSession,
        changes: SessionChanges,
    ) -> PortResult<()> {
        self.breaker
            .guard(async { persist(&self.db(), session, &changes, SaveMode::Incremental).await })
            .await
    }

    async fn load_session(&self, session_id: Uuid) -> PortResult<StoredSession> {
        self.breaker
            .guard(async {
                debug!(
                    "SurrealDBStorage: Loading session with ID: {} (normalized)",
                    session_id
                );

                // Load session metadata
                let record = select_session_record(&self.db(), session_id).await?;

                let r = record.ok_or_else(|| session_not_found(session_id))?;

                // Load ALL context updates from normalized table (source of truth!)
                let all_updates = load_session_updates(&self.db(), session_id).await?;

                // The graph is stored state: a failed read fails the load (an empty
                // graph recorded as persisted would let the next save lose it).
                let entities =
                    crate::surrealdb_storage::graph::load_entities(&self.db(), session_id).await?;
                let entity_count = entities.len();
                let mut entity_graph = SimpleEntityGraph::new();
                // As stored: re-adding them as mentions would reset mention_count,
                // importance_score and first_mentioned.
                for entity in entities {
                    entity_graph.insert_entity(entity);
                }

                // Load relationships from native graph and add to entity_graph
                let relationships =
                    crate::surrealdb_storage::graph::load_all_relationships(&self.db(), session_id)
                        .await?;
                // Endpoints are normally among the stored entities; one that is
                // missing is created as last seen when the session last changed.
                let last_updated = parse_timestamp("session", session_id, &r.last_updated)?;
                for rel in relationships {
                    entity_graph.add_relationship(rel, last_updated);
                }
                debug!(
                    "SurrealDBStorage: Loaded {} entities, graph rebuilt",
                    entity_count
                );

                // Rows written before preferences were stored hold `{}`, which
                // means the defaults; anything else must parse.
                let user_preferences: UserPreferences = match r.user_preferences {
                    serde_json::Value::Object(ref map) if map.is_empty() => {
                        UserPreferences::default()
                    }
                    value => serde_json::from_value(value).map_err(|e| {
                        post_cortex_ports::error::corrupt(
                            "session",
                            session_id,
                            format!("preferences: {e}"),
                        )
                    })?,
                };

                let vectorized_ids = r
                    .vectorized_update_ids
                    .iter()
                    .map(|id| parse_uuid("session", session_id, id))
                    .collect::<post_cortex_ports::error::Result<Vec<Uuid>>>()?;

                // Records and metadata; every other view derives from the records.
                let metadata = SessionMetadata::new(
                    session_id,
                    r.name,
                    r.description,
                    user_preferences,
                    parse_timestamp("session", session_id, &r.created_at)?,
                );
                let session =
                    ActiveSession::from_records(metadata, last_updated, all_updates, entity_graph);

                debug!(
                    "SurrealDBStorage: Session loaded (normalized) - {} updates, {} entities",
                    session.records().len(),
                    entity_count
                );
                Ok(StoredSession::new(session, vectorized_ids))
            })
            .await
    }

    async fn delete_session(&self, session_id: Uuid) -> PortResult<bool> {
        self.breaker
            .guard(async {
                debug!("SurrealDBStorage: Deleting session with ID: {}", session_id);
                // Deletes of one session are ordered by the session writer,
                // so nothing deletes it between this read and the delete.
                let existed = select_session_record(&self.db(), session_id)
                    .await?
                    .is_some();

                // Raw DELETE: a typed delete deserializes the removed row, and rows
                // with NULL name/description (see select_session_record) cannot be.
                let query = format!(
                    "DELETE type::record('session', $id); \
             DELETE context_update WHERE session_id = $session_id; \
             DELETE entity WHERE session_id = $session_id; \
             DELETE embedding WHERE session_id = $session_id; \
             DELETE message WHERE session_id = $session_id; \
             DELETE type::record('message_next', $session_id); \
             DELETE type::record('message_stream', $session_id); \
             DELETE workspace_session WHERE session_id = $session_id; \
             DELETE checkpoint WHERE session_id = $session_id; {}",
                    per_relation_table("DELETE {table} WHERE session_id = $session_id;")
                );
                self.db()
                    .query(query)
                    .bind(("id", session_id.to_string()))
                    .bind(("session_id", session_id.to_string()))
                    .await?
                    .check()?;

                debug!("SurrealDBStorage: Session deleted successfully");
                Ok(existed)
            })
            .await
    }

    async fn list_sessions(&self) -> PortResult<Vec<Uuid>> {
        self.breaker
            .guard(async {
                debug!("SurrealDBStorage: Listing sessions");

                // Raw query + `response.take(0)` routes through serde (honours
                // `deserialize_surreal_option` on the Option<String> fields).
                // `db.select(table)` goes through SurrealValue conversion which
                // rejects NONE for Option<String> and crashed any list with
                // sessions that had `name = null` in storage.
                let mut response = self.db().query("SELECT session_id FROM session").await?;
                let ids: Vec<String> = response.take("session_id")?;

                let sessions = ids
                    .iter()
                    .map(|id| parse_uuid("session", id, id))
                    .collect::<post_cortex_ports::error::Result<Vec<Uuid>>>()?;

                debug!("SurrealDBStorage: Found {} sessions", sessions.len());

                Ok(sessions)
            })
            .await
    }

    /// Session rows plus per-session update counts, in one query.
    async fn list_session_summaries(&self) -> PortResult<Vec<SessionSummary>> {
        self.breaker
            .guard(async {
                let mut response = self
                    .db()
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

                rows.into_iter()
                    .map(|row| {
                        let id = parse_uuid("session", &row.session_id, &row.session_id)?;
                        // A session without update rows has no group: zero updates.
                        let update_count = match counts.get(&row.session_id) {
                            Some(&n) => usize::try_from(n).map_err(|_| {
                                post_cortex_ports::error::corrupt(
                                    "session",
                                    id,
                                    format!("update count {n}"),
                                )
                            })?,
                            None => 0,
                        };
                        Ok(SessionSummary {
                            id,
                            update_count,
                            name: row.name,
                            description: row.description,
                            created_at: parse_timestamp("session", id, &row.created_at)?,
                            last_updated: parse_timestamp("session", id, &row.last_updated)?,
                        })
                    })
                    .collect()
            })
            .await
    }

    async fn session_exists(&self, session_id: Uuid) -> PortResult<bool> {
        self.breaker
            .guard(async {
                Ok(select_session_record(&self.db(), session_id)
                    .await?
                    .is_some())
            })
            .await
    }

    /// One transaction.
    async fn replace_session(
        &self,
        session: &ActiveSession,
        vectorized: &[Uuid],
    ) -> PortResult<()> {
        self.breaker
            .guard(async {
                persist(
                    &self.db(),
                    session,
                    &SessionChanges::everything(session, vectorized),
                    SaveMode::Replace,
                )
                .await
            })
            .await
    }
}

/// The record rows of a session, in session order (by time; the update id
/// breaks ties, so pages are stable).
pub async fn load_session_updates(
    db: &Surreal<Any>,
    session_id: Uuid,
) -> Result<Vec<ContextUpdate>> {
    debug!(
        "SurrealDBStorage: Loading updates for session {}",
        session_id
    );
    let mut updates = Vec::new();
    let mut start = 0;
    let limit = 1000;
    loop {
        let mut response = db
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
        for r in records {
            let update = serde_json::from_value(r.update_data)
                .map_err(|e| post_cortex_ports::error::corrupt("update row", &r.update_id, e))?;
            updates.push(update);
        }
        if count < limit {
            break;
        }
        start += count;
    }
    debug!("SurrealDBStorage: Loaded {} updates", updates.len());
    Ok(updates)
}
/// Load one session row. `name`/`description` are coalesced to NONE:
/// older clients wrote JSON NULL there, which `SurrealValue` cannot
/// read into `Option<String>` — the whole session then failed to load
/// and surfaced as "Session not found".
async fn select_session_record(
    db: &Surreal<Any>,
    session_id: Uuid,
) -> surrealdb::Result<Option<crate::surrealdb_storage::records::SessionRecord>> {
    let mut res = db
        .query(
            "SELECT *, (name ?? NONE) AS name, (description ?? NONE) AS description \
             FROM type::record('session', $id)",
        )
        .bind(("id", session_id.to_string()))
        .await?;
    let rows: Vec<crate::surrealdb_storage::records::SessionRecord> = res.take(0)?;
    Ok(rows.into_iter().next())
}
