// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! A session's entity graph as rows (data version 3): one row per entity
//! (`session:{id}:ent:{name}`) and one per edge
//! (`session:{id}:edge:{from}\0{to}\0{type}`). A save writes the rows of
//! what its records changed (see `SimpleEntityGraph::changed_by`), or all
//! of them when the whole graph changed; a load reads them back. Record
//! links (mentions) are not stored: the load derives them from the records.

use crate::port_error::Result;
use chrono::{DateTime, Utc};
use rocksdb::{DB, SnapshotWithThreadMode, WriteBatch};
use uuid::Uuid;

use post_cortex_core::core::context_update::{ContextUpdate, EntityData, EntityRelationship};
use post_cortex_core::graph::entity_graph::SimpleEntityGraph;

use crate::rocksdb_storage::core::{keys_with_prefix, scan_snapshot_prefix};
use crate::rocksdb_storage::rows::{decode_edge, decode_entity, encode_edge, encode_entity};

fn entity_prefix(session_id: Uuid) -> String {
    format!("session:{session_id}:ent:")
}

fn edge_prefix(session_id: Uuid) -> String {
    format!("session:{session_id}:edge:")
}

fn entity_key(session_id: Uuid, name: &str) -> String {
    format!("{}{name}", entity_prefix(session_id))
}

fn edge_key(session_id: Uuid, edge: &EntityRelationship) -> String {
    format!(
        "{}{}\0{}\0{}",
        edge_prefix(session_id),
        edge.from_entity,
        edge.to_entity,
        edge.relation_type.as_str()
    )
}

fn put_entity(batch: &mut WriteBatch, session_id: Uuid, entity: &EntityData) -> Result<()> {
    batch.put(
        entity_key(session_id, &entity.name).as_bytes(),
        encode_entity(entity)?,
    );
    Ok(())
}

fn put_edge(batch: &mut WriteBatch, session_id: Uuid, edge: &EntityRelationship) -> Result<()> {
    batch.put(edge_key(session_id, edge).as_bytes(), encode_edge(edge)?);
    Ok(())
}

/// Queue the rows of the entities and edges `updates` changed in `graph`
/// (the graph they were applied to).
pub fn queue_changed<'a>(
    batch: &mut WriteBatch,
    session_id: Uuid,
    graph: &SimpleEntityGraph,
    updates: impl IntoIterator<Item = &'a ContextUpdate>,
) -> Result<()> {
    for update in updates {
        let (entities, edges) = graph.changed_by(update);
        for entity in entities {
            put_entity(batch, session_id, entity)?;
        }
        for edge in &edges {
            put_edge(batch, session_id, edge)?;
        }
    }
    Ok(())
}

/// Queue `graph` in place of the session's stored graph: its stored rows
/// are deleted and every entity and edge is written.
pub fn queue_whole(
    db: &DB,
    batch: &mut WriteBatch,
    session_id: Uuid,
    graph: &SimpleEntityGraph,
) -> Result<()> {
    for prefix in [entity_prefix(session_id), edge_prefix(session_id)] {
        for key in keys_with_prefix(db, prefix.as_bytes())? {
            batch.delete(&key);
        }
    }
    queue_all(batch, session_id, graph)
}

/// Queue every entity and edge of `graph` (a session without stored graph
/// rows: new, replaced or migrated).
pub fn queue_all(
    batch: &mut WriteBatch,
    session_id: Uuid,
    graph: &SimpleEntityGraph,
) -> Result<()> {
    for entity in graph.entities.values_unordered() {
        put_entity(batch, session_id, entity)?;
    }
    for edge in graph.get_all_relationships() {
        put_edge(batch, session_id, &edge)?;
    }
    Ok(())
}

/// The stored graph of `session_id`, read from `snapshot`; edges get
/// `at` as the time of an endpoint they name that has no row.
pub fn read_graph(
    snapshot: &SnapshotWithThreadMode<'_, DB>,
    session_id: Uuid,
    at: DateTime<Utc>,
) -> Result<SimpleEntityGraph> {
    let mut graph = SimpleEntityGraph::new();
    let entities = entity_prefix(session_id);
    for item in scan_snapshot_prefix(snapshot, entities.as_bytes()) {
        let (key, row) = item?;
        graph.insert_entity(decode_entity(&String::from_utf8_lossy(&key), &row)?);
    }
    let edges = edge_prefix(session_id);
    for item in scan_snapshot_prefix(snapshot, edges.as_bytes()) {
        let (key, row) = item?;
        graph.add_relationship(decode_edge(&String::from_utf8_lossy(&key), &row)?, at);
    }
    Ok(graph)
}
