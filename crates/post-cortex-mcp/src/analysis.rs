//! Analysis, summaries, insights, and session statistics.

use crate::{MCPToolResult, get_memory_system};
use anyhow::Result;
use post_cortex_core::session::active_session::ActiveSession;
use post_cortex_core::summary::SummaryGenerator;
use uuid::Uuid;

/// Generate a structured summary of the session with optional filtering.
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

    use post_cortex_core::summary::SummaryOptions;
    let user_requested_compact = compact.unwrap_or(false);
    const MAX_TOKENS: usize = 50_000;

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

/// Extract a timeline of key architectural and technical decisions.
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

/// Extract the top insights from a session ranked by importance.
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

/// Analyse entity importance with optional minimum-score and limit filters.
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
    let graph = &session.entity_graph;

    let total_entities = graph.entities.len();
    let entity_limit = limit.unwrap_or(100);

    // Ranked by importance_score, so the min_importance filter keeps a
    // prefix of the ranking: only the top `entity_limit` are built.
    let mut analysis = graph.analyze_entity_importance_top(entity_limit);
    let after_filter = match min_importance {
        Some(min_imp) => {
            analysis.retain(|entity| entity.importance_score >= min_imp);
            graph.count_entities_with_importance_at_least(min_imp)
        }
        None => total_entities,
    };
    let truncated = after_filter > entity_limit;

    let result = serde_json::json!({
        "entities": analysis,
        "pagination": {
            "total_entities": total_entities,
            "after_filter": after_filter,
            "shown": if truncated { entity_limit } else { after_filter },
            "truncated": truncated
        }
    });

    Ok(MCPToolResult::success(
        format!(
            "Analyzed {} entities (showing {})",
            total_entities,
            if truncated {
                entity_limit
            } else {
                after_filter
            }
        ),
        Some(result),
    ))
}

/// Build a network view of entities and their relationships.
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

    let max_entities = max_entities.unwrap_or(50);
    let max_relationships = max_relationships.unwrap_or(100);

    // Resolve the BFS center identically to the gRPC handler (explicit name,
    // else the most important entity).
    let resolved_center = match center_entity.as_deref() {
        Some(name) if !name.is_empty() => Some(name.to_string()),
        _ => session
            .entity_graph
            .get_most_important_entities(1)
            .first()
            .map(|e| e.name.clone()),
    };

    // The BFS stops at the limits, so only the entities and relationships
    // shown are visited and cloned (closest to the center first).
    let network = session.entity_graph.get_entity_network_bounded(
        resolved_center.as_deref().unwrap_or(""),
        2,
        max_entities,
        max_relationships,
    );

    let entities: Vec<serde_json::Value> = network
        .entities
        .values()
        .map(|e| {
            serde_json::json!({
                "name": e.name,
                "entity_type": format!("{:?}", e.entity_type),
                "importance_score": e.importance_score,
                "mention_count": e.mention_count
            })
        })
        .collect();

    let relationships: Vec<serde_json::Value> = network
        .relationships
        .iter()
        .map(|r| {
            serde_json::json!({
                "from": r.from_entity,
                "type": format!("{:?}", r.relation_type),
                "to": r.to_entity
            })
        })
        .collect();

    Ok(MCPToolResult::success(
        format!(
            "Network view: {} entities, {} relationships",
            entities.len(),
            relationships.len()
        ),
        Some(serde_json::json!({
            "session_id": session_id,
            "center_entity": resolved_center,
            "entities": entities,
            "relationships": relationships
        })),
    ))
}

/// Return detailed statistics about a session's size and activity.
pub async fn get_session_statistics(session_id: String) -> Result<MCPToolResult> {
    let uuid =
        Uuid::parse_str(&session_id).map_err(|e| anyhow::anyhow!("Invalid session ID: {}", e))?;

    let system = get_memory_system().await?;
    let session_arc = system
        .get_session(uuid)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to load session: {}", e))?;

    let session = session_arc.load();

    Ok(MCPToolResult::success(
        format!("Session statistics for {}", session_id),
        Some(session_statistics(&session_id, &session)),
    ))
}

/// Build the statistics payload for one session.
fn session_statistics(session_id: &str, session: &ActiveSession) -> serde_json::Value {
    let hot_updates = session.hot_context.len();
    let warm_updates = session.warm_context.len();
    let incremental_updates = session.incremental_updates.len();
    // Hot and warm tiers are subsets of the incremental log, so the log alone
    // is the total.
    let total_updates = incremental_updates;
    let entity_count = session.entity_graph.entities.len();
    let relationship_count = session.entity_graph.get_graph_stats().edge_count;
    let code_refs = session.code_references.len();
    let change_history = session.change_history.len();

    let first = session
        .incremental_updates
        .iter()
        .map(|u| u.timestamp)
        .min();
    let last = session
        .incremental_updates
        .iter()
        .map(|u| u.timestamp)
        .max();
    let duration = first.zip(last).map(|(f, l)| (l - f).num_minutes());

    serde_json::json!({
        "session_id": session_id,
        "name": session.name(),
        "description": session.description(),
        "created_at": session.created_at().to_rfc3339(),
        "last_updated": session.last_updated.to_rfc3339(),
        "hot_updates": hot_updates,
        "warm_updates": warm_updates,
        "incremental_updates": incremental_updates,
        "total_updates": total_updates,
        "entity_count": entity_count,
        "relationship_count": relationship_count,
        "code_references": code_refs,
        "change_history": change_history,
        "first_update": first.map(|t| t.to_rfc3339()),
        "last_update": last.map(|t| t.to_rfc3339()),
        "duration_minutes": duration,
        "vectorized_count": 0
    })
}

/// Return a JSON catalog describing every available MCP tool and its usage.
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
            "Semantic search auto-vectorizes - no manual vectorize_session needed!",
            "Use semantic_search for concepts, query_conversation_context for keywords",
            "get_structured_summary has auto-compact - safe for large sessions",
            "Similarity scores: 0.65-0.75 = excellent, 0.45-0.55 = good, <0.30 = weak",
            "All sessions persist across Claude Code conversations"
        ]
    });

    Ok(MCPToolResult::success(
        "Retrieved tool catalog with 26 tools across 5 categories".to_string(),
        Some(catalog),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use post_cortex_core::core::context_update::{
        ContextUpdate, EntityRelationship, RelationType, UpdateContent, UpdateType,
    };
    use std::sync::Arc;

    fn update() -> ContextUpdate {
        ContextUpdate {
            meta: Default::default(),
            id: Uuid::new_v4(),
            timestamp: chrono::Utc::now(),
            update_type: UpdateType::ConceptDefined,
            content: UpdateContent {
                title: "t".into(),
                description: String::new(),
                details: vec![],
                examples: vec![],
                implications: vec![],
            },
            related_code: None,
            parent_update: None,
            user_marked_important: false,
            creates_entities: vec![],
            creates_relationships: vec![],
            references_entities: vec![],
            typed_entities: vec![],
            author: None,
            derived_from: Vec::new(),
        }
    }

    #[tokio::test]
    async fn statistics_do_not_double_count_tiers() {
        let mut session = ActiveSession::new(Uuid::new_v4(), None, None);
        for _ in 0..3 {
            session.add_incremental_update_fast(update()).await.unwrap();
        }
        let graph = Arc::make_mut(&mut session.entity_graph);
        for (from, to) in [("a", "b"), ("b", "c")] {
            graph.add_relationship(EntityRelationship {
                from_entity: from.into(),
                to_entity: to.into(),
                relation_type: RelationType::RelatedTo,
                context: String::new(),
            });
        }

        let stats = session_statistics("sid", &session);
        assert_eq!(stats["hot_updates"], 3);
        assert_eq!(stats["incremental_updates"], 3);
        // Hot entries are also in the incremental log: total is 3, not 6.
        assert_eq!(stats["total_updates"], 3);
        assert_eq!(stats["relationship_count"], 2);
        assert_eq!(stats["entity_count"], 3);
        assert_eq!(stats["duration_minutes"], 0);
    }
}
