// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Typed queries over one session's records and entity graph (the
//! `query_conversation_context` query types that are not graph views).

use std::borrow::Borrow;

use post_cortex_core::core::context_update::{
    CodeReference, ContextUpdate, EntityType, UpdateType,
};
use post_cortex_core::core::structured_context::QuestionStatus;
use post_cortex_core::session::active_session::ActiveSession;
use serde::{Deserialize, Serialize};

/// Typed query descriptors dispatched by [`crate::capabilities::Views::query`].
#[derive(Serialize, Deserialize, Debug)]
pub enum ContextQuery {
    /// Retrieve context updates created after a given timestamp.
    GetRecentChanges {
        /// Earliest timestamp to include; the last
        /// [`RECENT_CHANGES_DEFAULT_DAYS`] days when absent.
        since: Option<chrono::DateTime<chrono::Utc>>,
    },
    /// Look up code references for a specific file path.
    FindCodeReferences {
        /// File path to search references for.
        file_path: String,
    },
    /// Return the current structured summary of the session.
    GetStructuredSummary,
    /// Keyword search over stored context updates.
    SearchUpdates {
        /// Search query string.
        query: String,
    },
    /// Retrieve decision-type updates, optionally after a timestamp.
    GetDecisions {
        /// Optional lower-bound timestamp filter.
        since: Option<chrono::DateTime<chrono::Utc>>,
    },
    /// Return open questions tracked in the session.
    GetOpenQuestions,
    /// Return change history, optionally filtered by file path.
    GetChangeHistory {
        /// Optional file path to narrow results.
        file_path: Option<String>,
    },

    /// Find entity names related to a given entity.
    FindRelatedEntities {
        /// Name of the entity to search relations for.
        entity_name: String,
    },
    /// Return full context string for a named entity.
    GetEntityContext {
        /// Name of the entity.
        entity_name: String,
    },
    /// List all known entities, optionally filtered by type.
    GetAllEntities {
        /// Optional entity-type filter.
        entity_type: Option<EntityType>,
    },
    /// Traverse relationship edges starting from an entity.
    TraceRelationships {
        /// Entity name to start traversal from.
        from_entity: String,
        /// Maximum graph traversal depth.
        max_depth: usize,
    },

    /// Build a sub-graph network centered on an entity.
    GetEntityNetwork {
        /// Entity to place at the center of the network.
        center_entity: String,
        /// Maximum traversal depth.
        max_depth: usize,
    },
    /// Find the shortest relationship path between two entities.
    FindConnectionPath {
        /// Starting entity.
        from_entity: String,
        /// Target entity.
        to_entity: String,
        /// Maximum path length to explore.
        max_depth: usize,
    },
    /// Return the top-N entities ranked by importance score.
    GetMostImportantEntities {
        /// Maximum number of entities to return.
        limit: usize,
    },
    /// Return the most recently mentioned entities.
    GetRecentlyMentionedEntities {
        /// Maximum number of entities to return.
        limit: usize,
    },
    /// Perform a full importance analysis across all entities.
    AnalyzeEntityImportance,
    /// List entities matching a specific type.
    FindEntitiesByType {
        /// Entity type to filter by.
        entity_type: EntityType,
    },

    /// Return a hierarchical tree rooted at an entity.
    GetEntityHierarchy {
        /// Root entity for the hierarchy.
        root_entity: String,
        /// Maximum depth of the hierarchy.
        max_depth: usize,
    },
    /// Detect clusters of closely related entities.
    FindEntityClusters {
        /// Minimum number of entities to form a cluster.
        min_cluster_size: usize,
    },

    /// Return a timeline of mentions for a specific entity.
    GetEntityTimeline {
        /// Entity name to track.
        entity_name: String,
        /// Optional start of the time window.
        start_time: Option<chrono::DateTime<chrono::Utc>>,
        /// Optional end of the time window.
        end_time: Option<chrono::DateTime<chrono::Utc>>,
    },
    /// Analyse how entity activity trends over a time window.
    AnalyzeEntityTrends {
        /// Width of the rolling time window in days.
        time_window_days: i64,
    },
}

/// Typed response payloads returned by [`crate::capabilities::Views::query`].
#[derive(Serialize, Deserialize, Debug)]
pub enum ContextResponse {
    /// Recent context updates since a given timestamp.
    RecentChanges(Vec<ContextUpdate>),
    /// Code references matching a file path.
    CodeReferences(Vec<CodeReference>),
    /// Full structured summary of the session.
    StructuredSummary(post_cortex_core::core::structured_context::StructuredContext),
    /// Context updates matching a keyword search.
    SearchResults(Vec<ContextUpdate>),
    /// Decision-type context updates.
    Decisions(Vec<ContextUpdate>),
    /// Open questions tracked in the session.
    OpenQuestions(Vec<String>),
    /// Change history for a file or across all files.
    ChangeHistory(Vec<ContextUpdate>),
    /// Entity names related to a target entity.
    RelatedEntities(Vec<String>),
    /// Human-readable context summary for a single entity.
    EntityContext(String),
    /// All entity names, optionally filtered by type.
    AllEntities(Vec<String>),
    /// Entity names discovered by relationship traversal.
    EntityRelationships(Vec<String>),
    /// Entity network around a center entity.
    EntityNetwork(post_cortex_core::graph::entity_graph::EntityNetwork),
    /// Serialized shortest path between two entities.
    ConnectionPath(String),
    /// Generic list of entity names.
    Entities(Vec<String>),
    /// Per-entity importance analysis.
    ImportanceAnalysis(Vec<post_cortex_core::graph::entity_graph::EntityAnalysis>),
    /// Serialized entity hierarchy tree.
    EntityHierarchy(String),
    /// Serialized entity cluster data.
    EntityClusters(String),
    /// Serialized entity timeline data.
    EntityTimeline(String),
    /// Serialized entity trend analysis.
    EntityTrends(String),
    /// Graph-aware assembled context within a token budget.
    AssembledContext(crate::context_assembly::AssembledContext),
}

/// Every update in the session, oldest first. `incremental_updates` is the
/// complete log; the hot (last ~50) and warm tiers are subsets of it.
fn all_updates(session: &ActiveSession) -> impl Iterator<Item = &ContextUpdate> {
    // `Borrow` keeps this working whether the log stores `ContextUpdate` or
    // `Arc<ContextUpdate>`.
    session.records().iter().map(Borrow::borrow)
}

/// The required text of `query` that is empty, by its field name.
pub fn missing_field(query: &ContextQuery) -> Option<&'static str> {
    let required: &[(&'static str, &str)] = match query {
        ContextQuery::FindCodeReferences { file_path } => &[("file_path", file_path)],
        ContextQuery::SearchUpdates { query } => &[("query", query)],
        ContextQuery::FindRelatedEntities { entity_name }
        | ContextQuery::GetEntityContext { entity_name } => &[("entity_name", entity_name)],
        ContextQuery::TraceRelationships { from_entity, .. } => &[("entity_name", from_entity)],
        ContextQuery::GetEntityNetwork { center_entity, .. } => &[("entity_name", center_entity)],
        ContextQuery::FindConnectionPath {
            from_entity,
            to_entity,
            ..
        } => &[("from_entity", from_entity), ("to_entity", to_entity)],
        _ => &[],
    };
    required
        .iter()
        .find(|(_, value)| value.trim().is_empty())
        .map(|(name, _)| *name)
}

/// How far back `recent_changes` looks when no `since` is given.
pub const RECENT_CHANGES_DEFAULT_DAYS: i64 = 30;

/// The answer to `query` over `session` at `now`.
pub fn answer(
    session: &ActiveSession,
    query: ContextQuery,
    now: chrono::DateTime<chrono::Utc>,
) -> ContextResponse {
    match query {
        ContextQuery::GetRecentChanges { since } => {
            let since = since.unwrap_or(now - chrono::Duration::days(RECENT_CHANGES_DEFAULT_DAYS));
            let recent_updates: Vec<ContextUpdate> = all_updates(session)
                .filter(|u| u.timestamp >= since)
                .cloned()
                .collect();
            ContextResponse::RecentChanges(recent_updates)
        }
        ContextQuery::FindCodeReferences { file_path } => ContextResponse::CodeReferences(
            session
                .code_references()
                .remove(&file_path)
                .unwrap_or_default(),
        ),
        ContextQuery::GetStructuredSummary => {
            ContextResponse::StructuredSummary(session.current_state())
        }
        ContextQuery::FindRelatedEntities { entity_name } => {
            let related = session.entity_graph().find_related_entities(&entity_name);
            ContextResponse::RelatedEntities(related)
        }
        ContextQuery::GetEntityContext { entity_name } => {
            let context = session.entity_graph().get_entity_context(&entity_name);
            ContextResponse::EntityContext(context.unwrap_or("Entity not found".to_string()))
        }
        ContextQuery::GetAllEntities { entity_type } => {
            let entities: Vec<String> = match entity_type {
                Some(et) => session
                    .entity_graph()
                    .get_entities_by_type(&et)
                    .into_iter()
                    .map(|e| e.name.clone())
                    .collect(),
                None => session
                    .entity_graph()
                    .get_most_important_entities(50)
                    .into_iter()
                    .map(|e| e.name.clone())
                    .collect(),
            };
            ContextResponse::AllEntities(entities)
        }
        ContextQuery::TraceRelationships {
            from_entity,
            max_depth,
        } => {
            let trace = session
                .entity_graph()
                .trace_entity_relationships(&from_entity, max_depth);
            ContextResponse::Entities(trace.into_iter().map(|(a, _, _)| a).collect())
        }
        ContextQuery::GetEntityNetwork {
            center_entity,
            max_depth,
        } => {
            let network = session
                .entity_graph()
                .get_entity_network(&center_entity, max_depth);
            ContextResponse::EntityNetwork(network)
        }
        ContextQuery::GetMostImportantEntities { limit } => {
            let entities = session.entity_graph().get_most_important_entities(limit);
            ContextResponse::Entities(entities.into_iter().map(|e| e.name.clone()).collect())
        }
        ContextQuery::GetRecentlyMentionedEntities { limit } => {
            let entities = session
                .entity_graph()
                .get_recently_mentioned_entities(limit);
            ContextResponse::Entities(entities.into_iter().map(|e| e.name.clone()).collect())
        }
        ContextQuery::AnalyzeEntityImportance => {
            let analysis = session.entity_graph().analyze_entity_importance();
            ContextResponse::ImportanceAnalysis(analysis)
        }
        ContextQuery::FindEntitiesByType { entity_type } => {
            let entities = session.entity_graph().get_entities_by_type(&entity_type);
            ContextResponse::Entities(entities.into_iter().map(|e| e.name.clone()).collect())
        }
        ContextQuery::SearchUpdates { query } => {
            let query = query.to_lowercase();
            let update_results: Vec<ContextUpdate> = all_updates(session)
                .filter(|u| {
                    u.content.title.to_lowercase().contains(&query)
                        || u.content.description.to_lowercase().contains(&query)
                })
                .cloned()
                .collect();
            ContextResponse::SearchResults(update_results)
        }
        ContextQuery::GetDecisions { since } => {
            let decisions: Vec<ContextUpdate> = all_updates(session)
                .filter(|u| matches!(u.update_type, UpdateType::DecisionMade))
                .filter(|u| since.is_none_or(|s| u.timestamp >= s))
                .cloned()
                .collect();
            ContextResponse::Decisions(decisions)
        }
        ContextQuery::GetOpenQuestions => {
            let questions = session
                .current_state()
                .answered_questions
                .iter()
                .filter(|q| matches!(q.status, QuestionStatus::Open | QuestionStatus::InProgress))
                .map(|q| q.question.clone())
                .collect();
            ContextResponse::OpenQuestions(questions)
        }
        ContextQuery::GetChangeHistory { file_path } => {
            let changes: Vec<ContextUpdate> = all_updates(session)
                .filter(|u| matches!(u.update_type, UpdateType::CodeChanged))
                .filter(|u| {
                    file_path.as_deref().is_none_or(|path| {
                        u.related_code
                            .as_ref()
                            .is_some_and(|code| code.file_path == path)
                    })
                })
                .cloned()
                .collect();
            ContextResponse::ChangeHistory(changes)
        }
        _ => ContextResponse::Entities(vec!["Not implemented".to_string()]),
    }
}

#[cfg(test)]
mod tests;
