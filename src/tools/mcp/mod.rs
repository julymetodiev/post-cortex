use crate::core::context_update::{CodeReference, ContextUpdate, EntityType, UpdateType};
use anyhow::Result;
use arc_swap::ArcSwap;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, LazyLock};
use tracing::info;
use uuid::Uuid;

use crate::core::memory_system::{ConversationMemorySystem, SystemConfig};

pub mod update_context;
pub mod query;
pub mod session;
pub mod search;
pub mod analysis;
pub mod workspace;
pub mod schemas;

pub use update_context::{
    update_conversation_context, update_conversation_context_with_system,
    bulk_update_conversation_context,
};
pub use query::{query_conversation_context, query_conversation_context_with_system};
pub use session::{
    create_session_checkpoint, create_session_checkpoint_with_system,
    load_session_checkpoint, load_session_checkpoint_with_system,
    mark_important, list_sessions_with_storage, list_sessions,
    load_session_with_system, load_session, search_sessions,
    update_session_metadata,
};
pub use search::{
    semantic_search, semantic_search_global, semantic_search_session,
    find_related_content, vectorize_session, get_vectorization_stats,
    enable_embeddings,
};
pub use analysis::{
    get_structured_summary, get_key_decisions, get_key_insights,
    get_entity_importance_analysis, get_entity_network_view,
    get_session_statistics, get_tool_catalog,
};
pub use workspace::{
    create_workspace, get_workspace, list_workspaces,
    delete_workspace, add_session_to_workspace, remove_session_from_workspace,
};
pub use schemas::get_all_tool_schemas;

fn string_to_anyhow(s: String) -> anyhow::Error {
    anyhow::Error::msg(s)
}

#[derive(Serialize, Deserialize, Debug)]
pub enum ContextQuery {
    GetRecentChanges {
        since: chrono::DateTime<chrono::Utc>,
    },
    FindCodeReferences {
        file_path: String,
    },
    GetStructuredSummary,
    SearchUpdates {
        query: String,
    },
    GetDecisions {
        since: Option<chrono::DateTime<chrono::Utc>>,
    },
    GetOpenQuestions,
    GetChangeHistory {
        file_path: Option<String>,
    },

    FindRelatedEntities {
        entity_name: String,
    },
    GetEntityContext {
        entity_name: String,
    },
    GetAllEntities {
        entity_type: Option<EntityType>,
    },
    TraceRelationships {
        from_entity: String,
        max_depth: usize,
    },

    GetEntityNetwork {
        center_entity: String,
        max_depth: usize,
    },
    FindConnectionPath {
        from_entity: String,
        to_entity: String,
        max_depth: usize,
    },
    GetMostImportantEntities {
        limit: usize,
    },
    GetRecentlyMentionedEntities {
        limit: usize,
    },
    AnalyzeEntityImportance,
    FindEntitiesByType {
        entity_type: EntityType,
    },

    GetEntityHierarchy {
        root_entity: String,
        max_depth: usize,
    },
    FindEntityClusters {
        min_cluster_size: usize,
    },

    GetEntityTimeline {
        entity_name: String,
        start_time: Option<chrono::DateTime<chrono::Utc>>,
        end_time: Option<chrono::DateTime<chrono::Utc>>,
    },
    AnalyzeEntityTrends {
        time_window_days: i64,
    },

    AssembleContext {
        query: String,
        token_budget: usize,
    },
}

#[derive(Serialize, Deserialize, Debug)]
pub enum ContextResponse {
    RecentChanges(Vec<ContextUpdate>),
    CodeReferences(Vec<CodeReference>),
    StructuredSummary(crate::core::structured_context::StructuredContext),
    SearchResults(Vec<ContextUpdate>),
    Decisions(Vec<ContextUpdate>),
    OpenQuestions(Vec<String>),
    ChangeHistory(Vec<ContextUpdate>),
    RelatedEntities(Vec<String>),
    EntityContext(String),
    AllEntities(Vec<String>),
    EntityRelationships(Vec<String>),
    EntityNetwork(String),
    ConnectionPath(String),
    Entities(Vec<String>),
    ImportanceAnalysis(String),
    EntityHierarchy(String),
    EntityClusters(String),
    EntityTimeline(String),
    EntityTrends(String),
    AssembledContext(crate::core::context_assembly::AssembledContext),
}

static MEMORY_SYSTEM: LazyLock<ArcSwap<Option<Arc<ConversationMemorySystem>>>> =
    LazyLock::new(|| ArcSwap::new(Arc::new(None)));

pub fn inject_memory_system(system: Arc<ConversationMemorySystem>) {
    info!("MCP-TOOLS: Injecting external memory system for daemon mode");
    MEMORY_SYSTEM.store(Arc::new(Some(system)));
    info!("MCP-TOOLS: Memory system injection complete");
}

pub async fn get_memory_system_with_config(
    config: SystemConfig,
) -> Result<ConversationMemorySystem> {
    ConversationMemorySystem::new(config)
        .await
        .map_err(anyhow::Error::msg)
}

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

#[derive(Serialize, Deserialize, Debug)]
pub enum Interaction {
    QA {
        question: String,
        answer: String,
        details: Vec<String>,
    },
    CodeChange {
        file_path: String,
        diff: String,
        details: Vec<String>,
    },
    ProblemSolved {
        problem: String,
        solution: String,
        details: Vec<String>,
    },
    DecisionMade {
        decision: String,
        rationale: String,
        details: Vec<String>,
    },
    RequirementAdded {
        requirement: String,
        priority: String,
        details: Vec<String>,
    },
    ConceptDefined {
        concept: String,
        definition: String,
        details: Vec<String>,
    },
}

#[derive(Serialize, Deserialize, Debug)]
pub struct MCPToolResult {
    pub success: bool,
    pub message: String,
    pub data: Option<serde_json::Value>,
}

impl MCPToolResult {
    pub fn success(message: String, data: Option<serde_json::Value>) -> Self {
        Self {
            success: true,
            message,
            data,
        }
    }

    pub fn error(message: String) -> Self {
        Self {
            success: false,
            message,
            data: None,
        }
    }
}

#[derive(Serialize, Deserialize, Debug, JsonSchema, Clone)]
pub struct ContextUpdateItem {
    pub interaction_type: String,
    pub content: HashMap<String, String>,
    pub code_reference: Option<CodeReference>,
}

pub(crate) fn interaction_to_context_update(
    interaction: Interaction,
    code_reference: Option<CodeReference>,
) -> Result<ContextUpdate> {
    let id = Uuid::new_v4();
    let timestamp = chrono::Utc::now();

    let (update_type, content) = match interaction {
        Interaction::QA {
            question,
            answer,
            details,
        } => (
            UpdateType::QuestionAnswered,
            crate::core::context_update::UpdateContent {
                title: question.clone(),
                description: answer.clone(),
                details,
                examples: vec![],
                implications: vec![],
            },
        ),
        Interaction::CodeChange {
            file_path,
            diff,
            details,
        } => (
            UpdateType::CodeChanged,
            crate::core::context_update::UpdateContent {
                title: file_path.clone(),
                description: diff.clone(),
                details,
                examples: vec![],
                implications: vec!["Code functionality updated".to_string()],
            },
        ),
        Interaction::ProblemSolved {
            problem,
            solution,
            details,
        } => (
            UpdateType::ProblemSolved,
            crate::core::context_update::UpdateContent {
                title: problem.clone(),
                description: solution.clone(),
                details,
                examples: vec![],
                implications: vec!["Problem resolved".to_string()],
            },
        ),
        Interaction::DecisionMade {
            decision,
            rationale,
            details,
        } => (
            UpdateType::DecisionMade,
            crate::core::context_update::UpdateContent {
                title: decision.clone(),
                description: rationale.clone(),
                details,
                examples: vec![],
                implications: vec!["Decision recorded".to_string()],
            },
        ),
        Interaction::RequirementAdded {
            requirement,
            priority,
            details,
        } => (
            UpdateType::RequirementAdded,
            crate::core::context_update::UpdateContent {
                title: requirement.clone(),
                description: format!("Priority: {}", priority),
                details,
                examples: vec![],
                implications: vec!["Requirement added".to_string()],
            },
        ),
        Interaction::ConceptDefined {
            concept,
            definition,
            details,
        } => (
            UpdateType::ConceptDefined,
            crate::core::context_update::UpdateContent {
                title: concept.clone(),
                description: definition.clone(),
                details,
                examples: vec![],
                implications: vec!["Concept defined".to_string()],
            },
        ),
    };

    Ok(ContextUpdate {
        id,
        timestamp,
        update_type,
        content,
        related_code: code_reference,
        parent_update: None,
        user_marked_important: false,
        creates_entities: vec![],
        creates_relationships: vec![],
        references_entities: vec![],
        typed_entities: vec![],
    })
}

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
