// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Model Context Protocol (MCP) tool definitions for post-cortex.
//!
//! Pure library — no `rmcp`, `axum`, `tonic`, or transport runtime.
//! Each public function takes the typed parameters of a single MCP
//! tool, dispatches into the post-cortex domain layer, and returns an
//! [`MCPToolResult`].
//!
//! The 9 consolidated tool surfaces live as top-level modules: see
//! [`session`], [`update_context`], [`query`], [`search`], [`analysis`],
//! [`workspace`], and [`schemas`]. Headline helpers
//! ([`MCPToolResult`], `MEMORY_SYSTEM`, `get_memory_system`) are
//! re-exported below.
//!
//! ## Phase 6 status
//!
//! The crate currently calls into [`post_cortex_memory::ConversationMemorySystem`]
//! directly via a global `LazyLock<ArcSwap>` singleton. Phase 7 (daemon
//! extraction) is where the function signatures flip to take
//! `&dyn post_cortex_core::services::PostCortexService` and the
//! singleton goes away — at that point this crate's transitive
//! dependency on `post-cortex-memory` / `post-cortex-storage` drops to
//! `dev-dependencies` and downstream Rust projects can plug in their
//! own service impl.

#![forbid(unsafe_code)]
#![allow(clippy::result_large_err)]
#![allow(clippy::type_complexity)]

/// Typed error hierarchy for the MCP tool layer.
pub mod error;
pub use error::{Error, Result as McpResult};

use anyhow::Result;
use arc_swap::ArcSwap;
use post_cortex_core::core::context_update::{CodeReference, ContextUpdate, EntityType};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, LazyLock};
use tracing::info;

use post_cortex_memory::services::MemoryServiceImpl;
use post_cortex_memory::{ConversationMemorySystem, SystemConfig};

/// Analysis, summaries, insights, and session statistics.
pub mod analysis;
/// Structured and keyword-based queries over session context.
pub mod query;
/// JSON Schema descriptors for every MCP tool.
pub mod schemas;
/// Semantic and embedding-powered search across sessions.
pub mod search;
/// Session lifecycle: create, load, checkpoint, list, search, metadata.
pub mod session;
/// Helpers for recording context updates (single and bulk).
pub mod update_context;
/// Workspace CRUD and session-to-workspace membership.
pub mod workspace;

pub use analysis::{
    get_entity_importance_analysis, get_entity_network_view, get_key_decisions, get_key_insights,
    get_session_statistics, get_structured_summary, get_tool_catalog,
};
pub use query::{query_conversation_context, query_conversation_context_with_system};
pub use schemas::get_all_tool_schemas;
pub use search::{
    enable_embeddings, find_related_content, get_vectorization_stats, semantic_search,
    semantic_search_global, semantic_search_session, vectorize_session,
};
pub use session::{
    create_session_checkpoint, create_session_checkpoint_with_system, list_sessions,
    list_sessions_with_storage, load_session, load_session_checkpoint,
    load_session_checkpoint_with_system, load_session_with_system, mark_important, search_sessions,
    update_session_metadata,
};
pub use update_context::{bulk_update_conversation_context, update_conversation_context};
pub use workspace::{
    add_session_to_workspace, create_workspace, delete_workspace, get_workspace, list_workspaces,
    remove_session_from_workspace,
};

/// Convert a plain string into an `anyhow::Error`.
fn string_to_anyhow(s: String) -> anyhow::Error {
    anyhow::Error::msg(s)
}

/// Typed query descriptors dispatched by `query::query_context`.
#[derive(Serialize, Deserialize, Debug)]
pub enum ContextQuery {
    /// Retrieve context updates created after a given timestamp.
    GetRecentChanges {
        /// Earliest timestamp to include.
        since: chrono::DateTime<chrono::Utc>,
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

    /// Graph-aware retrieval combining semantic search and traversal.
    AssembleContext {
        /// Natural-language query for relevance scoring.
        query: String,
        /// Maximum approximate token count for the returned context.
        token_budget: usize,
    },
}

/// Typed response payloads returned by `query::query_context`.
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
    /// Serialized entity network graph.
    EntityNetwork(String),
    /// Serialized shortest path between two entities.
    ConnectionPath(String),
    /// Generic list of entity names.
    Entities(Vec<String>),
    /// Human-readable entity importance analysis.
    ImportanceAnalysis(String),
    /// Serialized entity hierarchy tree.
    EntityHierarchy(String),
    /// Serialized entity cluster data.
    EntityClusters(String),
    /// Serialized entity timeline data.
    EntityTimeline(String),
    /// Serialized entity trend analysis.
    EntityTrends(String),
    /// Graph-aware assembled context within a token budget.
    AssembledContext(post_cortex_memory::context_assembly::AssembledContext),
}

/// Global singleton holding the optional injected memory system.
static MEMORY_SYSTEM: LazyLock<ArcSwap<Option<Arc<ConversationMemorySystem>>>> =
    LazyLock::new(|| ArcSwap::new(Arc::new(None)));

/// Global singleton holding the canonical [`MemoryServiceImpl`] derived
/// from `MEMORY_SYSTEM`. Built lazily on the first call to `get_service`
/// and replaced whenever a new memory system is injected.
///
/// Phase 6 (this commit) wires only `update_conversation_context` and
/// `bulk_update_conversation_context` through this service. Other MCP
/// tools still use the raw [`ConversationMemorySystem`] until they're
/// migrated. Once every tool flows through the service, the singleton
/// goes away and the function signatures flip to take
/// `&dyn PostCortexService` (Phase 7).
static SERVICE: LazyLock<ArcSwap<Option<Arc<MemoryServiceImpl>>>> =
    LazyLock::new(|| ArcSwap::new(Arc::new(None)));

/// Inject a pre-built memory system for daemon mode.
pub fn inject_memory_system(system: Arc<ConversationMemorySystem>) {
    info!("MCP-TOOLS: Injecting external memory system for daemon mode");
    // Wrap the injected system in a canonical service and store it
    // alongside the raw handle so write-path callers can pick it up
    // without reconstructing the Pipeline on every request.
    let service = Arc::new(MemoryServiceImpl::new(system.clone()));
    MEMORY_SYSTEM.store(Arc::new(Some(system)));
    SERVICE.store(Arc::new(Some(service)));
    info!("MCP-TOOLS: Memory system injection complete");
}

/// Return the cached canonical service, building it from the current
/// memory system if necessary. Both `update_conversation_context` and
/// `bulk_update_conversation_context` flow through this — no transport
/// is allowed to bypass the canonical impl.
pub async fn get_service() -> Result<Arc<MemoryServiceImpl>> {
    if let Some(svc) = SERVICE.load().as_ref() {
        return Ok(svc.clone());
    }
    // Cold start: build from the (possibly cold) memory system. If two
    // callers race here they'll each construct a `MemoryServiceImpl`,
    // but `rcu` keeps the first one installed and the loser is dropped
    // when its Arc goes out of scope.
    let system = get_memory_system().await?;
    let new_svc = Arc::new(MemoryServiceImpl::new(system));
    let new_option = Arc::new(Some(new_svc));
    SERVICE.rcu(|current| {
        if current.is_none() {
            new_option.clone()
        } else {
            current.clone()
        }
    });
    Ok(SERVICE.load().as_ref().as_ref().unwrap().clone())
}

/// Create a new memory system from the given configuration.
pub async fn get_memory_system_with_config(
    config: SystemConfig,
) -> Result<ConversationMemorySystem> {
    ConversationMemorySystem::new(config)
        .await
        .map_err(anyhow::Error::msg)
}

/// Return the global memory system, lazily initialising it if needed.
pub async fn get_memory_system() -> Result<Arc<ConversationMemorySystem>> {
    info!("MCP-TOOLS: get_memory_system() called");

    if let Some(system) = MEMORY_SYSTEM.load().as_ref() {
        info!("MCP-TOOLS: Using existing system");
        return Ok(system.clone());
    }

    info!("MCP-TOOLS: System not initialized, proceeding with initialization");

    let data_directory = dirs::home_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join(".post-cortex/data")
        .to_str()
        .unwrap()
        .to_string();

    let mut config = SystemConfig {
        data_directory,
        ..SystemConfig::default()
    };

    #[cfg(feature = "embeddings")]
    {
        config.enable_embeddings = true;
        config.embeddings_model_type = "MultilingualMiniLM".to_string();
        config.auto_vectorize_on_update = true;
        config.cross_session_search_enabled = true;
        info!("MCP-TOOLS: Embeddings enabled in config");
    }

    #[cfg(not(feature = "embeddings"))]
    {
        info!("MCP-TOOLS: Embeddings not compiled in");
    }

    info!("MCP-TOOLS: About to call ConversationMemorySystem::new()");
    let system = ConversationMemorySystem::new(config)
        .await
        .map_err(anyhow::Error::msg)?;
    info!("MCP-TOOLS: ConversationMemorySystem created successfully");

    let arc_system = Arc::new(system);

    let new_option = Arc::new(Some(arc_system.clone()));
    MEMORY_SYSTEM.rcu(|current| {
        if current.is_none() {
            info!("MCP-TOOLS: Storing newly created system");
            new_option.clone()
        } else {
            info!("MCP-TOOLS: Another thread already initialized the system, using existing");
            current.clone()
        }
    });

    info!("MCP-TOOLS: System initialization completed");

    Ok(MEMORY_SYSTEM.load().as_ref().as_ref().unwrap().clone())
}

/// Typed interaction payloads matching MCP `interaction_type` values.
#[derive(Serialize, Deserialize, Debug)]
pub enum Interaction {
    /// A question-answer pair.
    QA {
        /// The question asked.
        question: String,
        /// The answer provided.
        answer: String,
        /// Supplementary detail lines.
        details: Vec<String>,
    },
    /// A code change recorded in the session.
    CodeChange {
        /// Path to the changed file.
        file_path: String,
        /// Diff or change description.
        diff: String,
        /// Supplementary detail lines.
        details: Vec<String>,
    },
    /// A problem that was solved.
    ProblemSolved {
        /// Description of the problem.
        problem: String,
        /// Description of the solution.
        solution: String,
        /// Supplementary detail lines.
        details: Vec<String>,
    },
    /// An architectural or technical decision.
    DecisionMade {
        /// The decision that was made.
        decision: String,
        /// Rationale behind the decision.
        rationale: String,
        /// Supplementary detail lines.
        details: Vec<String>,
    },
    /// A new requirement added to the project.
    RequirementAdded {
        /// The requirement text.
        requirement: String,
        /// Priority level (e.g. "high", "medium", "low").
        priority: String,
        /// Supplementary detail lines.
        details: Vec<String>,
    },
    /// A concept definition recorded for future reference.
    ConceptDefined {
        /// Name of the concept.
        concept: String,
        /// Definition of the concept.
        definition: String,
        /// Supplementary detail lines.
        details: Vec<String>,
    },
}

/// Standardised result envelope for every MCP tool.
#[derive(Serialize, Deserialize, Debug)]
pub struct MCPToolResult {
    /// Whether the tool invocation succeeded.
    pub success: bool,
    /// Human-readable status or error message.
    pub message: String,
    /// Optional structured payload.
    pub data: Option<serde_json::Value>,
}

impl MCPToolResult {
    /// Build a successful result with an optional JSON payload.
    pub fn success(message: String, data: Option<serde_json::Value>) -> Self {
        Self {
            success: true,
            message,
            data,
        }
    }

    /// Build an error result with no payload.
    pub fn error(message: String) -> Self {
        Self {
            success: false,
            message,
            data: None,
        }
    }
}

/// A single context update item accepted by the bulk-update tool.
#[derive(Serialize, Deserialize, Debug, JsonSchema, Clone)]
pub struct ContextUpdateItem {
    /// Interaction type discriminator (e.g. `"qa"`, `"decision_made"`).
    pub interaction_type: String,
    /// Key-value content fields for the interaction.
    pub content: HashMap<String, String>,
    /// Named entities mentioned in this update. Required by the canonical
    /// write path so the entity graph is never silently empty for
    /// MCP-driven writes.
    #[serde(default)]
    pub entities: Vec<EntityItem>,
    /// Relations between the entities listed above. Both endpoints must
    /// appear in `entities`; the canonical impl rejects dangling
    /// references and self-relations.
    #[serde(default)]
    pub relations: Vec<RelationItem>,
    /// Optional code reference attached to the update.
    pub code_reference: Option<CodeReference>,
}

/// Wire shape for an entity carried by an MCP `update_conversation_context`
/// call. `entity_type` is a lowercase string from the closed set:
/// `technology`, `concept`, `problem`, `solution`, `decision`,
/// `code_component`. Unknown values fall back to `concept` server-side.
#[derive(Serialize, Deserialize, Debug, JsonSchema, Clone)]
pub struct EntityItem {
    /// Unique human-readable entity name.
    pub name: String,
    /// Lowercase entity-type string (`technology`, `concept`, ...).
    #[serde(default = "default_entity_type")]
    pub entity_type: String,
}

fn default_entity_type() -> String {
    "concept".to_string()
}

/// Wire shape for a relation between two named entities. `relation_type`
/// is a lowercase string from the closed set: `required_by`, `leads_to`,
/// `related_to`, `conflicts_with`, `depends_on`, `implements`,
/// `caused_by`, `solves`. Unknown values cause the request to be
/// rejected with `InvalidArgument`.
#[derive(Serialize, Deserialize, Debug, JsonSchema, Clone)]
pub struct RelationItem {
    /// Source entity name (must match a `name` in the `entities` array).
    pub from_entity: String,
    /// Target entity name (must also match an `entities` entry).
    pub to_entity: String,
    /// Lowercase relation-type string.
    pub relation_type: String,
    /// Short explanation of why this relation exists.
    pub context: String,
}

/// Parse a datetime string in RFC 3339, `%Y-%m-%d %H:%M:%S`, or `%Y-%m-%d` format.
///
/// Returns 30 days ago when the input is empty.
pub(crate) fn parse_datetime(date_str: &str) -> Result<chrono::DateTime<chrono::Utc>> {
    if date_str.is_empty() {
        return Ok(chrono::Utc::now() - chrono::Duration::days(30));
    }

    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(date_str) {
        return Ok(dt.with_timezone(&chrono::Utc));
    }

    if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(date_str, "%Y-%m-%d %H:%M:%S") {
        return Ok(dt.and_utc());
    }

    if let Ok(dt) = chrono::NaiveDate::parse_from_str(date_str, "%Y-%m-%d") {
        return dt
            .and_hms_opt(0, 0, 0)
            .ok_or_else(|| anyhow::anyhow!("Invalid time components for date: {}", date_str))
            .map(|dt| dt.and_utc());
    }

    Err(anyhow::anyhow!("Failed to parse datetime: {}", date_str))
}
