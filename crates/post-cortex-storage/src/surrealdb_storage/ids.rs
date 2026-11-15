// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Record ids and per-relation-table statements, shared by the graph, the
//! session writer and the migrations.

use post_cortex_core::core::context_update::RelationType;
use uuid::Uuid;

/// Key of an entity node: `{session}_{name}`.
pub fn entity_id(session_id: Uuid, name: &str) -> String {
    format!("{session_id}_{name}")
}

/// Record id of an edge, version 1 of the edge identity: built from the
/// wire names only (no hasher, so it is the same on every toolchain),
/// with the source name length-prefixed so no two edges share an id:
/// `{session}_v1_{relation}_{len(from)}:{from}_{to}`.
/// [`crate::surrealdb_storage::migrate`] re-keys edges stored under older ids.
pub fn relation_id(
    session_id: Uuid,
    from_entity: &str,
    to_entity: &str,
    rel_type: &RelationType,
) -> String {
    format!(
        "{session_id}_v1_{}_{}:{from_entity}_{to_entity}",
        rel_type.as_str(),
        from_entity.len()
    )
}

/// One SurrealQL statement per relation (edge) table, in
/// [`RelationType::ALL`] order, built from `template` with `{table}`
/// replaced by the table name (the relation's wire name). Lets callers
/// touch all eight tables in a single round trip.
pub fn per_relation_table(template: &str) -> String {
    RelationType::ALL
        .iter()
        .map(|rel| template.replace("{table}", rel.as_str()))
        .collect::<Vec<_>>()
        .join(" ")
}
