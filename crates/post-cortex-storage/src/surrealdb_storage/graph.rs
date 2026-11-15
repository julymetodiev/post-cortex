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

//! The graph rows of a session: the entity record a save writes, and the
//! loaders the session reader rebuilds a session's graph with.
//!
//! Entity names are user data, so they are always bound as parameters,
//! never spliced into SurrealQL.

use crate::port_error::Result;
use serde::Deserialize;
use surrealdb::types::SurrealValue;
use tracing::debug;
use uuid::Uuid;

use crate::surrealdb_storage::ids::per_relation_table;
use crate::surrealdb_storage::records::EntityRecord;
use post_cortex_core::core::context_update::{EntityData, EntityRelationship, RelationType};
use surrealdb::Surreal;
use surrealdb::engine::any::Any;

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

/// Persisted form of an entity.
pub fn entity_record(session_id: Uuid, entity: &EntityData) -> EntityRecord {
    EntityRecord {
        session_id: session_id.to_string(),
        name: entity.name.clone(),
        entity_type: entity.entity_type.stored_name().to_string(),
        first_mentioned: entity.first_mentioned.to_rfc3339(),
        last_mentioned: entity.last_mentioned.to_rfc3339(),
        mention_count: entity.mention_count,
        importance_score: entity.importance_score,
        description: entity.description.clone(),
        aliases: (!entity.aliases.is_empty()).then(|| entity.aliases.clone()),
    }
}

/// The stored entity; an unknown type or an unparsable timestamp is
/// [`Corrupt`](post_cortex_ports::PortError::Corrupt).
fn entity_from_record(r: EntityRecord) -> Result<EntityData> {
    use post_cortex_ports::error::{parse_name, parse_timestamp};
    const KIND: &str = "entity";
    Ok(EntityData {
        entity_type: parse_name(KIND, &r.name, &r.entity_type)?,
        first_mentioned: parse_timestamp(KIND, &r.name, &r.first_mentioned)?,
        last_mentioned: parse_timestamp(KIND, &r.name, &r.last_mentioned)?,
        aliases: r.aliases.unwrap_or_default(),
        name: r.name,
        mention_count: r.mention_count,
        importance_score: r.importance_score,
        description: r.description,
    })
}

/// Every entity of `session_id`, by importance (the adapter rebuilds
/// session graphs from these).
pub async fn load_entities(db: &Surreal<Any>, session_id: Uuid) -> Result<Vec<EntityData>> {
    let mut all_entities = Vec::new();
    let mut start = 0;
    let limit = 1000;

    loop {
        // `name` breaks importance ties so pages never overlap or skip.
        let mut response = db
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
        for record in records {
            all_entities.push(entity_from_record(record)?);
        }

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

/// Load all relationships for a session from the relation tables, in
/// one round trip (one statement per table).
pub async fn load_all_relationships(
    db: &Surreal<Any>,
    session_id: Uuid,
) -> Result<Vec<EntityRelationship>> {
    let query = per_relation_table(
        "SELECT in.name AS from_entity, out.name AS to_entity, context \
         FROM {table} WHERE session_id = $session_id;",
    );
    let mut response = db
        .query(query)
        .bind(("session_id", session_id.to_string()))
        .await?;

    let mut all_relationships = Vec::new();
    for (index, rel_type) in RelationType::ALL.iter().enumerate() {
        let records: Vec<RelRecord> = response.take(index)?;
        all_relationships.extend(records.into_iter().filter_map(|r| {
            Some(EntityRelationship {
                from_entity: r.from_entity?,
                to_entity: r.to_entity?,
                relation_type: *rel_type,
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
