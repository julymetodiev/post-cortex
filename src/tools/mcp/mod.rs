// Copyright (c) 2025 Julius ML
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
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
// SOFTWARE.
use crate::core::context_update::{CodeReference, ContextUpdate, EntityType, UpdateType};
use crate::core::memory_system::{ConversationMemorySystem, SystemConfig};
use crate::core::structured_context::StructuredContext;
use crate::core::timeout_utils::{with_mcp_timeout, with_storage_timeout};
use crate::session::active_session::ActiveSession;
use crate::storage::rocksdb_storage::SessionCheckpoint;
use crate::summary::SummaryGenerator;

#[cfg(feature = "embeddings")]
use crate::core::content_vectorizer::ContentType;
use anyhow::Result;
use arc_swap::ArcSwap;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, LazyLock};
use tracing::{debug, error, info, instrument, warn};
use uuid::Uuid;

// Helper function to convert String errors to anyhow::Error
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

    // Graph-based queries
    // Entity queries
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

    // Network analysis
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

    // Hierarchical analysis
    GetEntityHierarchy {
        root_entity: String,
        max_depth: usize,
    },
    FindEntityClusters {
        min_cluster_size: usize,
    },

    // Temporal analysis
    GetEntityTimeline {
        entity_name: String,
        start_time: Option<chrono::DateTime<chrono::Utc>>,
        end_time: Option<chrono::DateTime<chrono::Utc>>,
    },
    AnalyzeEntityTrends {
        time_window_days: i64,
    },

    // Graph-aware context assembly
    AssembleContext {
        query: String,
        token_budget: usize,
    },
}

#[derive(Serialize, Deserialize, Debug)]
pub enum ContextResponse {
    RecentChanges(Vec<ContextUpdate>),
    CodeReferences(Vec<CodeReference>),
    StructuredSummary(StructuredContext),
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

// Lock-free global system with lazy initialization using ArcSwap + LazyLock
// ArcSwap provides wait-free reads and lock-free writes, eliminating all Mutex contention
// LazyLock allows const initialization with Arc::new()
static MEMORY_SYSTEM: LazyLock<ArcSwap<Option<Arc<ConversationMemorySystem>>>> =
    LazyLock::new(|| ArcSwap::new(Arc::new(None)));

/// Inject external memory system for daemon mode
///
/// This allows daemon server to share its memory system with MCP tools,
/// enabling multiple Claude Code instances to use the same RocksDB database
/// through a central daemon process.
///
/// # Usage
/// ```ignore
/// # use post_cortex::{ConversationMemorySystem, SystemConfig};
/// # use post_cortex::tools::mcp::inject_memory_system;
/// # use std::sync::Arc;
/// # async fn example(config: SystemConfig) -> anyhow::Result<()> {
/// // In daemon startup:
/// let memory_system: Arc<ConversationMemorySystem> = Arc::new(ConversationMemorySystem::new(config).await?);
/// inject_memory_system(memory_system.clone());
/// # Ok(())
/// # }
/// ```
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

    // Fast path - lock-free read with ArcSwap (wait-free, no blocking)
    if let Some(system) = MEMORY_SYSTEM.load().as_ref() {
        info!("MCP-TOOLS: Using existing system");
        return Ok(system.clone());
    }

    info!("MCP-TOOLS: System not initialized, proceeding with initialization");

    // Slow path - initialize new system
    // Use same data directory as daemon mode: ~/.post-cortex/data
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

    // Store the system using lock-free compare-and-swap (RCU pattern)
    // This handles race conditions gracefully - if another thread won the race,
    // we use their system instead of ours
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

    // Return the system (either our newly created one or the one from the race winner)
    // Load returns Arc<Option<Arc<T>>>, deref to Option<Arc<T>>, then unwrap to Arc<T>
    Ok(MEMORY_SYSTEM.load().as_ref().as_ref().unwrap().clone())
}

#[instrument(skip(system, content), fields(
    session_id = %session_id,
    interaction_type = %interaction_type,
    has_code_reference = code_reference.is_some()
))]
pub async fn update_conversation_context_with_system(
    interaction_type: String,
    content: HashMap<String, String>,
    code_reference: Option<CodeReference>,
    session_id: Uuid,
    system: &ConversationMemorySystem,
) -> Result<MCPToolResult> {
    info!("Starting update_conversation_context_with_system");
    debug!("Parsing interaction type: {}", interaction_type);

    // Wrap entire operation in timeout to prevent infinite hangs
    let result = with_mcp_timeout(async {
        // Extract extra fields from content HashMap for details/implications
        let extract_extras = |exclude_keys: &[&str]| -> Vec<String> {
            content
                .iter()
                .filter(|(k, _)| !exclude_keys.contains(&k.as_str()))
                .map(|(k, v)| format!("{}: {}", k, v))
                .collect()
        };

        // Resolve a primary slot (title/description) using a list of preferred keys.
        // Falls back to a generic `title`/`description` key when none of the
        // domain-specific keys are present. This prevents ghost entries like
        // ": " when callers post content with unrecognised key names.
        let resolve_slot = |preferred: &[&str], fallback_keys: &[&str]| -> String {
            for k in preferred.iter().chain(fallback_keys.iter()) {
                if let Some(v) = content.get(*k) {
                    if !v.trim().is_empty() {
                        return v.clone();
                    }
                }
            }
            String::new()
        };

        let interaction = match interaction_type.as_str() {
            "qa" => {
                let question = resolve_slot(&["question"], &["title"]);
                let answer = resolve_slot(&["answer"], &["description"]);
                let extras = extract_extras(&["question", "answer", "title", "description"]);
                Interaction::QA {
                    question,
                    answer,
                    details: extras,
                }
            }
            "code_change" => {
                // Accept: file_path/file/description, changes/diff/change_type
                let file_path = resolve_slot(&["file_path", "file"], &["title"]);
                let changes = resolve_slot(
                    &["changes", "diff", "change_type", "change"],
                    &["description"],
                );
                let extras = extract_extras(&[
                    "file_path",
                    "file",
                    "title",
                    "description",
                    "changes",
                    "diff",
                    "change_type",
                    "change",
                ]);
                Interaction::CodeChange {
                    file_path,
                    diff: changes,
                    details: extras,
                }
            }
            "problem_solved" => {
                let problem = resolve_slot(&["problem"], &["title"]);
                let solution = resolve_slot(&["solution"], &["description"]);
                let extras = extract_extras(&["problem", "solution", "title", "description"]);
                Interaction::ProblemSolved {
                    problem,
                    solution,
                    details: extras,
                }
            }
            "decision_made" => {
                let decision = resolve_slot(&["decision"], &["title"]);
                let rationale = resolve_slot(&["rationale"], &["description"]);
                let extras = extract_extras(&["decision", "rationale", "title", "description"]);
                Interaction::DecisionMade {
                    decision,
                    rationale,
                    details: extras,
                }
            }
            "requirement_added" => {
                let requirement = resolve_slot(&["requirement"], &["title"]);
                let priority = content
                    .get("priority")
                    .cloned()
                    .unwrap_or_else(|| "medium".to_string());
                let extras = extract_extras(&[
                    "requirement",
                    "priority",
                    "title",
                    "description",
                ]);
                Interaction::RequirementAdded {
                    requirement,
                    priority,
                    details: extras,
                }
            }
            "concept_defined" => {
                let concept = resolve_slot(&["concept"], &["title"]);
                let definition = resolve_slot(&["definition"], &["description"]);
                let extras = extract_extras(&[
                    "concept",
                    "definition",
                    "title",
                    "description",
                ]);
                Interaction::ConceptDefined {
                    concept,
                    definition,
                    details: extras,
                }
            }
            _ => {
                error!("Unknown interaction type: {}", interaction_type);
                return Ok(MCPToolResult::error(format!(
                    "Unknown interaction type: {}",
                    interaction_type
                )));
            }
        };

        debug!("Converting interaction to ContextUpdate...");
        let mut update = interaction_to_context_update(interaction, code_reference)?;

        // Final guard: refuse to store an entry whose title AND description are
        // both empty after all fallbacks ran. This is what produced the 1-token
        // ": " ghost rows discovered 2026-05-16 when callers posted content
        // with keys the dispatcher didn't recognise.
        if update.content.title.trim().is_empty()
            && update.content.description.trim().is_empty()
        {
            let known_keys: Vec<String> = content.keys().cloned().collect();
            warn!(
                "Rejecting empty update for interaction_type='{}' (no usable title/description). Provided keys: {:?}",
                interaction_type, known_keys
            );
            return Ok(MCPToolResult::error(format!(
                "Refusing to store empty update for interaction_type='{}': none of the recognised keys carried text. \
                 Provide one of the type-specific keys (e.g. decision/rationale, problem/solution, concept/definition, \
                 question/answer, file/changes, requirement) or a generic 'title' and/or 'description'. \
                 Got keys: {:?}",
                interaction_type, known_keys
            )));
        }

        // Parse explicit entities from content (comma or space separated)
        if let Some(entities_str) = content.get("entities") {
            let entities: Vec<String> = entities_str
                .split(|c| c == ',' || c == ' ')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty() && s.len() > 2)
                .collect();
            if !entities.is_empty() {
                info!(
                    "Parsed {} explicit entities: {:?}",
                    entities.len(),
                    entities
                );
                update.creates_entities = entities;
            }
        }

        // Parse explicit relationships from content (format: "A RELATION B, C RELATION D")
        if let Some(rels_str) = content.get("relationships") {
            use crate::core::context_update::{EntityRelationship, RelationType};

            let mut relationships = Vec::new();
            for rel_part in rels_str.split(',') {
                let parts: Vec<&str> = rel_part.trim().split_whitespace().collect();
                // Expected format: "entity1 RELATION entity2"
                if parts.len() >= 3 {
                    let from = parts[0].to_string();
                    let rel_type_str = parts[1].to_uppercase();
                    let to = parts[2..].join(" ");

                    let relation_type = match rel_type_str.as_str() {
                        "DEPENDS_ON" => RelationType::DependsOn,
                        "IMPLEMENTS" => RelationType::Implements,
                        "CAUSES" | "CAUSED_BY" => RelationType::CausedBy,
                        "SOLVES" => RelationType::Solves,
                        "LEADS_TO" => RelationType::LeadsTo,
                        "REQUIRED_BY" => RelationType::RequiredBy,
                        "CONFLICTS_WITH" => RelationType::ConflictsWith,
                        _ => RelationType::RelatedTo, // Default for IMPROVES, USES, ENABLES, etc.
                    };

                    relationships.push(EntityRelationship {
                        from_entity: from,
                        to_entity: to,
                        relation_type,
                        context: update.content.title.clone(),
                    });
                }
            }
            if !relationships.is_empty() {
                info!("Parsed {} explicit relationships", relationships.len());
                update.creates_relationships = relationships;
            }
        }

        info!(
            "Created ContextUpdate with {} entities and {} relationships",
            update.creates_entities.len(),
            update.creates_relationships.len()
        );

        debug!("Adding context update to session: {}", session_id);
        // Combine title and description for full context
        let text = format!("{}\n{}", update.content.title, update.content.description);
        let metadata = Some(
            serde_json::to_value(&update)
                .map_err(|e| anyhow::anyhow!("Failed to serialize update metadata: {}", e))?,
        );
        system
            .add_incremental_update(session_id, text, metadata)
            .await
            .map_err(string_to_anyhow)?;
        info!("system.add_context_update completed successfully!");

        Ok(MCPToolResult::success(
            "Context updated successfully".to_string(),
            None,
        ))
    })
    .await;

    match result {
        Ok(success_result) => success_result,
        Err(timeout_error) => {
            error!(
                "TIMEOUT: update_conversation_context_with_system - session: {}, error: {}",
                session_id, timeout_error
            );
            Ok(MCPToolResult::error(format!(
                "Operation timed out: {}",
                timeout_error
            )))
        }
    }
}

pub async fn query_conversation_context_with_system(
    query_type: String,
    parameters: HashMap<String, String>,
    session_id: Uuid,
    system: &ConversationMemorySystem,
) -> Result<MCPToolResult> {
    eprintln!(
        "DEBUG: query_conversation_context_with_system - Looking for session: {}",
        session_id
    );

    let result = with_mcp_timeout(async {
        let session_arc = match system.get_session(session_id).await {
            Ok(session) => {
                eprintln!(
                    "DEBUG: query_conversation_context_with_system - Session found successfully"
                );
                session
            }
            Err(e) => {
                eprintln!(
                    "DEBUG: query_conversation_context_with_system - Session not found: {}",
                    e
                );
                return Err(anyhow::anyhow!("Session not found: {}", e));
            }
        };
        let session = session_arc.load();
        debug!("query_conversation_context: session {} loaded", session_id);

        let query = match query_type.as_str() {
            "recent_changes" => {
                let since_str = parameters.get("since").cloned().unwrap_or_default();
                let since = parse_datetime(&since_str)?;
                ContextQuery::GetRecentChanges { since }
            }
            "code_references" => {
                let file_path = parameters.get("file_path").cloned().unwrap_or_default();
                ContextQuery::FindCodeReferences { file_path }
            }
            "structured_summary" => ContextQuery::GetStructuredSummary,
            "decisions" => ContextQuery::GetDecisions { since: None },
            "open_questions" => ContextQuery::GetOpenQuestions,
            "related_entities" => {
                let entity_name = parameters.get("entity_name").cloned().unwrap_or_default();
                ContextQuery::FindRelatedEntities { entity_name }
            }
            "entity_context" => {
                let entity_name = parameters.get("entity_name").cloned().unwrap_or_default();
                ContextQuery::GetEntityContext { entity_name }
            }
            "all_entities" => ContextQuery::GetAllEntities { entity_type: None },
            "trace_relationships" => {
                let from_entity = parameters.get("entity_name").cloned().unwrap_or_default();
                let max_depth = parameters
                    .get("max_depth")
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(3);
                ContextQuery::TraceRelationships {
                    from_entity,
                    max_depth,
                }
            }
            "find_related_entities" => {
                let entity_name = parameters.get("entity_name").cloned().unwrap_or_default();
                ContextQuery::FindRelatedEntities { entity_name }
            }
            "get_entity_context" => {
                let entity_name = parameters.get("entity_name").cloned().unwrap_or_default();
                ContextQuery::GetEntityContext { entity_name }
            }
            "get_entity_network" => {
                let center_entity = parameters.get("entity_name").cloned().unwrap_or_default();
                let max_depth = parameters
                    .get("max_depth")
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(2);
                ContextQuery::GetEntityNetwork {
                    center_entity,
                    max_depth,
                }
            }
            "get_most_important_entities" => {
                let limit = parameters
                    .get("limit")
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(10);
                ContextQuery::GetMostImportantEntities { limit }
            }
            "get_recently_mentioned_entities" => {
                let limit = parameters
                    .get("limit")
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(10);
                ContextQuery::GetRecentlyMentionedEntities { limit }
            }
            "analyze_entity_importance" => ContextQuery::AnalyzeEntityImportance,
            "find_entities_by_type" => {
                let entity_type_str = parameters.get("entity_type").cloned().unwrap_or_default();
                let entity_type = match entity_type_str.as_str() {
                    "technology" => EntityType::Technology,
                    "concept" => EntityType::Concept,
                    "problem" => EntityType::Problem,
                    "solution" => EntityType::Solution,
                    "decision" => EntityType::Decision,
                    "code_component" => EntityType::CodeComponent,
                    _ => EntityType::Concept, // default
                };
                ContextQuery::FindEntitiesByType { entity_type }
            }
            "search_updates" => {
                let query = parameters.get("query").cloned().unwrap_or_default();
                ContextQuery::SearchUpdates { query }
            }
            "assemble_context" => {
                let query = parameters.get("query").cloned().unwrap_or_default();
                let token_budget = parameters
                    .get("token_budget")
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(4000);
                ContextQuery::AssembleContext { query, token_budget }
            }
            _ => {
                return Ok(MCPToolResult::error(format!(
                    "Unknown query type: {}",
                    query_type
                )));
            }
        };

        let response = query_context(&session, query).await?;
        let json_response = serde_json::to_value(response)?;

        Ok(MCPToolResult::success(
            "Query successful".to_string(),
            Some(json_response),
        ))
    })
    .await;

    match result {
        Ok(success_result) => success_result,
        Err(timeout_error) => {
            error!(
                "TIMEOUT: query_conversation_context_with_system - session: {}, error: {}",
                session_id, timeout_error
            );
            Ok(MCPToolResult::error(format!(
                "Query timed out: {}",
                timeout_error
            )))
        }
    }
}

pub async fn create_session_checkpoint_with_system(
    session_id: Uuid,
    system: &ConversationMemorySystem,
) -> Result<MCPToolResult> {
    let session_arc = system
        .get_session(session_id)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to load session: {}", e))?;
    let session = session_arc.load();

    // Create checkpoint
    let checkpoint = create_comprehensive_checkpoint(&session).await?;

    // Save checkpoint
    system
        .storage_actor
        .save_checkpoint(&checkpoint)
        .await
        .map_err(string_to_anyhow)?;

    Ok(MCPToolResult::success(
        "Checkpoint created successfully".to_string(),
        Some(serde_json::json!({ "checkpoint_id": checkpoint.id.to_string() })),
    ))
}

pub async fn load_session_checkpoint_with_system(
    checkpoint_id: String,
    session_id: Uuid,
    system: &ConversationMemorySystem,
) -> Result<MCPToolResult> {
    eprintln!("Loading checkpoint - step 1: Parsing checkpoint ID");
    let checkpoint_id = Uuid::parse_str(&checkpoint_id)?;

    eprintln!("Loading checkpoint - step 2: Loading checkpoint from storage");
    let checkpoint = system
        .storage_actor
        .load_checkpoint(checkpoint_id)
        .await
        .map_err(string_to_anyhow)?;

    eprintln!("Loading checkpoint - step 3: Checkpoint loaded successfully");

    // Restore session from checkpoint
    eprintln!("Loading checkpoint - step 4: Creating session from checkpoint");
    let mut session = ActiveSession::new(session_id, None, None);

    eprintln!("Loading checkpoint - step 5: Restoring current state");
    session.current_state = Arc::new(checkpoint.structured_context);

    eprintln!("Loading checkpoint - step 6: Restoring incremental updates");
    session.incremental_updates = Arc::new(checkpoint.recent_updates);

    eprintln!("Loading checkpoint - step 10: Restoring code references");
    session.code_references = Arc::new(checkpoint.code_references);

    eprintln!("Loading checkpoint - step 11: Restoring change history");
    session.change_history = Arc::new(checkpoint.change_history);

    eprintln!("Loading checkpoint - step 12: Entity graph restored");

    eprintln!("Loading checkpoint - step 13: Adding session to session manager");
    system
        .session_manager
        .sessions
        .put(session_id, Arc::new(ArcSwap::new(Arc::new(session))));

    eprintln!("Loading checkpoint - step 14: Updated session manager");

    Ok(MCPToolResult::success(
        "Session loaded from checkpoint successfully".to_string(),
        None,
    ))
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

// pub struct LLMClient {
//     api_key: Option<String>,
//     model: String,
//     base_url: String,
// }

// impl LLMClient {
//     pub fn new(api_key: Option<String>, model: String, base_url: String) -> Self {
//         Self {
//             api_key,
//             model,
//             base_url,
//         }
//     }
// }

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

/// Structure for a single context update in bulk operations
#[derive(Serialize, Deserialize, Debug, JsonSchema, Clone)]
pub struct ContextUpdateItem {
    pub interaction_type: String,
    pub content: HashMap<String, String>,
    pub code_reference: Option<CodeReference>,
}

/// Bulk update conversation context - add multiple context updates at once
pub async fn bulk_update_conversation_context(
    updates: Vec<ContextUpdateItem>,
    session_id: Uuid,
) -> Result<MCPToolResult> {
    info!(
        "MCP-TOOLS: bulk_update_conversation_context() called with {} updates for session {}",
        updates.len(),
        session_id
    );

    let system = get_memory_system().await?;
    let mut success_count = 0;
    let mut error_count = 0;
    let mut errors: Vec<String> = Vec::new();

    for (index, update_item) in updates.into_iter().enumerate() {
        // Extract extra fields from content HashMap for details
        let extract_extras = |exclude_keys: &[&str]| -> Vec<String> {
            update_item
                .content
                .iter()
                .filter(|(k, _)| !exclude_keys.contains(&k.as_str()))
                .map(|(k, v)| format!("{}: {}", k, v))
                .collect()
        };

        // Same fallback resolver as the single-update path: prefer domain keys,
        // fall back to generic title/description, ignore whitespace-only values.
        let resolve_slot = |preferred: &[&str], fallback_keys: &[&str]| -> String {
            for k in preferred.iter().chain(fallback_keys.iter()) {
                if let Some(v) = update_item.content.get(*k) {
                    if !v.trim().is_empty() {
                        return v.clone();
                    }
                }
            }
            String::new()
        };

        let interaction = match update_item.interaction_type.as_str() {
            "qa" => {
                let question = resolve_slot(&["question"], &["title"]);
                let answer = resolve_slot(&["answer"], &["description"]);
                let extras = extract_extras(&["question", "answer", "title", "description"]);
                Interaction::QA {
                    question,
                    answer,
                    details: extras,
                }
            }
            "code_change" => {
                let file_path = resolve_slot(&["file_path", "file"], &["title", "description"]);
                let changes =
                    resolve_slot(&["changes", "diff", "change_type", "change"], &["description"]);
                let extras = extract_extras(&[
                    "file_path",
                    "file",
                    "title",
                    "description",
                    "changes",
                    "diff",
                    "change_type",
                    "change",
                ]);
                Interaction::CodeChange {
                    file_path,
                    diff: changes,
                    details: extras,
                }
            }
            "problem_solved" => {
                let problem = resolve_slot(&["problem"], &["title"]);
                let solution = resolve_slot(&["solution"], &["description"]);
                let extras = extract_extras(&["problem", "solution", "title", "description"]);
                Interaction::ProblemSolved {
                    problem,
                    solution,
                    details: extras,
                }
            }
            "decision_made" => {
                let decision = resolve_slot(&["decision"], &["title"]);
                let rationale = resolve_slot(&["rationale"], &["description"]);
                let extras = extract_extras(&["decision", "rationale", "title", "description"]);
                Interaction::DecisionMade {
                    decision,
                    rationale,
                    details: extras,
                }
            }
            "requirement_added" => {
                let requirement = resolve_slot(&["requirement"], &["title"]);
                let priority = update_item
                    .content
                    .get("priority")
                    .cloned()
                    .unwrap_or_else(|| "medium".to_string());
                let extras = extract_extras(&[
                    "requirement",
                    "priority",
                    "title",
                    "description",
                ]);
                Interaction::RequirementAdded {
                    requirement,
                    priority,
                    details: extras,
                }
            }
            "concept_defined" => {
                let concept = resolve_slot(&["concept"], &["title"]);
                let definition = resolve_slot(&["definition"], &["description"]);
                let extras = extract_extras(&[
                    "concept",
                    "definition",
                    "title",
                    "description",
                ]);
                Interaction::ConceptDefined {
                    concept,
                    definition,
                    details: extras,
                }
            }
            _ => {
                error_count += 1;
                errors.push(format!(
                    "Update {}: Unknown interaction type: {}",
                    index, update_item.interaction_type
                ));
                continue;
            }
        };

        // Convert to ContextUpdate
        match interaction_to_context_update(interaction, update_item.code_reference) {
            Ok(update) => {
                // Refuse to persist empty rows — same guard as the single-update path.
                if update.content.title.trim().is_empty()
                    && update.content.description.trim().is_empty()
                {
                    let known_keys: Vec<String> =
                        update_item.content.keys().cloned().collect();
                    error_count += 1;
                    errors.push(format!(
                        "Update {}: empty title and description for interaction_type='{}'. \
                         Provided keys: {:?}. Supply a recognised key or a generic 'title'/'description'.",
                        index, update_item.interaction_type, known_keys
                    ));
                    continue;
                }
                let description = update.content.description.clone();
                let metadata = match serde_json::to_value(&update) {
                    Ok(v) => Some(v),
                    Err(e) => {
                        error_count += 1;
                        errors.push(format!(
                            "Update {}: Failed to serialize metadata: {}",
                            index, e
                        ));
                        continue;
                    }
                };

                // Add to session
                match system
                    .add_incremental_update(session_id, description, metadata)
                    .await
                {
                    Ok(_) => {
                        success_count += 1;
                        debug!("Update {} added successfully", index);
                    }
                    Err(e) => {
                        error_count += 1;
                        errors.push(format!("Update {}: Failed to add: {}", index, e));
                    }
                }
            }
            Err(e) => {
                error_count += 1;
                errors.push(format!("Update {}: Failed to convert: {}", index, e));
            }
        }
    }

    let message = if error_count == 0 {
        format!(
            "Bulk update completed successfully: {} updates added",
            success_count
        )
    } else {
        format!(
            "Bulk update completed with errors: {} succeeded, {} failed",
            success_count, error_count
        )
    };

    // Auto-vectorize after all bulk updates are complete
    #[cfg(feature = "embeddings")]
    {
        if let Err(e) = system.auto_vectorize_if_enabled(session_id).await {
            // Don't fail the bulk operation if auto-vectorization fails
            warn!(
                "Auto-vectorization warning after bulk updates for session {}: {}",
                session_id, e
            );
        } else {
            info!(
                "Auto-vectorization completed after {} bulk updates for session {}",
                success_count, session_id
            );
        }
    }

    Ok(MCPToolResult::success(
        message,
        Some(serde_json::json!({
            "success_count": success_count,
            "error_count": error_count,
            "errors": errors
        })),
    ))
}

pub async fn update_conversation_context(
    interaction_type: String,
    content: HashMap<String, String>,
    code_reference: Option<CodeReference>,
    session_id: Uuid,
) -> Result<MCPToolResult> {
    info!("MCP-TOOLS: Getting memory system for update_conversation_context");
    let system = get_memory_system().await?;
    info!("MCP-TOOLS: Got memory system, delegating to update_conversation_context_with_system");

    // Delegate to the single implementation with timeout handling
    update_conversation_context_with_system(
        interaction_type,
        content,
        code_reference,
        session_id,
        &system,
    )
    .await
}

pub async fn query_conversation_context(
    query_type: String,
    parameters: HashMap<String, String>,
    session_id: Uuid,
) -> Result<MCPToolResult> {
    let system = get_memory_system().await?;
    let session_arc = system
        .get_session(session_id)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to load session: {}", e))?;
    let session = session_arc.load();

    let query = match query_type.as_str() {
        "recent_changes" => {
            let since_str = parameters.get("since").cloned().unwrap_or_default();
            let since = parse_datetime(&since_str)?;
            ContextQuery::GetRecentChanges { since }
        }
        "code_references" => {
            let file_path = parameters.get("file_path").cloned().unwrap_or_default();
            ContextQuery::FindCodeReferences { file_path }
        }
        "structured_summary" => ContextQuery::GetStructuredSummary,
        "decisions" => ContextQuery::GetDecisions { since: None },
        "open_questions" => ContextQuery::GetOpenQuestions,
        "related_entities" => {
            let entity_name = parameters.get("entity_name").cloned().unwrap_or_default();
            ContextQuery::FindRelatedEntities { entity_name }
        }
        "entity_context" => {
            let entity_name = parameters.get("entity_name").cloned().unwrap_or_default();
            ContextQuery::GetEntityContext { entity_name }
        }
        "all_entities" => ContextQuery::GetAllEntities { entity_type: None },
        "trace_relationships" => {
            let from_entity = parameters.get("entity_name").cloned().unwrap_or_default();
            let max_depth = parameters
                .get("max_depth")
                .and_then(|s| s.parse().ok())
                .unwrap_or(3);
            ContextQuery::TraceRelationships {
                from_entity,
                max_depth,
            }
        }
        "find_related_entities" => {
            let entity_name = parameters.get("entity_name").cloned().unwrap_or_default();
            ContextQuery::FindRelatedEntities { entity_name }
        }
        "get_entity_context" => {
            let entity_name = parameters.get("entity_name").cloned().unwrap_or_default();
            ContextQuery::GetEntityContext { entity_name }
        }
        "get_entity_network" => {
            let center_entity = parameters.get("entity_name").cloned().unwrap_or_default();
            let max_depth = parameters
                .get("max_depth")
                .and_then(|s| s.parse().ok())
                .unwrap_or(2);
            ContextQuery::GetEntityNetwork {
                center_entity,
                max_depth,
            }
        }
        "get_most_important_entities" => {
            let limit = parameters
                .get("limit")
                .and_then(|s| s.parse().ok())
                .unwrap_or(10);
            ContextQuery::GetMostImportantEntities { limit }
        }
        "get_recently_mentioned_entities" => {
            let limit = parameters
                .get("limit")
                .and_then(|s| s.parse().ok())
                .unwrap_or(10);
            ContextQuery::GetRecentlyMentionedEntities { limit }
        }
        "analyze_entity_importance" => ContextQuery::AnalyzeEntityImportance,
        "find_entities_by_type" => {
            let entity_type_str = parameters.get("entity_type").cloned().unwrap_or_default();
            let entity_type = match entity_type_str.as_str() {
                "technology" => EntityType::Technology,
                "concept" => EntityType::Concept,
                "problem" => EntityType::Problem,
                "solution" => EntityType::Solution,
                "decision" => EntityType::Decision,
                "code_component" => EntityType::CodeComponent,
                _ => EntityType::Concept, // default
            };
            ContextQuery::FindEntitiesByType { entity_type }
        }
        "search_updates" => {
            let query = parameters.get("query").cloned().unwrap_or_default();
            ContextQuery::SearchUpdates { query }
        }
        "assemble_context" => {
            let query = parameters.get("query").cloned().unwrap_or_default();
            let token_budget = parameters
                .get("token_budget")
                .and_then(|s| s.parse().ok())
                .unwrap_or(4000);
            ContextQuery::AssembleContext { query, token_budget }
        }
        _ => {
            return Ok(MCPToolResult::error(format!(
                "Unknown query type: {}",
                query_type
            )));
        }
    };

    let response = query_context(&session, query).await?;
    let json_response = serde_json::to_value(response)?;

    Ok(MCPToolResult::success(
        "Query successful".to_string(),
        Some(json_response),
    ))
}

pub async fn create_session_checkpoint(session_id: Uuid) -> Result<MCPToolResult> {
    let result = with_storage_timeout(async {
        let system = get_memory_system().await?;
        let session_arc = system
            .get_session(session_id)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to load session: {}", e))?;
        let session = session_arc.load();

        // Create checkpoint
        let checkpoint = create_comprehensive_checkpoint(&session).await?;

        // Save checkpoint
        system
            .storage_actor
            .save_checkpoint(&checkpoint)
            .await
            .map_err(string_to_anyhow)?;

        Ok(MCPToolResult::success(
            "Checkpoint created successfully".to_string(),
            Some(serde_json::json!({ "checkpoint_id": checkpoint.id.to_string() })),
        ))
    })
    .await;

    match result {
        Ok(success_result) => success_result,
        Err(timeout_error) => {
            error!(
                "TIMEOUT: create_session_checkpoint - session: {}, error: {}",
                session_id, timeout_error
            );
            Ok(MCPToolResult::error(format!(
                "Checkpoint creation timed out: {}",
                timeout_error
            )))
        }
    }
}

pub async fn load_session_checkpoint(
    checkpoint_id: String,
    session_id: Uuid,
) -> Result<MCPToolResult> {
    let result = with_storage_timeout(async {
        let system = get_memory_system().await?;
        let checkpoint_id = Uuid::parse_str(&checkpoint_id)?;

        // Load checkpoint from storage
        let checkpoint = system
            .storage_actor
            .load_checkpoint(checkpoint_id)
            .await
            .map_err(string_to_anyhow)?;

        // Restore session from checkpoint
        let mut session = ActiveSession::new(session_id, None, None);
        session.current_state = Arc::new(checkpoint.structured_context);
        session.incremental_updates = Arc::new(checkpoint.recent_updates);
        session.code_references = Arc::new(checkpoint.code_references);
        session.change_history = Arc::new(checkpoint.change_history);
        // Entity graph will be rebuilt from incremental updates

        // Add session to session manager
        system
            .session_manager
            .sessions
            .put(session_id, Arc::new(ArcSwap::new(Arc::new(session))));

        Ok(MCPToolResult::success(
            "Session loaded from checkpoint successfully".to_string(),
            None,
        ))
    })
    .await;

    match result {
        Ok(success_result) => success_result,
        Err(timeout_error) => {
            error!(
                "TIMEOUT: load_session_checkpoint - session: {}, error: {}",
                session_id, timeout_error
            );
            Ok(MCPToolResult::error(format!(
                "Checkpoint loading timed out: {}",
                timeout_error
            )))
        }
    }
}

pub async fn mark_important(session_id: Uuid, update_id: String) -> Result<MCPToolResult> {
    let update_id = Uuid::parse_str(&update_id)?;
    let system = get_memory_system().await?;
    let session_arc = system
        .get_session(session_id)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to load session: {}", e))?;

    // Find and mark the update as important using ArcSwap update
    let mut found = false;
    session_arc.rcu(|current| {
        let mut updated = (**current).clone();
        // Use Arc::make_mut for CoW semantics on incremental_updates
        let updates = Arc::make_mut(&mut updated.incremental_updates);
        for update in updates.iter_mut() {
            if update.id == update_id {
                update.user_marked_important = true;
                found = true;
                break;
            }
        }
        Arc::new(updated)
    });

    if found {
        Ok(MCPToolResult::success(
            "Update marked as important".to_string(),
            None,
        ))
    } else {
        Ok(MCPToolResult::error("Update not found".to_string()))
    }
}

pub async fn list_sessions_with_storage(
    storage: &crate::storage::rocksdb_storage::RealRocksDBStorage,
) -> Result<MCPToolResult> {
    match storage.list_sessions().await {
        Ok(session_ids) => {
            let mut sessions_info = Vec::new();

            // Load each session to get real metadata
            for session_id in session_ids {
                match storage.load_session(session_id).await {
                    Ok(session) => {
                        sessions_info.push(serde_json::json!({
                            "id": session_id.to_string(),
                            "name": session.name(),
                            "description": session.description(),
                            "created_at": session.created_at().to_rfc3339(),
                            "last_updated": session.last_updated.to_rfc3339(),
                            "update_count": session.incremental_updates.len(),
                            "entity_count": session.entity_graph.entities.len()
                        }));
                    }
                    Err(_) => {
                        // If we can't load the session, still include basic info
                        sessions_info.push(serde_json::json!({
                            "id": session_id.to_string(),
                            "name": null,
                            "description": null,
                            "created_at": "unknown",
                            "last_updated": "unknown",
                            "update_count": 0,
                            "entity_count": 0
                        }));
                    }
                }
            }
            Ok(MCPToolResult::success(
                format!("Found {} sessions", sessions_info.len()),
                Some(serde_json::json!({
                    "sessions": sessions_info
                })),
            ))
        }
        Err(e) => Ok(MCPToolResult::error(format!(
            "Failed to load sessions: {e}"
        ))),
    }
}

pub async fn list_sessions() -> Result<MCPToolResult> {
    info!("MCP-TOOLS: list_sessions() called");
    let result = with_storage_timeout(async {
        info!("MCP-TOOLS: Getting memory system for list_sessions");
        let system = get_memory_system().await?;
        info!("MCP-TOOLS: Got memory system, listing sessions");
        let session_ids = system.list_sessions().await.map_err(string_to_anyhow)?;

        let mut sessions_info = Vec::new();
        for session_id in session_ids {
            match system.get_session(session_id).await {
                Ok(session_arc) => {
                    let session = session_arc.load();
                    sessions_info.push(serde_json::json!({
                        "id": session_id.to_string(),
                        "name": session.name(),
                        "description": session.description(),
                        "created_at": session.created_at().to_rfc3339(),
                        "last_updated": session.last_updated.to_rfc3339(),
                        "update_count": session.incremental_updates.len(),
                        "entity_count": session.entity_graph.entities.len()
                    }));
                }
                Err(_) => {
                    sessions_info.push(serde_json::json!({
                        "id": session_id.to_string(),
                        "name": null,
                        "description": null,
                        "created_at": "unknown",
                        "last_updated": "unknown",
                        "update_count": 0,
                        "entity_count": 0
                    }));
                }
            }
        }

        Ok(MCPToolResult::success(
            format!("Found {} sessions", sessions_info.len()),
            Some(serde_json::json!({
                "sessions": sessions_info
            })),
        ))
    })
    .await;

    match result {
        Ok(success_result) => success_result,
        Err(timeout_error) => {
            error!("TIMEOUT: list_sessions - error: {timeout_error}");
            Ok(MCPToolResult::error(format!(
                "Session listing timed out: {timeout_error}"
            )))
        }
    }
}

pub async fn load_session_with_system(
    session_id: Uuid,
    system: &ConversationMemorySystem,
) -> Result<MCPToolResult> {
    match system.get_session(session_id).await {
        Ok(session_arc) => {
            let session = session_arc.load();
            Ok(MCPToolResult::success(
                "Session loaded successfully".to_string(),
                Some(serde_json::json!({
                    "session": {
                        "id": session.id().to_string(),
                        "created_at": session.created_at().to_rfc3339(),
                        "last_updated": session.last_updated.to_rfc3339(),
                        "update_count": session.incremental_updates.len(),
                        "entity_count": session.entity_graph.entities.len(),
                        "hot_context_size": session.hot_context.len(),
                        "warm_context_size": session.warm_context.len(),
                        "cold_context_size": session.cold_context.len(),
                        "code_references": session.code_references.keys().collect::<Vec<_>>(),
                        "change_history_count": session.change_history.len()
                    }
                })),
            ))
        }
        Err(e) => Ok(MCPToolResult::error(
            format!("Failed to load session: {e}",),
        )),
    }
}

pub async fn load_session(session_id: Uuid) -> Result<MCPToolResult> {
    let result = with_storage_timeout(async {
        let system = get_memory_system().await?;
        load_session_with_system(session_id, &system).await
    })
    .await;

    match result {
        Ok(success_result) => success_result,
        Err(timeout_error) => {
            error!(
                "TIMEOUT: load_session - session: {}, error: {}",
                session_id, timeout_error
            );
            Ok(MCPToolResult::error(format!(
                "Session loading timed out: {}",
                timeout_error
            )))
        }
    }
}

// Search for sessions by name or description
pub async fn search_sessions(query: String) -> Result<MCPToolResult> {
    let result = with_storage_timeout(async {
        let system = get_memory_system().await?;
        let session_ids = system
            .find_sessions_by_name_or_description(&query)
            .await
            .map_err(string_to_anyhow)?;

        // Load full session data for each matching session ID
        let mut sessions = Vec::new();
        for session_id in session_ids {
            if let Ok(session_arc) = system.get_session(session_id).await {
                let session = session_arc.load();
                sessions.push(serde_json::json!({
                    "id": session_id.to_string(),
                    "name": session.name(),
                    "description": session.description()
                }));
            }
        }

        Ok(MCPToolResult::success(
            format!("Found {} sessions matching '{}'", sessions.len(), query),
            Some(serde_json::json!({
                "sessions": sessions
            })),
        ))
    })
    .await;

    match result {
        Ok(success_result) => success_result,
        Err(timeout_error) => {
            error!(
                "TIMEOUT: search_sessions - query: {}, error: {}",
                query, timeout_error
            );
            Ok(MCPToolResult::error(format!(
                "Session search timed out: {}",
                timeout_error
            )))
        }
    }
}

// Universal semantic search tool
pub async fn semantic_search(
    query: String,
    scope: Option<serde_json::Value>,
) -> Result<MCPToolResult> {
    let result = with_storage_timeout(async {
        let system = get_memory_system().await?;

        // Parse scope
        let (scope_type, scope_id) = if let Some(scope_json) = scope {
            let type_ = scope_json["scope_type"]
                .as_str()
                .unwrap_or("global")
                .to_string();
            let id_str = scope_json.get("id").and_then(|v| v.as_str());
            let id = id_str.and_then(|s| Uuid::parse_str(s).ok());
            (type_, id)
        } else {
            ("global".to_string(), None)
        };

        // Use system methods which use the shared vectorizer
        let results = match scope_type.as_str() {
            "session" => {
                let session_id = scope_id
                    .ok_or_else(|| anyhow::anyhow!("Missing session ID for session scope"))?;
                system
                    .semantic_search_session(session_id, &query, None, None, None)
                    .await
                    .map_err(|e| anyhow::anyhow!(e))
            }
            "workspace" => {
                let ws_id = scope_id
                    .ok_or_else(|| anyhow::anyhow!("Missing workspace ID for workspace scope"))?;
                let workspace = system
                    .workspace_manager
                    .get_workspace(&ws_id)
                    .ok_or_else(|| anyhow::anyhow!("Workspace {} not found", ws_id))?;

                let session_ids: Vec<Uuid> = workspace
                    .get_all_sessions()
                    .into_iter()
                    .map(|(id, _)| id)
                    .collect();

                system
                    .semantic_search_multisession(&session_ids, &query, None, None, None)
                    .await
                    .map_err(|e| anyhow::anyhow!(e))
            }
            "global" => system
                .semantic_search_global(&query, None, None, None)
                .await
                .map_err(|e| anyhow::anyhow!(e)),
            _ => {
                return Ok(MCPToolResult::error(format!(
                    "Invalid search scope type: {}",
                    scope_type
                )));
            }
        };

        match results {
            Ok(results) => {
                let formatted: Vec<serde_json::Value> = results
                    .iter()
                    .map(|r| {
                        serde_json::json!({
                            "content": r.text_content,
                            "score": r.combined_score,
                            "session_id": r.session_id,
                            "type": format!("{:?}", r.content_type),
                            "timestamp": r.timestamp.to_rfc3339()
                        })
                    })
                    .collect();

                Ok(MCPToolResult::success(
                    format!("Found {} results", results.len()),
                    Some(serde_json::json!({ "results": formatted })),
                ))
            }
            Err(e) => Ok(MCPToolResult::error(format!("Search failed: {e}"))),
        }
    })
    .await;

    match result {
        Ok(success_result) => success_result,
        Err(timeout_error) => {
            error!("TIMEOUT: semantic_search - error: {timeout_error}");
            Ok(MCPToolResult::error(format!(
                "Semantic search timed out: {timeout_error}"
            )))
        }
    }
}

#[instrument(skip(session_id), fields(session_id = %session_id))]
pub async fn update_session_metadata(
    session_id: Uuid,
    name: Option<String>,
    description: Option<String>,
) -> Result<MCPToolResult> {
    let result = with_storage_timeout(async {
        let system = get_memory_system().await?;
        system
            .update_session_metadata(session_id, name, description)
            .await
            .map_err(string_to_anyhow)?;

        // Get the final values after update
        let session_arc = system
            .get_session(session_id)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to load session: {}", e))?;
        let session = session_arc.load();
        let (final_name, final_description) = session.get_metadata();

        Ok(MCPToolResult::success(
            "Session metadata updated successfully".to_string(),
            Some(serde_json::json!({
                "session_id": session_id.to_string(),
                "name": final_name,
                "description": final_description
            })),
        ))
    })
    .await;

    match result {
        Ok(success_result) => success_result,
        Err(timeout_error) => {
            error!(
                "TIMEOUT: update_session_metadata - session_id: {}, error: {}",
                session_id, timeout_error
            );
            Ok(MCPToolResult::error(format!(
                "TIMEOUT: Failed to update session metadata: {}",
                timeout_error
            )))
        }
    }
}

fn interaction_to_context_update(
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

async fn query_context(session: &ActiveSession, query: ContextQuery) -> Result<ContextResponse> {
    match query {
        ContextQuery::GetRecentChanges { since } => {
            let recent_updates: Vec<ContextUpdate> = session
                .hot_context
                .iter()
                .iter()
                .chain(session.warm_context.iter().map(|c| &c.update))
                .filter(|u| u.timestamp >= since)
                .cloned()
                .collect();
            Ok(ContextResponse::RecentChanges(recent_updates))
        }
        ContextQuery::FindCodeReferences { file_path } => {
            let refs = session
                .code_references
                .get(&file_path)
                .cloned()
                .unwrap_or_default();
            let converted_refs: Vec<CodeReference> = refs
                .into_iter()
                .map(|r| CodeReference {
                    file_path: r.file_path,
                    start_line: r.start_line,
                    end_line: r.end_line,
                    code_snippet: r.code_snippet,
                    commit_hash: r.commit_hash,
                    branch: r.branch,
                    change_description: r.change_description,
                })
                .collect();
            Ok(ContextResponse::CodeReferences(converted_refs))
        }
        ContextQuery::GetStructuredSummary => Ok(ContextResponse::StructuredSummary(
            (*session.current_state).clone(),
        )),
        ContextQuery::FindRelatedEntities { entity_name } => {
            let related = session.entity_graph.find_related_entities(&entity_name);
            Ok(ContextResponse::RelatedEntities(related))
        }
        ContextQuery::GetEntityContext { entity_name } => {
            let context = session.entity_graph.get_entity_context(&entity_name);
            Ok(ContextResponse::EntityContext(
                context.unwrap_or("Entity not found".to_string()),
            ))
        }
        ContextQuery::GetAllEntities { entity_type } => {
            let entities: Vec<String> = match entity_type {
                Some(et) => session
                    .entity_graph
                    .get_entities_by_type(&et)
                    .into_iter()
                    .map(|e| e.name.clone())
                    .collect(),
                None => session
                    .entity_graph
                    .get_most_important_entities(50)
                    .into_iter()
                    .map(|e| e.name.clone())
                    .collect(),
            };
            Ok(ContextResponse::AllEntities(entities))
        }
        ContextQuery::TraceRelationships {
            from_entity,
            max_depth,
        } => {
            let trace = session
                .entity_graph
                .trace_entity_relationships(&from_entity, max_depth);
            Ok(ContextResponse::Entities(
                trace.into_iter().map(|(a, _, _)| a).collect(),
            ))
        }
        ContextQuery::GetEntityNetwork {
            center_entity,
            max_depth,
        } => {
            let _network = session
                .entity_graph
                .get_entity_network(&center_entity, max_depth);
            Ok(ContextResponse::EntityNetwork("Network data".to_string()))
        }
        ContextQuery::GetMostImportantEntities { limit } => {
            let entities = session.entity_graph.get_most_important_entities(limit);
            Ok(ContextResponse::Entities(
                entities.into_iter().map(|e| e.name.clone()).collect(),
            ))
        }
        ContextQuery::GetRecentlyMentionedEntities { limit } => {
            let entities = session.entity_graph.get_recently_mentioned_entities(limit);
            Ok(ContextResponse::Entities(
                entities.into_iter().map(|e| e.name.clone()).collect(),
            ))
        }
        ContextQuery::AnalyzeEntityImportance => {
            let _analysis = session.entity_graph.analyze_entity_importance();
            Ok(ContextResponse::ImportanceAnalysis(
                "Analysis complete".to_string(),
            ))
        }
        ContextQuery::FindEntitiesByType { entity_type } => {
            let entities = session.entity_graph.get_entities_by_type(&entity_type);
            Ok(ContextResponse::Entities(
                entities.into_iter().map(|e| e.name.clone()).collect(),
            ))
        }
        ContextQuery::SearchUpdates { query } => {
            let update_results: Vec<ContextUpdate> = session
                .hot_context
                .iter()
                .iter()
                .chain(session.warm_context.iter().map(|c| &c.update))
                .filter(|u| {
                    u.content
                        .title
                        .to_lowercase()
                        .contains(&query.to_lowercase())
                        || u.content
                            .description
                            .to_lowercase()
                            .contains(&query.to_lowercase())
                })
                .cloned()
                .collect();
            Ok(ContextResponse::SearchResults(update_results))
        }
        ContextQuery::GetDecisions { since: _ } => {
            let decisions: Vec<ContextUpdate> = session
                .hot_context
                .iter()
                .into_iter()
                .filter(|u| matches!(u.update_type, UpdateType::DecisionMade))
                .collect();
            Ok(ContextResponse::Decisions(decisions))
        }
        ContextQuery::GetOpenQuestions => Ok(ContextResponse::OpenQuestions(vec![
            "No open questions".to_string(),
        ])),
        ContextQuery::GetChangeHistory { file_path: _ } => {
            let changes: Vec<ContextUpdate> = session
                .hot_context
                .iter()
                .into_iter()
                .filter(|u| matches!(u.update_type, UpdateType::CodeChanged))
                .collect();
            Ok(ContextResponse::ChangeHistory(changes))
        }
        ContextQuery::AssembleContext { query, token_budget } => {
            use crate::core::context_assembly;

            // Collect all context updates from hot + warm context
            let updates: Vec<_> = session.hot_context.iter().iter()
                .chain(session.warm_context.iter().map(|c| &c.update))
                .cloned()
                .collect();

            let assembled = context_assembly::assemble_context(
                &query,
                &session.entity_graph,
                &updates,
                token_budget,
            );

            Ok(ContextResponse::AssembledContext(assembled))
        }
        _ => Ok(ContextResponse::Entities(vec![
            "Not implemented".to_string(),
        ])),
    }
}

async fn create_comprehensive_checkpoint(session: &ActiveSession) -> Result<SessionCheckpoint> {
    // Dereference Arc-wrapped fields to get underlying data for checkpoint
    Ok(SessionCheckpoint {
        id: Uuid::new_v4(),
        session_id: session.id(),
        created_at: chrono::Utc::now(),
        structured_context: (*session.current_state).clone(),
        recent_updates: (*session.incremental_updates).clone(),
        code_references: (*session.code_references).clone(),
        change_history: (*session.change_history).clone(),
        total_updates: session.incremental_updates.len(),
        context_quality_score: 1.0,
        compression_ratio: 1.0,
    })
}

fn parse_datetime(date_str: &str) -> Result<chrono::DateTime<chrono::Utc>> {
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

/// Get structured summary of a session using existing data
pub async fn get_structured_summary(
    session_id: String,
    decisions_limit: Option<usize>,
    entities_limit: Option<usize>,
    questions_limit: Option<usize>,
    concepts_limit: Option<usize>,
    min_confidence: Option<f32>,
    compact: Option<bool>,
) -> Result<MCPToolResult> {
    let uuid =
        Uuid::parse_str(&session_id).map_err(|e| anyhow::anyhow!("Invalid session ID: {}", e))?;

    let system = get_memory_system().await?;
    let session_arc = system
        .get_session(uuid)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to load session: {}", e))?;

    let session = session_arc.load();

    // Build summary options
    use crate::summary::SummaryOptions;
    let user_requested_compact = compact.unwrap_or(false);
    const MAX_TOKENS: usize = 50_000;

    // Pre-estimate size to avoid double generation
    let (estimated_tokens, should_compact) =
        SummaryGenerator::estimate_summary_size(&session, MAX_TOKENS);
    let auto_compacted = !user_requested_compact && should_compact;

    if auto_compacted {
        log::info!(
            "Pre-estimated {} tokens > {} max. Using compact mode directly.",
            estimated_tokens,
            MAX_TOKENS
        );
    }

    let options = if user_requested_compact || auto_compacted {
        SummaryOptions::compact()
    } else {
        SummaryOptions {
            decisions_limit,
            entities_limit,
            questions_limit,
            concepts_limit,
            min_confidence,
            compact: false,
        }
    };

    let summary = SummaryGenerator::generate_structured_summary_filtered(&session, &options);

    let message = if user_requested_compact {
        "Generated compact structured summary".to_string()
    } else if auto_compacted {
        format!(
            "Auto-compacted summary (was too large for MCP). Showing: {} decisions, {} entities, {} questions, {} concepts",
            summary.key_decisions.len(),
            summary.entity_summaries.len(),
            summary.open_questions.len(),
            summary.key_concepts.len()
        )
    } else {
        format!(
            "Generated structured summary (decisions: {}, entities: {}, questions: {}, concepts: {})",
            summary.key_decisions.len(),
            summary.entity_summaries.len(),
            summary.open_questions.len(),
            summary.key_concepts.len()
        )
    };

    Ok(MCPToolResult::success(
        message,
        Some(serde_json::to_value(summary)?),
    ))
}

/// Get key decisions from a session
pub async fn get_key_decisions(session_id: String) -> Result<MCPToolResult> {
    let uuid =
        Uuid::parse_str(&session_id).map_err(|e| anyhow::anyhow!("Invalid session ID: {}", e))?;

    let system = get_memory_system().await?;
    let session_arc = system
        .get_session(uuid)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to load session: {}", e))?;

    let session = session_arc.load();
    let decisions = SummaryGenerator::extract_decision_timeline(&session);

    Ok(MCPToolResult::success(
        format!("Found {} key decisions", decisions.len()),
        Some(serde_json::to_value(decisions)?),
    ))
}

/// Get key insights from session data
pub async fn get_key_insights(session_id: String, limit: Option<usize>) -> Result<MCPToolResult> {
    let uuid =
        Uuid::parse_str(&session_id).map_err(|e| anyhow::anyhow!("Invalid session ID: {}", e))?;

    let system = get_memory_system().await?;
    let session_arc = system
        .get_session(uuid)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to load session: {}", e))?;

    let session = session_arc.load();
    let insights = SummaryGenerator::extract_key_insights(&session, limit.unwrap_or(5));

    Ok(MCPToolResult::success(
        format!("Found {} key insights", insights.len()),
        Some(serde_json::to_value(insights)?),
    ))
}

/// Get entity importance analysis from existing data
pub async fn get_entity_importance_analysis(
    session_id: String,
    limit: Option<usize>,
    min_importance: Option<f32>,
) -> Result<MCPToolResult> {
    let uuid =
        Uuid::parse_str(&session_id).map_err(|e| anyhow::anyhow!("Invalid session ID: {}", e))?;

    let system = get_memory_system().await?;
    let session_arc = system
        .get_session(uuid)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to load session: {}", e))?;

    let session = session_arc.load();
    let mut analysis = session.entity_graph.analyze_entity_importance();

    let total_entities = analysis.len();

    // Apply min_importance filter if specified
    if let Some(min_imp) = min_importance {
        analysis.retain(|entity| entity.importance_score >= min_imp);
    }

    let after_filter = analysis.len();

    // Apply limit if specified (default: 100 for manageable output)
    let entity_limit = limit.unwrap_or(100);
    let truncated = analysis.len() > entity_limit;
    if truncated {
        analysis.truncate(entity_limit);
    }

    // Add pagination metadata
    let result = serde_json::json!({
        "entities": analysis,
        "pagination": {
            "total_entities": total_entities,
            "after_filter": after_filter,
            "returned": analysis.len(),
            "limit": entity_limit,
            "min_importance_threshold": min_importance,
            "truncated": truncated
        }
    });

    Ok(MCPToolResult::success(
        "Generated entity importance analysis with pagination".to_string(),
        Some(result),
    ))
}

/// Get entity network view centered on an entity with pagination
pub async fn get_entity_network_view(
    session_id: String,
    center_entity: Option<String>,
    max_entities: Option<usize>,
    max_relationships: Option<usize>,
) -> Result<MCPToolResult> {
    let uuid =
        Uuid::parse_str(&session_id).map_err(|e| anyhow::anyhow!("Invalid session ID: {}", e))?;

    let system = get_memory_system().await?;
    let session_arc = system
        .get_session(uuid)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to load session: {}", e))?;

    let session = session_arc.load();
    let mut network = match center_entity {
        Some(entity) => session.entity_graph.get_entity_network(&entity, 2),
        None => {
            let top_entities = session.entity_graph.get_most_important_entities(1);
            if let Some(top) = top_entities.first() {
                session.entity_graph.get_entity_network(&top.name, 2)
            } else {
                return Ok(MCPToolResult::error(
                    "No entities found in session".to_string(),
                ));
            }
        }
    };

    // Apply pagination limits
    let max_entities_limit = max_entities.unwrap_or(50); // Default: 50 entities
    let max_relationships_limit = max_relationships.unwrap_or(100); // Default: 100 relationships

    let total_entities = network.entities.len();
    let total_relationships = network.relationships.len();

    // Truncate entities if needed (keep most important ones based on importance_score)
    if network.entities.len() > max_entities_limit {
        let mut sorted_entities: Vec<_> = network.entities.iter().collect();
        sorted_entities.sort_by(|a, b| {
            b.1.importance_score
                .partial_cmp(&a.1.importance_score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        let kept_entities: std::collections::HashSet<String> = sorted_entities
            .into_iter()
            .take(max_entities_limit)
            .map(|(name, _)| name.clone())
            .collect();

        // Filter entities
        network
            .entities
            .retain(|name, _| kept_entities.contains(name));

        // Filter relationships to only include kept entities
        network.relationships.retain(|rel| {
            kept_entities.contains(&rel.from_entity) && kept_entities.contains(&rel.to_entity)
        });
    }

    // Truncate relationships if still needed
    if network.relationships.len() > max_relationships_limit {
        network.relationships.truncate(max_relationships_limit);
    }

    // Add pagination metadata
    let mut result = serde_json::to_value(network)?;

    // Calculate returned counts before taking mutable reference
    let returned_entities = result["entities"].as_object().map(|o| o.len()).unwrap_or(0);
    let returned_relationships = result["relationships"]
        .as_array()
        .map(|a| a.len())
        .unwrap_or(0);

    if let Some(obj) = result.as_object_mut() {
        obj.insert(
            "pagination".to_string(),
            serde_json::json!({
                "total_entities": total_entities,
                "total_relationships": total_relationships,
                "returned_entities": returned_entities,
                "returned_relationships": returned_relationships,
                "max_entities_limit": max_entities_limit,
                "max_relationships_limit": max_relationships_limit,
                "truncated": total_entities > max_entities_limit || total_relationships > max_relationships_limit
            }),
        );
    }

    Ok(MCPToolResult::success(
        "Generated entity network view with pagination".to_string(),
        Some(result),
    ))
}

/// Get session statistics from existing data
pub async fn get_session_statistics(session_id: String) -> Result<MCPToolResult> {
    let uuid =
        Uuid::parse_str(&session_id).map_err(|e| anyhow::anyhow!("Invalid session ID: {}", e))?;

    let system = get_memory_system().await?;
    let session_arc = system
        .get_session(uuid)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to load session: {}", e))?;

    let session = session_arc.load();
    // Use lightweight stats calculation instead of full summary generation
    let stats = SummaryGenerator::calculate_session_stats(&session);

    Ok(MCPToolResult::success(
        "Generated session statistics".to_string(),
        Some(serde_json::to_value(stats)?),
    ))
}

/// Get comprehensive catalog of all available tools organized by category
pub async fn get_tool_catalog() -> Result<MCPToolResult> {
    let catalog = serde_json::json!({
        "total_tools": 26,
        "categories": {
            "Session Management": {
                "description": "Tools for creating, loading, and managing conversation sessions",
                "tool_count": 5,
                "tools": [
                    {
                        "name": "create_session",
                        "description": "Start new conversation session with optional name/description",
                        "use_when": "Beginning a new project, conversation, or knowledge context"
                    },
                    {
                        "name": "load_session",
                        "description": "Resume existing session into active memory",
                        "use_when": "Continuing work on a previous conversation or project"
                    },
                    {
                        "name": "list_sessions",
                        "description": "View all sessions with metadata and statistics",
                        "use_when": "Finding available sessions or checking session details"
                    },
                    {
                        "name": "search_sessions",
                        "description": "Find sessions by name or description (text search)",
                        "use_when": "Looking for specific sessions when you know the name/topic"
                    },
                    {
                        "name": "update_session_metadata",
                        "description": "Change session name or description",
                        "use_when": "Renaming or updating session information"
                    }
                ]
            },
            "Context Operations": {
                "description": "Tools for adding and querying conversation knowledge",
                "tool_count": 3,
                "tools": [
                    {
                        "name": "update_conversation_context",
                        "description": "Add knowledge: QA, decisions, problems solved, code changes",
                        "use_when": "Storing information for future retrieval and learning",
                        "interaction_types": ["qa", "decision_made", "problem_solved", "code_change", "requirement_added", "concept_defined"]
                    },
                    {
                        "name": "query_conversation_context",
                        "description": "Search using entities, keywords, or structured queries",
                        "use_when": "Finding exact entities or keyword-based searches (faster than semantic)",
                        "query_types": ["find_related_entities", "get_entity_context", "search_updates", "get_most_important_entities"]
                    },
                    {
                        "name": "create_session_checkpoint",
                        "description": "Create snapshot of current session state",
                        "use_when": "Creating restore points before major changes"
                    }
                ]
            },
            "Semantic Search": {
                "description": "AI-powered conceptual search using embeddings (requires embeddings feature)",
                "tool_count": 5,
                "tools": [
                    {
                        "name": "semantic_search_session",
                        "description": "AI search within session - auto-loads and auto-vectorizes!",
                        "use_when": "Finding information by concept/meaning, not exact keywords",
                        "note": "Automatically vectorizes on first use - no manual setup needed"
                    },
                    {
                        "name": "semantic_search_global",
                        "description": "AI search across ALL sessions",
                        "use_when": "Finding related knowledge across multiple conversations"
                    },
                    {
                        "name": "find_related_content",
                        "description": "Discover connections between different sessions",
                        "use_when": "Finding similar discussions from other conversations"
                    },
                    {
                        "name": "vectorize_session",
                        "description": "Generate embeddings for semantic search (optional - auto-called)",
                        "use_when": "Rarely needed - semantic_search_session auto-vectorizes",
                        "note": "Only useful for batch processing or manual control"
                    },
                    {
                        "name": "get_vectorization_stats",
                        "description": "Check embedding statistics and performance metrics",
                        "use_when": "Debugging or monitoring semantic search system"
                    }
                ]
            },
            "Analysis & Insights": {
                "description": "Tools for extracting insights, summaries, and analyzing session data",
                "tool_count": 7,
                "tools": [
                    {
                        "name": "get_structured_summary",
                        "description": "Comprehensive summary with auto-compact for large sessions",
                        "use_when": "Getting overview of decisions, entities, questions, concepts",
                        "note": "Auto-compacts if response > 25K tokens - prevents MCP overflow"
                    },
                    {
                        "name": "get_key_decisions",
                        "description": "Timeline of decisions with confidence levels",
                        "use_when": "Reviewing architectural choices and their rationale"
                    },
                    {
                        "name": "get_key_insights",
                        "description": "Extract top insights from session data",
                        "use_when": "Quick overview of most important discoveries"
                    },
                    {
                        "name": "get_entity_importance_analysis",
                        "description": "Entity rankings with mention counts and relationships",
                        "use_when": "Understanding which concepts/technologies are most central"
                    },
                    {
                        "name": "get_entity_network_view",
                        "description": "Visualize entity relationships as network graph",
                        "use_when": "Understanding how concepts connect to each other"
                    },
                    {
                        "name": "get_session_statistics",
                        "description": "Session metrics: updates, entities, activity level, duration",
                        "use_when": "Checking session health and growth metrics"
                    },
                    {
                        "name": "get_tool_catalog",
                        "description": "View all 26 tools organized by category with usage guidance",
                        "use_when": "First time using post-cortex or discovering available tools",
                        "note": "Returns categories, workflows, tips, and most-used tools list"
                    }
                ]
            },
            "Workspace Management": {
                "description": "Tools for organizing related sessions (e.g., microservices, monorepo projects)",
                "tool_count": 6,
                "tools": [
                    {
                        "name": "create_workspace",
                        "description": "Create workspace to group related sessions",
                        "use_when": "Starting a multi-service project or organizing session groups"
                    },
                    {
                        "name": "get_workspace",
                        "description": "Retrieve workspace details including all sessions and metadata",
                        "use_when": "Viewing workspace configuration and session associations"
                    },
                    {
                        "name": "list_workspaces",
                        "description": "List all workspaces with session counts",
                        "use_when": "Finding available workspaces or checking workspace overview"
                    },
                    {
                        "name": "delete_workspace",
                        "description": "Delete workspace (sessions remain intact)",
                        "use_when": "Removing workspace organization without deleting sessions"
                    },
                    {
                        "name": "add_session_to_workspace",
                        "description": "Add session to workspace with role (primary/related/dependency/shared)",
                        "use_when": "Organizing session into workspace with specific relationship role"
                    },
                    {
                        "name": "remove_session_from_workspace",
                        "description": "Remove session from workspace",
                        "use_when": "Reorganizing sessions or removing from workspace group"
                    }
                ]
            }
        },
        "getting_started": [
            "1. create_session → get session_id",
            "2. update_conversation_context → add knowledge (qa, decisions, problems, code)",
            "3. semantic_search_session → AI-powered search (auto-vectorizes on first use!)",
            "4. get_structured_summary → comprehensive overview"
        ],
        "most_used_tools": [
            "create_session",
            "load_session",
            "update_conversation_context",
            "semantic_search_session",
            "get_structured_summary"
        ],
        "tips": [
            "💡 Semantic search auto-vectorizes - no manual vectorize_session needed!",
            "💡 Use semantic_search for concepts, query_conversation_context for keywords",
            "💡 get_structured_summary has auto-compact - safe for large sessions",
            "💡 Similarity scores: 0.65-0.75 = excellent, 0.45-0.55 = good, <0.30 = weak",
            "💡 All sessions persist across Claude Code conversations"
        ]
    });

    Ok(MCPToolResult::success(
        "Retrieved tool catalog with 26 tools across 5 categories".to_string(),
        Some(catalog),
    ))
}

// Semantic Search and Embeddings Tools (requires embeddings feature)

/// Helper function to map interaction type strings to ContentType enums
#[cfg(feature = "embeddings")]
fn interaction_type_to_content_type(interaction_type: &str) -> Option<ContentType> {
    match interaction_type {
        "qa" => Some(ContentType::UserMessage),
        "code_change" => Some(ContentType::CodeSnippet),
        "decision_made" => Some(ContentType::DecisionPoint),
        "problem_solved" => Some(ContentType::ProblemSolution),
        "requirement_added" => Some(ContentType::UpdateContent),
        "concept_defined" => Some(ContentType::EntityDescription),
        _ => None,
    }
}

/// Perform semantic search across all sessions
#[cfg(feature = "embeddings")]
pub async fn semantic_search_global(
    query: String,
    limit: Option<usize>,
    date_from: Option<String>,
    date_to: Option<String>,
    interaction_type: Option<Vec<String>>,
    recency_bias: Option<f32>,
) -> Result<MCPToolResult> {
    info!(
        "MCP-TOOLS: semantic_search_global() called with query: '{}' and recency_bias: {:?}",
        query, recency_bias
    );
    let system = get_memory_system().await?;
    info!("MCP-TOOLS: Got memory system for semantic_search_global");

    if !system.config.enable_embeddings {
        return Ok(MCPToolResult::error(
            "Embeddings not enabled or initialized".to_string(),
        ));
    }

    // Parse date range if provided
    let date_range = match (date_from, date_to) {
        (Some(from_str), Some(to_str)) => {
            let from = chrono::DateTime::parse_from_rfc3339(&from_str)
                .map_err(|e| anyhow::anyhow!("Invalid date_from format: {}", e))?
                .with_timezone(&chrono::Utc);
            let to = chrono::DateTime::parse_from_rfc3339(&to_str)
                .map_err(|e| anyhow::anyhow!("Invalid date_to format: {}", e))?
                .with_timezone(&chrono::Utc);
            Some((from, to))
        }
        (Some(_), None) | (None, Some(_)) => {
            return Ok(MCPToolResult::error(
                "Both date_from and date_to must be provided together".to_string(),
            ));
        }
        (None, None) => None,
    };

    // When interaction_type filter is provided, request more results to ensure
    // we get enough results of the desired ContentType after filtering
    // (EntityDescription with high similarity often dominates top results)
    let search_limit = if interaction_type.is_some() {
        // Request 10x more results to account for ContentType filtering
        Some(limit.unwrap_or(10) * 10)
    } else {
        limit
    };

    // Use system's semantic_search_global which uses the shared vectorizer
    let search_results = system
        .semantic_search_global(&query, search_limit, date_range, recency_bias)
        .await
        .map_err(|e| anyhow::anyhow!("Semantic search failed: {}", e))?;

    let mut results = search_results;
    let results_before_filter = results.len();

    // Filter by interaction_type if provided
    if let Some(ref types) = interaction_type {
        info!("Filtering by interaction_types: {:?}", types);

        // Convert interaction types to ContentType
        let content_types: Vec<ContentType> = types
            .iter()
            .filter_map(|t| {
                let ct = interaction_type_to_content_type(t);
                info!("Mapping '{}' -> {:?}", t, ct);
                ct
            })
            .collect();

        info!("Mapped to ContentTypes: {:?}", content_types);

        if !content_types.is_empty() {
            results.retain(|r| content_types.contains(&r.content_type));
            info!(
                "Filtered from {} to {} results",
                results_before_filter,
                results.len()
            );

            // Trim to original limit after filtering
            let final_limit = limit.unwrap_or(10);
            if results.len() > final_limit {
                results.truncate(final_limit);
                info!("Truncated to {} results after filtering", final_limit);
            }
        } else {
            info!("No valid ContentTypes mapped, skipping filter");
        }
    } else {
        info!("No interaction_type filter provided");
    }

    // Build formatted message with full results
    let mut message = format!(
        "Found {} global semantic search results for query: '{}'\n\n",
        results.len(),
        query
    );

    for (idx, r) in results.iter().enumerate() {
        message.push_str(&format!(
            "{}. [Session: {}] [{:?}] Similarity: {:.1}% | Relevance: {:.1}% ({})\n",
            idx + 1,
            &r.session_id.to_string(),
            r.content_type,
            r.similarity_score * 100.0,
            r.combined_score * 100.0,
            r.similarity_quality()
        ));
        message.push_str(&format!(
            "   Time: {}\n",
            r.timestamp.format("%Y-%m-%d %H:%M:%S")
        ));
        message.push_str(&format!("   {}\n", r.score_explanation()));

        // Truncate text_content if too long (UTF-8 safe)
        let content = if r.text_content.chars().count() > 500 {
            let truncated: String = r.text_content.chars().take(500).collect();
            format!("{}...", truncated)
        } else {
            r.text_content.clone()
        };
        message.push_str(&format!("   Content: {}\n\n", content));
    }

    // Also build JSON for data field
    let search_results: Vec<serde_json::Value> = results
        .into_iter()
        .map(|r| {
            serde_json::json!({
                "content_id": r.content_id,
                "session_id": r.session_id.to_string(),
                "content_type": format!("{:?}", r.content_type),
                "text_content": r.text_content,
                "similarity_score": r.similarity_score,
                "similarity_quality": r.similarity_quality(),
                "importance_score": r.importance_score,
                "timestamp": r.timestamp.to_rfc3339(),
                "combined_score": r.combined_score,
                "score_explanation": r.score_explanation()
            })
        })
        .collect();

    Ok(MCPToolResult::success(
        message,
        Some(serde_json::json!({
            "query": query,
            "results": search_results,
            "scoring_info": {
                "algorithm": "combined_score = (similarity_score × 0.7) + (importance_weight × 0.3)",
                "quality_levels": {
                    "Excellent": "≥ 0.85",
                    "Very Good": "0.70 - 0.84",
                    "Good": "0.55 - 0.69",
                    "Moderate": "0.40 - 0.54",
                    "Fair": "0.30 - 0.39",
                    "Weak": "< 0.30"
                }
            }
        })),
    ))
}

/// Perform semantic search within a specific session
#[cfg(feature = "embeddings")]
pub async fn semantic_search_session(
    session_id: Uuid,
    query: String,
    limit: Option<usize>,
    date_from: Option<String>,
    date_to: Option<String>,
    interaction_type: Option<Vec<String>>,
    recency_bias: Option<f32>,
) -> Result<MCPToolResult> {
    info!(
        "MCP-TOOLS: semantic_search_session() called for session {} with query: '{}' and recency_bias: {:?}",
        session_id, query, recency_bias
    );
    let system = get_memory_system().await?;
    info!("MCP-TOOLS: Got memory system for semantic_search_session");

    if !system.config.enable_embeddings {
        return Ok(MCPToolResult::error(
            "Embeddings not enabled or initialized".to_string(),
        ));
    }

    // Parse date range if provided
    let date_range = match (date_from, date_to) {
        (Some(from_str), Some(to_str)) => {
            let from = chrono::DateTime::parse_from_rfc3339(&from_str)
                .map_err(|e| anyhow::anyhow!("Invalid date_from format: {}", e))?
                .with_timezone(&chrono::Utc);
            let to = chrono::DateTime::parse_from_rfc3339(&to_str)
                .map_err(|e| anyhow::anyhow!("Invalid date_to format: {}", e))?
                .with_timezone(&chrono::Utc);
            Some((from, to))
        }
        (Some(_), None) | (None, Some(_)) => {
            return Ok(MCPToolResult::error(
                "Both date_from and date_to must be provided together".to_string(),
            ));
        }
        (None, None) => None,
    };

    // When interaction_type filter is provided, request more results to ensure
    // we get enough results of the desired ContentType after filtering
    // (EntityDescription with high similarity often dominates top results)
    let search_limit = if interaction_type.is_some() {
        // Request 10x more results to account for ContentType filtering
        Some(limit.unwrap_or(10) * 10)
    } else {
        limit
    };

    // Use system's semantic_search_session which uses the shared vectorizer
    let search_results = system
        .semantic_search_session(session_id, &query, search_limit, date_range, recency_bias)
        .await
        .map_err(|e| anyhow::anyhow!("Semantic search failed: {}", e))?;

    let mut results = search_results;
    let results_before_filter = results.len();

    // Filter by interaction_type if provided
    if let Some(ref types) = interaction_type {
        info!("Filtering by interaction_types: {:?}", types);

        // Convert interaction types to ContentType
        let content_types: Vec<ContentType> = types
            .iter()
            .filter_map(|t| {
                let ct = interaction_type_to_content_type(t);
                info!("Mapping '{}' -> {:?}", t, ct);
                ct
            })
            .collect();

        info!("Mapped to ContentTypes: {:?}", content_types);

        if !content_types.is_empty() {
            results.retain(|r| content_types.contains(&r.content_type));
            info!(
                "Filtered from {} to {} results",
                results_before_filter,
                results.len()
            );

            // Trim to original limit after filtering
            let final_limit = limit.unwrap_or(10);
            if results.len() > final_limit {
                results.truncate(final_limit);
                info!("Truncated to {} results after filtering", final_limit);
            }
        } else {
            info!("No valid ContentTypes mapped, skipping filter");
        }
    } else {
        info!("No interaction_type filter provided");
    }

    // Build formatted message with full results
    let mut message = format!(
        "Found {} semantic search results for query: '{}'\n\n",
        results.len(),
        query
    );

    for (idx, r) in results.iter().enumerate() {
        message.push_str(&format!(
            "{}. [{:?}] Similarity: {:.1}% | Relevance: {:.1}% ({})\n",
            idx + 1,
            r.content_type,
            r.similarity_score * 100.0,
            r.combined_score * 100.0,
            r.similarity_quality()
        ));
        message.push_str(&format!(
            "   Time: {}\n",
            r.timestamp.format("%Y-%m-%d %H:%M:%S")
        ));
        message.push_str(&format!("   {}\n", r.score_explanation()));

        // Truncate text_content if too long (UTF-8 safe)
        let content = if r.text_content.chars().count() > 500 {
            let truncated: String = r.text_content.chars().take(500).collect();
            format!("{}...", truncated)
        } else {
            r.text_content.clone()
        };
        message.push_str(&format!("   Content: {}\n\n", content));
    }

    // Also build JSON for data field
    let search_results: Vec<serde_json::Value> = results
        .into_iter()
        .map(|r| {
            serde_json::json!({
                "content_id": r.content_id,
                "session_id": r.session_id.to_string(),
                "content_type": format!("{:?}", r.content_type),
                "text_content": r.text_content,
                "similarity_score": r.similarity_score,
                "similarity_quality": r.similarity_quality(),
                "importance_score": r.importance_score,
                "timestamp": r.timestamp.to_rfc3339(),
                "combined_score": r.combined_score,
                "score_explanation": r.score_explanation()
            })
        })
        .collect();

    Ok(MCPToolResult::success(
        message,
        Some(serde_json::json!({
            "session_id": session_id.to_string(),
            "query": query,
            "results": search_results,
            "scoring_info": {
                "algorithm": "combined_score = (similarity_score × 0.7) + (importance_weight × 0.3)",
                "quality_levels": {
                    "Excellent": "≥ 0.85",
                    "Very Good": "0.70 - 0.84",
                    "Good": "0.55 - 0.69",
                    "Moderate": "0.40 - 0.54",
                    "Fair": "0.30 - 0.39",
                    "Weak": "< 0.30"
                }
            }
        })),
    ))
}

/// Find related content across sessions
#[cfg(feature = "embeddings")]
pub async fn find_related_content(
    session_id: Uuid,
    topic: String,
    limit: Option<usize>,
) -> Result<MCPToolResult> {
    info!(
        "MCP-TOOLS: find_related_content() called for session {} with topic: '{}'",
        session_id, topic
    );
    let system = get_memory_system().await?;
    info!("MCP-TOOLS: Got memory system for find_related_content");

    if !system.config.enable_embeddings {
        return Ok(MCPToolResult::error(
            "Embeddings not enabled or initialized".to_string(),
        ));
    }

    match system.find_related_content(session_id, &topic, limit).await {
        Ok(results) => {
            // Build formatted message with full results
            let mut message = format!(
                "Found {} related content items for topic: '{}'\n\n",
                results.len(),
                topic
            );

            for (idx, r) in results.iter().enumerate() {
                message.push_str(&format!(
                    "{}. [{:?}] Score: {:.3}\n",
                    idx + 1,
                    r.content_type,
                    r.combined_score
                ));
                message.push_str(&format!(
                    "   Time: {}\n",
                    r.timestamp.format("%Y-%m-%d %H:%M:%S")
                ));

                // Truncate text_content if too long (UTF-8 safe)
                let content = if r.text_content.chars().count() > 500 {
                    let truncated: String = r.text_content.chars().take(500).collect();
                    format!("{}...", truncated)
                } else {
                    r.text_content.clone()
                };
                message.push_str(&format!("   Content: {}\n\n", content));
            }

            // Also build JSON for data field
            let related_content: Vec<serde_json::Value> = results
                .into_iter()
                .map(|r| {
                    serde_json::json!({
                        "content_id": r.content_id,
                        "session_id": r.session_id.to_string(),
                        "content_type": format!("{:?}", r.content_type),
                        "text_content": r.text_content,
                        "similarity_score": r.similarity_score,
                        "importance_score": r.importance_score,
                        "timestamp": r.timestamp.to_rfc3339(),
                        "combined_score": r.combined_score
                    })
                })
                .collect();

            Ok(MCPToolResult::success(
                message,
                Some(serde_json::json!({
                    "session_id": session_id.to_string(),
                    "topic": topic,
                    "related_content": related_content
                })),
            ))
        }
        Err(e) => Ok(MCPToolResult::error(format!(
            "Related content search failed: {}",
            e
        ))),
    }
}

/// Vectorize a session's content
#[cfg(feature = "embeddings")]
pub async fn vectorize_session(session_id: Uuid) -> Result<MCPToolResult> {
    info!(
        "MCP-TOOLS: vectorize_session() called for session {}",
        session_id
    );
    let system = get_memory_system().await?;
    info!("MCP-TOOLS: Got memory system for vectorize_session");

    // Check if embeddings are enabled in config (lazy init will happen automatically)
    if !system.config.enable_embeddings {
        return Ok(MCPToolResult::error(
            "Embeddings not enabled in configuration".to_string(),
        ));
    }

    match system.vectorize_session(session_id).await {
        Ok(count) => Ok(MCPToolResult::success(
            format!("Successfully vectorized {} items", count),
            Some(serde_json::json!({
                "session_id": session_id.to_string(),
                "vectorized_count": count
            })),
        )),
        Err(e) => Ok(MCPToolResult::error(format!("Vectorization failed: {}", e))),
    }
}

/// Get vectorization statistics
#[cfg(feature = "embeddings")]
pub async fn get_vectorization_stats() -> Result<MCPToolResult> {
    info!("MCP-TOOLS: get_vectorization_stats() called");
    let system = get_memory_system().await?;
    info!("MCP-TOOLS: Got memory system for get_vectorization_stats");

    if !system.config.enable_embeddings {
        return Ok(MCPToolResult::error(
            "Embeddings not enabled or initialized".to_string(),
        ));
    }

    match system.get_vectorization_stats() {
        Ok(stats) => Ok(MCPToolResult::success(
            "Retrieved vectorization statistics".to_string(),
            Some(serde_json::json!({
                "stats": stats
            })),
        )),
        Err(e) => Ok(MCPToolResult::error(format!("Failed to get stats: {}", e))),
    }
}

/// Enable embeddings configuration
pub async fn enable_embeddings(model_type: Option<String>) -> Result<MCPToolResult> {
    if !cfg!(feature = "embeddings") {
        return Ok(MCPToolResult::error(
            "Embeddings feature not compiled in. Please rebuild with --features embeddings"
                .to_string(),
        ));
    }

    // Note: This would require restarting the system to take effect
    // For now, we return information about embeddings support
    Ok(MCPToolResult::success(
        "Embeddings feature is available".to_string(),
        Some(serde_json::json!({
            "embeddings_compiled": cfg!(feature = "embeddings"),
            "available_models": ["StaticSimilarityMRL", "MiniLM", "TinyBERT", "BGESmall"],
            "default_model": model_type.unwrap_or_else(|| "StaticSimilarityMRL".to_string()),
            "note": "Embeddings must be enabled in system configuration and requires restart"
        })),
    ))
}

// Non-embeddings fallback versions for when embeddings feature is disabled
#[cfg(not(feature = "embeddings"))]
pub async fn semantic_search_global(
    _query: String,
    _limit: Option<usize>,
    _date_from: Option<String>,
    _date_to: Option<String>,
    _interaction_type: Option<Vec<String>>,
    _recency_bias: Option<f32>,
) -> Result<MCPToolResult> {
    Ok(MCPToolResult::error(
        "Semantic search requires the 'embeddings' feature to be enabled. Please rebuild with --features embeddings".to_string()
    ))
}

#[cfg(not(feature = "embeddings"))]
pub async fn semantic_search_session(
    _session_id: Uuid,
    _query: String,
    _limit: Option<usize>,
    _date_from: Option<String>,
    _date_to: Option<String>,
    _interaction_type: Option<Vec<String>>,
    _recency_bias: Option<f32>,
) -> Result<MCPToolResult> {
    Ok(MCPToolResult::error(
        "Semantic search requires the 'embeddings' feature to be enabled. Please rebuild with --features embeddings".to_string()
    ))
}

#[cfg(not(feature = "embeddings"))]
pub async fn find_related_content(
    _session_id: Uuid,
    _topic: String,
    _limit: Option<usize>,
) -> Result<MCPToolResult> {
    Ok(MCPToolResult::error(
        "Related content search requires the 'embeddings' feature to be enabled. Please rebuild with --features embeddings".to_string()
    ))
}

#[cfg(not(feature = "embeddings"))]
pub async fn vectorize_session(_session_id: Uuid) -> Result<MCPToolResult> {
    Ok(MCPToolResult::error(
        "Vectorization requires the 'embeddings' feature to be enabled. Please rebuild with --features embeddings".to_string()
    ))
}

#[cfg(not(feature = "embeddings"))]
pub async fn get_vectorization_stats() -> Result<MCPToolResult> {
    Ok(MCPToolResult::error(
        "Vectorization stats require the 'embeddings' feature to be enabled. Please rebuild with --features embeddings".to_string()
    ))
}

/// Get all MCP tool schemas for tools/list response
/// This function returns the tool definitions in MCP protocol format
pub fn get_all_tool_schemas() -> Vec<serde_json::Value> {
    use serde_json::json;

    vec![
        // Session Management (5 tools)
        json!({
            "name": "create_session",
            "description": "Create a new conversation session with optional name and description",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "name": {"type": "string", "description": "Optional name for the session"},
                    "description": {"type": "string", "description": "Optional description for the session"}
                }
            }
        }),
        json!({
            "name": "load_session",
            "description": "Load an existing session into active memory",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "session_id": {"type": "string", "description": "UUID of the session to load"}
                },
                "required": ["session_id"]
            }
        }),
        json!({
            "name": "list_sessions",
            "description": "List all available conversation sessions",
            "inputSchema": {
                "type": "object"
            }
        }),
        json!({
            "name": "search_sessions",
            "description": "Search for sessions by name or description",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": {"type": "string", "description": "Search query string"}
                },
                "required": ["query"]
            }
        }),
        json!({
            "name": "update_session_metadata",
            "description": "Update session name or description",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "session_id": {"type": "string", "description": "UUID of the session"},
                    "name": {"type": "string", "description": "New name (optional)"},
                    "description": {"type": "string", "description": "New description (optional)"}
                },
                "required": ["session_id"]
            }
        }),
        // Context Operations (3 tools)
        json!({
            "name": "update_conversation_context",
            "description": "Add new knowledge to the conversation: QA, decisions, problems solved, code changes, requirements, or concepts. Content fields vary by interaction_type: qa={question,answer}, decision_made={decision,rationale}, problem_solved={problem,solution}, code_change={file_path|description,changes|diff}, requirement_added={requirement,priority}, concept_defined={concept,definition}. Extra fields become details.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "session_id": {"type": "string", "description": "UUID of the session"},
                    "interaction_type": {"type": "string", "description": "Type: qa, decision_made, problem_solved, code_change, requirement_added, concept_defined"},
                    "content": {
                        "type": "object",
                        "description": "Content object. Expected fields by type: qa={question,answer}, decision_made={decision,rationale}, problem_solved={problem,solution}, code_change={file_path,changes}, requirement_added={requirement,priority}, concept_defined={concept,definition}. Accepts variations: question/title/query, answer/response, problem/issue/bug, solution/fix, decision/choice, rationale/reason/why, file_path/file/description, changes/diff/changes_made. Extra fields become details array."
                    },
                    "code_reference": {"type": "object", "description": "Optional code reference with file paths and line numbers"}
                },
                "required": ["session_id", "interaction_type", "content"]
            }
        }),
        json!({
            "name": "query_conversation_context",
            "description": "Search conversation context using entities, keywords, or structured queries",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "session_id": {"type": "string", "description": "UUID of the session"},
                    "query_type": {"type": "string", "description": "Query type: find_related_entities, get_entity_context, search_updates, get_most_important_entities"},
                    "parameters": {"type": "object", "description": "Query-specific parameters"}
                },
                "required": ["session_id", "query_type", "parameters"]
            }
        }),
        json!({
            "name": "bulk_update_conversation_context",
            "description": "Add multiple context updates in a single operation for better performance",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "session_id": {"type": "string", "description": "UUID of the session"},
                    "updates": {"type": "array", "description": "Array of context update objects"}
                },
                "required": ["session_id", "updates"]
            }
        }),
        // Semantic Search (5 tools - all available regardless of embeddings feature)
        json!({
            "name": "semantic_search_session",
            "description": "Search within a session using AI semantic understanding (auto-vectorizes on first use)",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "session_id": {"type": "string", "description": "UUID of the session"},
                    "query": {"type": "string", "description": "Natural language search query"},
                    "limit": {"type": "number", "description": "Max results (default: 10)"},
                    "date_from": {"type": "string", "description": "Filter from date (ISO 8601)"},
                    "date_to": {"type": "string", "description": "Filter to date (ISO 8601)"},
                    "interaction_type": {"type": "array", "description": "Filter by interaction types"},
                    "recency_bias": {
                        "type": "number",
                        "description": "Temporal decay factor: 0.0=disabled (default), 0.1-0.5=soft bias, 1.0+=aggressive bias toward recent content",
                        "default": 0.0,
                        "minimum": 0.0,
                        "maximum": 10.0
                    }
                },
                "required": ["session_id", "query"]
            }
        }),
        json!({
            "name": "semantic_search_global",
            "description": "Search across all sessions using AI semantic understanding",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": {"type": "string", "description": "Natural language search query"},
                    "limit": {"type": "number", "description": "Max results (default: 10)"},
                    "date_from": {"type": "string", "description": "Filter from date (ISO 8601)"},
                    "date_to": {"type": "string", "description": "Filter to date (ISO 8601)"},
                    "interaction_type": {"type": "array", "description": "Filter by interaction types"},
                    "recency_bias": {
                        "type": "number",
                        "description": "Temporal decay factor: 0.0=disabled (default), 0.1-0.5=soft bias, 1.0+=aggressive bias toward recent content",
                        "default": 0.0,
                        "minimum": 0.0,
                        "maximum": 10.0
                    }
                },
                "required": ["query"]
            }
        }),
        json!({
            "name": "find_related_content",
            "description": "Find content related to a specific topic within a session",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "session_id": {"type": "string", "description": "UUID of the session"},
                    "topic": {"type": "string", "description": "Topic to find related content for"},
                    "limit": {"type": "number", "description": "Max results (default: 10)"}
                },
                "required": ["session_id", "topic"]
            }
        }),
        json!({
            "name": "vectorize_session",
            "description": "Manually trigger vectorization for a session (usually happens automatically)",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "session_id": {"type": "string", "description": "UUID of the session to vectorize"}
                },
                "required": ["session_id"]
            }
        }),
        json!({
            "name": "get_vectorization_stats",
            "description": "Get statistics about vectorization status and HNSW index",
            "inputSchema": {
                "type": "object"
            }
        }),
        // Analysis & Insights (6 tools)
        json!({
            "name": "get_structured_summary",
            "description": "Get comprehensive session overview with decisions, entities, questions, and concepts",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "session_id": {"type": "string", "description": "UUID of the session"},
                    "compact": {"type": "boolean", "description": "Use compact format for large sessions"},
                    "decisions_limit": {"type": "number", "description": "Max decisions to return"},
                    "entities_limit": {"type": "number", "description": "Max entities to return"},
                    "questions_limit": {"type": "number", "description": "Max questions to return"},
                    "concepts_limit": {"type": "number", "description": "Max concepts to return"},
                    "min_confidence": {"type": "number", "description": "Minimum confidence threshold (0.0-1.0)"}
                },
                "required": ["session_id"]
            }
        }),
        json!({
            "name": "get_key_decisions",
            "description": "Get timeline of key architectural and technical decisions from a session",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "session_id": {"type": "string", "description": "UUID of the session"}
                },
                "required": ["session_id"]
            }
        }),
        json!({
            "name": "get_key_insights",
            "description": "Get most important insights from a session ranked by importance",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "session_id": {"type": "string", "description": "UUID of the session"},
                    "limit": {"type": "number", "description": "Max insights to return (default: 10)"}
                },
                "required": ["session_id"]
            }
        }),
        json!({
            "name": "get_entity_importance_analysis",
            "description": "Analyze which entities (people, concepts, files) are most important in the conversation",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "session_id": {"type": "string", "description": "UUID of the session"},
                    "limit": {"type": "number", "description": "Max entities to return"},
                    "min_importance": {"type": "number", "description": "Minimum importance score (0.0-1.0)"}
                },
                "required": ["session_id"]
            }
        }),
        json!({
            "name": "get_entity_network_view",
            "description": "View the network of relationships between entities in the knowledge graph",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "session_id": {"type": "string", "description": "UUID of the session"},
                    "center_entity": {"type": "string", "description": "Optional entity to center the network on"},
                    "max_entities": {"type": "number", "description": "Max entities to include"},
                    "max_relationships": {"type": "number", "description": "Max relationships to include"}
                },
                "required": ["session_id"]
            }
        }),
        json!({
            "name": "get_session_statistics",
            "description": "Get detailed statistics about session size, update counts, and entity counts",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "session_id": {"type": "string", "description": "UUID of the session"}
                },
                "required": ["session_id"]
            }
        }),
        // Utility (2 tools)
        json!({
            "name": "create_session_checkpoint",
            "description": "Create a snapshot of current session state for backup or versioning",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "session_id": {"type": "string", "description": "UUID of the session"}
                },
                "required": ["session_id"]
            }
        }),
        json!({
            "name": "get_tool_catalog",
            "description": "Get comprehensive catalog of all available tools with usage guidance",
            "inputSchema": {
                "type": "object"
            }
        }),
        // Workspace Management (5 tools)
        json!({
            "name": "create_workspace",
            "description": "Create a workspace for organizing related sessions by project or topic",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "name": {"type": "string", "description": "Workspace name"},
                    "description": {"type": "string", "description": "Workspace description"}
                },
                "required": ["name", "description"]
            }
        }),
        json!({
            "name": "get_workspace",
            "description": "Get details about a specific workspace including all sessions",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "workspace_id": {"type": "string", "description": "UUID of the workspace"}
                },
                "required": ["workspace_id"]
            }
        }),
        json!({
            "name": "list_workspaces",
            "description": "List all available workspaces",
            "inputSchema": {
                "type": "object"
            }
        }),
        json!({
            "name": "delete_workspace",
            "description": "Delete a workspace (does not delete the sessions within it)",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "workspace_id": {"type": "string", "description": "UUID of the workspace"}
                },
                "required": ["workspace_id"]
            }
        }),
        json!({
            "name": "add_session_to_workspace",
            "description": "Add a session to a workspace with a specific role",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "workspace_id": {"type": "string", "description": "UUID of the workspace"},
                    "session_id": {"type": "string", "description": "UUID of the session"},
                    "role": {"type": "string", "description": "Role of this session in the workspace"}
                },
                "required": ["workspace_id", "session_id", "role"]
            }
        }),
        json!({
            "name": "remove_session_from_workspace",
            "description": "Remove a session from a workspace",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "workspace_id": {"type": "string", "description": "UUID of the workspace"},
                    "session_id": {"type": "string", "description": "UUID of the session"}
                },
                "required": ["workspace_id", "session_id"]
            }
        }),
    ]
}

// ============================================================================
// Workspace Management Tools
// ============================================================================

/// Create a new workspace for organizing related sessions
#[instrument(skip_all, fields(workspace_name = %name))]
pub async fn create_workspace(name: String, description: String) -> Result<MCPToolResult> {
    info!("MCP-TOOLS: create_workspace() called with name: '{}'", name);
    let system = get_memory_system().await?;

    let workspace_id = system
        .workspace_manager
        .create_workspace(name.clone(), description.clone());

    // Persist workspace to RocksDB
    if let Err(e) = system
        .storage_actor
        .save_workspace_metadata(
            workspace_id,
            &name,
            &description,
            &Vec::new(), // session_ids - empty initially
        )
        .await
    {
        error!("Failed to persist workspace: {}", e);
        return Ok(MCPToolResult::error(format!(
            "Failed to persist workspace: {}",
            e
        )));
    }

    info!("Created workspace {} with ID {}", name, workspace_id);

    Ok(MCPToolResult::success(
        format!("Created workspace '{}' with ID: {}", name, workspace_id),
        Some(serde_json::json!({
            "workspace_id": workspace_id.to_string(),
            "name": name,
            "description": description,
            "session_count": 0
        })),
    ))
}

/// Get workspace details by ID
#[instrument(skip_all, fields(workspace_id = %workspace_id))]
pub async fn get_workspace(workspace_id: Uuid) -> Result<MCPToolResult> {
    info!("MCP-TOOLS: get_workspace() called for ID: {}", workspace_id);
    let system = get_memory_system().await?;

    match system.workspace_manager.get_workspace(&workspace_id) {
        Some(workspace) => {
            let sessions = workspace.get_all_sessions();
            let metadata = workspace.metadata.load();

            Ok(MCPToolResult::success(
                format!("Retrieved workspace '{}'", workspace.name),
                Some(serde_json::json!({
                    "workspace_id": workspace.id.to_string(),
                    "name": workspace.name,
                    "description": workspace.description,
                    "created_at": workspace.created_at.duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs(),
                    "session_count": sessions.len(),
                    "sessions": sessions.iter().map(|(id, role)| {
                        serde_json::json!({
                            "session_id": id.to_string(),
                            "role": format!("{:?}", role)
                        })
                    }).collect::<Vec<_>>(),
                    "metadata": {
                        "project_type": metadata.project_type,
                        "tags": metadata.tags,
                        "root_paths": metadata.root_paths.iter()
                            .map(|p| p.to_string_lossy().to_string())
                            .collect::<Vec<_>>()
                    }
                })),
            ))
        }
        None => Ok(MCPToolResult::error(format!(
            "Workspace {} not found",
            workspace_id
        ))),
    }
}

/// List all workspaces
#[instrument(skip_all)]
pub async fn list_workspaces() -> Result<MCPToolResult> {
    info!("MCP-TOOLS: list_workspaces() called");
    let system = get_memory_system().await?;

    let workspaces = system.workspace_manager.list_workspaces();
    let total_count = workspaces.len();

    let workspace_list: Vec<serde_json::Value> = workspaces
        .iter()
        .map(|ws| {
            let sessions = ws.get_all_sessions();
            serde_json::json!({
                "workspace_id": ws.id.to_string(),
                "name": ws.name,
                "description": ws.description,
                "session_count": sessions.len(),
                "created_at": ws.created_at.duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs()
            })
        })
        .collect();

    let message = if total_count == 0 {
        "Found 0 workspaces".to_string()
    } else {
        let workspace_summaries: Vec<String> = workspaces
            .iter()
            .map(|ws| {
                let sessions = ws.get_all_sessions();
                format!(
                    "  • {} (ID: {}, {} sessions)",
                    ws.name,
                    ws.id,
                    sessions.len()
                )
            })
            .collect();
        format!(
            "Found {} workspace(s):\n{}",
            total_count,
            workspace_summaries.join("\n")
        )
    };

    Ok(MCPToolResult::success(
        message,
        Some(serde_json::json!({
            "total_count": total_count,
            "workspaces": workspace_list
        })),
    ))
}

/// Delete a workspace
#[instrument(skip_all, fields(workspace_id = %workspace_id))]
pub async fn delete_workspace(workspace_id: Uuid) -> Result<MCPToolResult> {
    info!(
        "MCP-TOOLS: delete_workspace() called for ID: {}",
        workspace_id
    );
    let system = get_memory_system().await?;

    match system.workspace_manager.delete_workspace(&workspace_id) {
        Some(workspace) => {
            // Delete from RocksDB
            if let Err(e) = system.storage_actor.delete_workspace(workspace_id).await {
                error!("Failed to delete workspace from storage: {}", e);
                return Ok(MCPToolResult::error(format!(
                    "Failed to delete workspace from storage: {}",
                    e
                )));
            }

            info!("Deleted workspace '{}' ({})", workspace.name, workspace_id);

            Ok(MCPToolResult::success(
                format!("Deleted workspace '{}' successfully", workspace.name),
                Some(serde_json::json!({
                    "workspace_id": workspace_id.to_string(),
                    "name": workspace.name
                })),
            ))
        }
        None => Ok(MCPToolResult::error(format!(
            "Workspace {} not found",
            workspace_id
        ))),
    }
}

/// Add a session to a workspace with a specific role
#[instrument(skip_all, fields(workspace_id = %workspace_id, session_id = %session_id))]
pub async fn add_session_to_workspace(
    workspace_id: Uuid,
    session_id: Uuid,
    role: String,
) -> Result<MCPToolResult> {
    info!(
        "MCP-TOOLS: add_session_to_workspace() called - workspace: {}, session: {}, role: {}",
        workspace_id, session_id, role
    );
    let system = get_memory_system().await?;

    // Parse role
    use crate::workspace::SessionRole;
    let session_role = match role.to_lowercase().as_str() {
        "primary" => SessionRole::Primary,
        "related" => SessionRole::Related,
        "dependency" => SessionRole::Dependency,
        "shared" => SessionRole::Shared,
        _ => {
            return Ok(MCPToolResult::error(format!(
                "Invalid role '{}'. Must be one of: primary, related, dependency, shared",
                role
            )));
        }
    };

    // Verify session exists
    if system.get_session(session_id).await.is_err() {
        return Ok(MCPToolResult::error(format!(
            "Session {} not found",
            session_id
        )));
    }

    // Add session to workspace
    if let Err(e) =
        system
            .workspace_manager
            .add_session_to_workspace(&workspace_id, session_id, session_role)
    {
        return Ok(MCPToolResult::error(e));
    }

    // Persist to RocksDB
    if let Err(e) = system
        .storage_actor
        .add_session_to_workspace(workspace_id, session_id, session_role)
        .await
    {
        error!("Failed to persist workspace-session association: {}", e);
        return Ok(MCPToolResult::error(format!(
            "Failed to persist workspace-session association: {}",
            e
        )));
    }

    info!(
        "Added session {} to workspace {} with role {:?}",
        session_id, workspace_id, session_role
    );

    Ok(MCPToolResult::success(
        format!("Added session to workspace with role '{}'", role),
        Some(serde_json::json!({
            "workspace_id": workspace_id.to_string(),
            "session_id": session_id.to_string(),
            "role": format!("{:?}", session_role)
        })),
    ))
}

/// Remove a session from a workspace
#[instrument(skip_all, fields(workspace_id = %workspace_id, session_id = %session_id))]
pub async fn remove_session_from_workspace(
    workspace_id: Uuid,
    session_id: Uuid,
) -> Result<MCPToolResult> {
    info!(
        "MCP-TOOLS: remove_session_from_workspace() called - workspace: {}, session: {}",
        workspace_id, session_id
    );
    let system = get_memory_system().await?;

    match system
        .workspace_manager
        .remove_session_from_workspace(&workspace_id, &session_id)
    {
        Ok(Some(role)) => {
            // Remove from RocksDB
            if let Err(e) = system
                .storage_actor
                .remove_session_from_workspace(workspace_id, session_id)
                .await
            {
                error!("Failed to remove session from workspace in storage: {}", e);
                return Ok(MCPToolResult::error(format!(
                    "Failed to remove session from workspace in storage: {}",
                    e
                )));
            }

            info!(
                "Removed session {} from workspace {} (was role {:?})",
                session_id, workspace_id, role
            );

            Ok(MCPToolResult::success(
                "Removed session from workspace successfully".to_string(),
                Some(serde_json::json!({
                    "workspace_id": workspace_id.to_string(),
                    "session_id": session_id.to_string(),
                    "previous_role": format!("{:?}", role)
                })),
            ))
        }
        Ok(None) => Ok(MCPToolResult::error(format!(
            "Session {} not found in workspace {}",
            session_id, workspace_id
        ))),
        Err(e) => Ok(MCPToolResult::error(e)),
    }
}
