// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Session writes: what a save's [`SessionChanges`] name, in one
//! transaction (the session's metadata, its changed record rows and
//! removals, the graph rows its records changed or the whole graph, and the
//! vectorized ids added and removed). The adapter keeps no state about what
//! is stored: the changes say what to write.

use crate::port_error::Result;
use crate::surrealdb_storage::graph::entity_record;
use crate::surrealdb_storage::ids;
use crate::surrealdb_storage::records::SessionHeadRecord;
use post_cortex_core::core::context_update::{EntityData, EntityRelationship};
use post_cortex_core::session::active_session::ActiveSession;
use post_cortex_ports::storage::SessionChanges;
use std::collections::HashSet;
use surrealdb::Surreal;
use surrealdb::engine::any::Any;
use surrealdb::types::RecordId;
use tracing::debug;
use uuid::Uuid;

mod rows;

use rows::{KeyedEntity, PERSIST_QUERY, RelationRow, prune_sweep, replace_prefix};
pub use rows::{SaveMode, update_record};

fn keyed_entity(session_id: Uuid, entity: &EntityData) -> KeyedEntity {
    KeyedEntity {
        key: ids::entity_id(session_id, &entity.name),
        record: entity_record(session_id, entity),
    }
}

fn relation_row(session_id: Uuid, rel: EntityRelationship) -> RelationRow {
    RelationRow {
        table: rel.relation_type.as_str().to_string(),
        key: ids::relation_id(
            session_id,
            &rel.from_entity,
            &rel.to_entity,
            &rel.relation_type,
        ),
        from_key: ids::entity_id(session_id, &rel.from_entity),
        to_key: ids::entity_id(session_id, &rel.to_entity),
        context: rel.context,
    }
}

/// Write `session` with `changes` (see the module docs). With
/// [`SaveMode::Replace`] the session's records, graph, vectors and messages
/// are deleted first, in the same transaction.
pub async fn persist(
    db: &Surreal<Any>,
    session: &ActiveSession,
    changes: &SessionChanges,
    mode: SaveMode,
) -> Result<()> {
    let session_id = session.id();
    let removed_set: HashSet<Uuid> = changes.removed.iter().copied().collect();
    let written: Vec<_> = changes
        .updates
        .iter()
        .filter(|u| !removed_set.contains(&u.id))
        .collect();
    let updates = written
        .iter()
        .map(|u| update_record(session_id, u))
        .collect::<Result<Vec<_>>>()?;
    let removed: Vec<String> = changes.removed.iter().map(Uuid::to_string).collect();

    // Graph rows: all of them when the whole graph changed, else those the
    // written records changed.
    let graph = session.entity_graph();
    let whole = mode == SaveMode::Replace || changes.whole_graph;
    let (entities, relations): (Vec<KeyedEntity>, Vec<RelationRow>) = if whole {
        (
            graph
                .entities
                .values_unordered()
                .map(|e| keyed_entity(session_id, e))
                .collect(),
            graph
                .get_all_relationships()
                .into_iter()
                .map(|r| relation_row(session_id, r))
                .collect(),
        )
    } else {
        let mut entities = Vec::new();
        let mut relations = Vec::new();
        for update in &written {
            let (changed, edges) = graph.changed_by(update);
            entities.extend(changed.into_iter().map(|e| keyed_entity(session_id, e)));
            relations.extend(edges.into_iter().map(|r| relation_row(session_id, r)));
        }
        (entities, relations)
    };
    let entity_keys: Vec<String> = entities.iter().map(|e| e.key.clone()).collect();
    let relation_keys: Vec<String> = relations.iter().map(|r| r.key.clone()).collect();
    let prune = match mode {
        SaveMode::Incremental if changes.whole_graph => prune_sweep(),
        _ => String::new(),
    };
    let prefix = match mode {
        SaveMode::Replace => replace_prefix(),
        SaveMode::Incremental => String::new(),
    };

    let head = SessionHeadRecord::of(session)?;
    let (n_updates, n_removed, n_entities, n_relations) = (
        updates.len(),
        removed.len(),
        entities.len(),
        relations.len(),
    );
    db.query(format!(
        "BEGIN TRANSACTION; {prefix} {PERSIST_QUERY} {prune} COMMIT TRANSACTION;"
    ))
    .bind((
        "session_rid",
        RecordId::new("session", session_id.to_string()),
    ))
    .bind(("session_head", head))
    .bind(("updates", updates))
    .bind(("removed", removed))
    .bind(("entities", entities))
    .bind(("relations", relations))
    .bind(("entity_keys", entity_keys))
    .bind(("relation_keys", relation_keys))
    .bind((
        "vectorized",
        changes
            .vectorized
            .iter()
            .map(Uuid::to_string)
            .collect::<Vec<_>>(),
    ))
    .bind((
        "unvectorized",
        changes
            .unvectorized
            .iter()
            .map(Uuid::to_string)
            .collect::<Vec<_>>(),
    ))
    .bind(("session_id", session_id.to_string()))
    .await?
    .check()?;

    debug!(
        "SurrealDBStorage: Saved session {} - {} update rows written, {} removed, {} entities, {} edges",
        session_id, n_updates, n_removed, n_entities, n_relations
    );
    Ok(())
}
