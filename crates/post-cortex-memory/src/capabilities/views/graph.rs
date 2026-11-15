// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Entity importance, the entity network around a center and session
//! statistics: one operation each for every transport (they were built
//! separately by the MCP tools and gRPC, with different limits and
//! shapes).

use chrono::{DateTime, Utc};
use post_cortex_core::core::context_update::{EntityType, RelationType};
use post_cortex_core::graph::entity_graph::{EntityAnalysis, SimpleEntityGraph};
use post_cortex_core::session::active_session::ActiveSession;
use serde::Serialize;
use uuid::Uuid;

/// Entities analysed when the request gives no limit.
pub const DEFAULT_IMPORTANCE_LIMIT: usize = 100;
/// Entities of a network when the request gives no limit.
pub const DEFAULT_NETWORK_ENTITIES: usize = 50;
/// Relationships of a network when the request gives no limit.
pub const DEFAULT_NETWORK_RELATIONSHIPS: usize = 100;

/// The entities of a session ranked by importance, with paging counts.
#[derive(Debug, Clone, Serialize)]
pub struct EntityImportance {
    /// The top entities, most important first.
    pub entities: Vec<EntityAnalysis>,
    /// How many there are and how many are shown.
    pub pagination: ImportancePage,
}

/// Paging counts of [`EntityImportance`].
#[derive(Debug, Clone, Copy, Serialize)]
pub struct ImportancePage {
    /// Entities in the session.
    pub total_entities: usize,
    /// Entities at or above `min_importance`.
    pub after_filter: usize,
    /// Entities returned.
    pub shown: usize,
    /// More matched than were returned.
    pub truncated: bool,
}

/// The entity network around a center entity.
#[derive(Debug, Clone, Serialize)]
pub struct EntityNetworkView {
    /// The session.
    pub session_id: String,
    /// The center: the one asked for, else the most important entity.
    pub center_entity: Option<String>,
    /// The entities reached, closest first.
    pub entities: Vec<NetworkEntity>,
    /// The relationships between them.
    pub relationships: Vec<NetworkRelationship>,
}

/// An entity of an [`EntityNetworkView`].
#[derive(Debug, Clone, Serialize)]
pub struct NetworkEntity {
    /// Its name.
    pub name: String,
    /// Its type (stored name, `CodeComponent`).
    pub entity_type: EntityType,
    /// Its importance score.
    pub importance_score: f32,
    /// How often it was mentioned.
    pub mention_count: u32,
}

/// A relationship of an [`EntityNetworkView`].
#[derive(Debug, Clone, Serialize)]
pub struct NetworkRelationship {
    /// Source entity.
    pub from: String,
    /// Relation type (stored name, `DependsOn`).
    #[serde(rename = "type")]
    pub relation_type: RelationType,
    /// Target entity.
    pub to: String,
}

/// Size and activity of a session.
#[derive(Debug, Clone, Serialize)]
pub struct SessionStatistics {
    /// The session.
    pub session_id: String,
    /// Its name.
    pub name: Option<String>,
    /// Its description.
    pub description: Option<String>,
    /// Creation time.
    pub created_at: DateTime<Utc>,
    /// Time of the latest change.
    pub last_updated: DateTime<Utc>,
    /// Records in the hot window.
    pub hot_updates: usize,
    /// Records.
    pub total_updates: usize,
    /// Entities of its graph.
    pub entity_count: usize,
    /// Edges of its graph.
    pub relationship_count: usize,
    /// Files with code references.
    pub code_references: usize,
    /// Entries of the change history.
    pub change_history: usize,
    /// Time of the first record.
    pub first_update: Option<DateTime<Utc>>,
    /// Time of the last record.
    pub last_update: Option<DateTime<Utc>>,
    /// Minutes between the first and the last record.
    pub duration_minutes: Option<i64>,
}

/// The entities of `graph` ranked by importance: at most `limit`, those
/// at or above `min_importance`.
pub fn importance(
    graph: &SimpleEntityGraph,
    limit: Option<usize>,
    min_importance: Option<f32>,
) -> EntityImportance {
    let total_entities = graph.entities.len();
    let limit = limit.unwrap_or(DEFAULT_IMPORTANCE_LIMIT);
    // Ranked by importance, so the filter keeps a prefix of the ranking:
    // only the top `limit` are built.
    let mut entities = graph.analyze_entity_importance_top(limit);
    let after_filter = match min_importance {
        Some(min) => {
            entities.retain(|e| e.importance_score >= min);
            graph.count_entities_with_importance_at_least(min)
        }
        None => total_entities,
    };
    let truncated = after_filter > limit;
    EntityImportance {
        entities,
        pagination: ImportancePage {
            total_entities,
            after_filter,
            shown: if truncated { limit } else { after_filter },
            truncated,
        },
    }
}

/// The network two hops around `center` (else the most important entity)
/// of `session_id`'s `graph`, closest first, bounded by the limits.
pub fn network(
    session_id: Uuid,
    graph: &SimpleEntityGraph,
    center: Option<&str>,
    max_entities: Option<usize>,
    max_relationships: Option<usize>,
) -> EntityNetworkView {
    let center_entity = match center {
        Some(name) if !name.is_empty() => Some(name.to_string()),
        _ => graph
            .get_most_important_entities(1)
            .first()
            .map(|e| e.name.clone()),
    };
    // The BFS stops at the limits: only what is shown is visited.
    let network = graph.get_entity_network_bounded(
        center_entity.as_deref().unwrap_or(""),
        2,
        max_entities.unwrap_or(DEFAULT_NETWORK_ENTITIES),
        max_relationships.unwrap_or(DEFAULT_NETWORK_RELATIONSHIPS),
    );
    EntityNetworkView {
        session_id: session_id.to_string(),
        center_entity,
        entities: network
            .entities
            .values()
            .map(|e| NetworkEntity {
                name: e.name.clone(),
                entity_type: e.entity_type,
                importance_score: e.importance_score,
                mention_count: e.mention_count,
            })
            .collect(),
        relationships: network
            .relationships
            .iter()
            .map(|r| NetworkRelationship {
                from: r.from_entity.clone(),
                relation_type: r.relation_type,
                to: r.to_entity.clone(),
            })
            .collect(),
    }
}

/// Size and activity of a session.
pub fn statistics(session_id: Uuid, session: &ActiveSession) -> SessionStatistics {
    let records = session.records();
    let first = records.iter().map(|u| u.timestamp).min();
    let last = records.iter().map(|u| u.timestamp).max();
    SessionStatistics {
        session_id: session_id.to_string(),
        name: session.name(),
        description: session.description(),
        created_at: session.created_at(),
        last_updated: session.last_updated(),
        hot_updates: session.hot_records().len(),
        total_updates: records.len(),
        entity_count: session.entity_graph().entities.len(),
        relationship_count: session.entity_graph().get_graph_stats().edge_count,
        code_references: session.code_references().len(),
        change_history: session.change_history().len(),
        first_update: first,
        last_update: last,
        duration_minutes: first.zip(last).map(|(f, l)| (l - f).num_minutes()),
    }
}

#[cfg(test)]
mod tests;
