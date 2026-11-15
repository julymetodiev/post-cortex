// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The rows and statements of a session save: keyed rows and the persist
//! query with its replace and prune statements.

use crate::port_error::Result;
use serde::Serialize;
use surrealdb::types::SurrealValue;
use uuid::Uuid;

use post_cortex_core::core::context_update::ContextUpdate;

use crate::surrealdb_storage::records::{ContextUpdateRecord, EntityRecord};

/// Entity row for the batched upsert.
#[derive(Serialize, SurrealValue)]
pub struct KeyedEntity {
    pub key: String,
    pub record: EntityRecord,
}

/// Relation row for the batched upsert.
#[derive(Serialize, SurrealValue)]
pub struct RelationRow {
    pub table: String,
    pub key: String,
    pub from_key: String,
    pub to_key: String,
    pub context: String,
}

pub fn update_record(session_id: Uuid, update: &ContextUpdate) -> Result<ContextUpdateRecord> {
    Ok(ContextUpdateRecord {
        update_id: update.id.to_string(),
        session_id: session_id.to_string(),
        timestamp: update.timestamp.to_rfc3339(),
        update_type: update.update_type.stored_name().to_string(),
        update_data: serde_json::to_value(update)?,
    })
}

/// How [`crate::surrealdb_storage::persist::persist`] writes a session.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SaveMode {
    /// What the changes name.
    Incremental,
    /// The session's records, graph, vectors and messages are deleted
    /// first, in the same transaction; then everything is written.
    Replace,
}

/// Deletes a [`SaveMode::Replace`] runs first: what a replacing import
/// rewrites (checkpoints and workspace memberships stay).
pub fn replace_prefix() -> String {
    format!(
        "DELETE context_update WHERE session_id = $session_id; \
         DELETE entity WHERE session_id = $session_id; \
         DELETE embedding WHERE session_id = $session_id; \
         DELETE message WHERE session_id = $session_id; \
         DELETE type::record('message_next', $session_id); \
         DELETE type::record('message_stream', $session_id); \
         UPDATE $session_rid SET vectorized_update_ids = []; {}",
        crate::surrealdb_storage::ids::per_relation_table(
            "DELETE {table} WHERE session_id = $session_id;"
        )
    )
}

/// The session's metadata, its vectorized ids, record rows, removals and
/// graph rows. Removed records leave the vectorized ids too.
pub const PERSIST_QUERY: &str = "\
    UPSERT $session_rid MERGE $session_head; \
    UPDATE $session_rid SET vectorized_update_ids = array::complement( \
        array::union(vectorized_update_ids ?? [], $vectorized), \
        array::union($unvectorized, $removed)); \
    FOR $u IN $updates { UPSERT type::record('context_update', $u.update_id) CONTENT $u; }; \
    FOR $id IN $removed { DELETE type::record('context_update', $id); }; \
    FOR $e IN $entities { UPSERT type::record('entity', $e.key) CONTENT $e.record; }; \
    FOR $r IN $relations { \
        UPSERT type::record($r.table, $r.key) SET \
            in = type::record('entity', $r.from_key), \
            out = type::record('entity', $r.to_key), \
            context = $r.context, session_id = $session_id; \
    };";

/// Deletes of every graph row of the session the snapshot does not have
/// (a whole-graph save).
pub fn prune_sweep() -> String {
    format!(
        "DELETE entity WHERE session_id = $session_id AND meta::id(id) NOT IN $entity_keys; {}",
        crate::surrealdb_storage::ids::per_relation_table(
            "DELETE {table} WHERE session_id = $session_id AND meta::id(id) NOT IN $relation_keys;"
        )
    )
}
