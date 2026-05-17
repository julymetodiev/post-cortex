use post_cortex_core::core::context_update::EntityType;
use post_cortex_memory::ConversationMemorySystem;
use post_cortex_core::core::timeout_utils::with_mcp_timeout;
use post_cortex_core::session::active_session::ActiveSession;
use crate::{
    get_memory_system, parse_datetime, ContextQuery, ContextResponse,
    MCPToolResult,
};
use post_cortex_core::core::context_update::UpdateType;
use anyhow::Result;
use std::collections::HashMap;
use tracing::{debug, error};
use uuid::Uuid;

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
                    _ => EntityType::Concept,
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
                _ => EntityType::Concept,
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

pub(crate) async fn query_context(
    session: &ActiveSession,
    query: ContextQuery,
) -> Result<ContextResponse> {
    use post_cortex_core::core::context_update::CodeReference;

    match query {
        ContextQuery::GetRecentChanges { since } => {
            let recent_updates: Vec<post_cortex_core::core::context_update::ContextUpdate> = session
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
            let update_results: Vec<post_cortex_core::core::context_update::ContextUpdate> = session
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
            let decisions: Vec<post_cortex_core::core::context_update::ContextUpdate> = session
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
            let changes: Vec<post_cortex_core::core::context_update::ContextUpdate> = session
                .hot_context
                .iter()
                .into_iter()
                .filter(|u| matches!(u.update_type, UpdateType::CodeChanged))
                .collect();
            Ok(ContextResponse::ChangeHistory(changes))
        }
        ContextQuery::AssembleContext { query, token_budget } => {
            use post_cortex_memory::context_assembly;

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
