//! Semantic and embedding-powered search across sessions.

use crate::{MCPToolResult, get_memory_system};
use anyhow::Result;
use post_cortex_core::core::timeout_utils::with_storage_timeout;
use tracing::{error, info};
use uuid::Uuid;

#[cfg(feature = "embeddings")]
use chrono::{DateTime, Utc};

/// Parse the optional `date_from` / `date_to` pair into a UTC range.
///
/// Both must be provided together; partial pairs return an error message
/// suitable for [`MCPToolResult::error`]. Empty (None, None) returns Ok(None).
#[cfg(feature = "embeddings")]
fn parse_date_range(
    date_from: Option<String>,
    date_to: Option<String>,
) -> std::result::Result<Option<(DateTime<Utc>, DateTime<Utc>)>, String> {
    match (date_from, date_to) {
        (Some(from_str), Some(to_str)) => {
            let from = DateTime::parse_from_rfc3339(&from_str)
                .map_err(|e| format!("Invalid date_from format: {}", e))?
                .with_timezone(&Utc);
            let to = DateTime::parse_from_rfc3339(&to_str)
                .map_err(|e| format!("Invalid date_to format: {}", e))?
                .with_timezone(&Utc);
            Ok(Some((from, to)))
        }
        (Some(_), None) | (None, Some(_)) => {
            Err("Both date_from and date_to must be provided together".to_string())
        }
        _ => Ok(None),
    }
}

/// Render a single semantic-search hit as a json object. When
/// `include_session_id` is true a `session_id` field is appended (used by the
/// global search response; session search omits it since the scope is implicit).
#[cfg(feature = "embeddings")]
fn search_hit_to_json(
    r: &post_cortex_memory::content_vectorizer::SemanticSearchResult,
    include_session_id: bool,
) -> serde_json::Value {
    let mut obj = serde_json::json!({
        "content_id": r.content_id,
        "content_type": format!("{:?}", r.content_type),
        "text_content": r.text_content,
        "similarity_score": r.similarity_score,
        "importance_score": r.importance_score,
        "timestamp": r.timestamp.to_rfc3339(),
        "combined_score": r.combined_score,
    });
    if include_session_id {
        if let Some(map) = obj.as_object_mut() {
            map.insert(
                "session_id".to_string(),
                serde_json::Value::String(r.session_id.to_string()),
            );
        }
    }
    obj
}

/// Build the human-readable summary block listed alongside the JSON payload.
/// Differs between global vs session scopes only in (a) the header line, (b)
/// whether each hit carries its session id, and (c) the snippet truncation
/// length.
#[cfg(feature = "embeddings")]
fn format_results_message(
    results: &[post_cortex_memory::content_vectorizer::SemanticSearchResult],
    header: String,
    include_session: bool,
    truncate_at: usize,
) -> String {
    let mut message = format!("{}\n\n", header);
    for (idx, r) in results.iter().enumerate() {
        message.push_str(&format!(
            "{}. [{:?}] Score: {:.3}\n",
            idx + 1,
            r.content_type,
            r.combined_score
        ));
        if include_session {
            message.push_str(&format!(
                "   Session: {}  Time: {}\n",
                r.session_id,
                r.timestamp.format("%Y-%m-%d %H:%M")
            ));
        } else {
            message.push_str(&format!(
                "   Time: {}\n",
                r.timestamp.format("%Y-%m-%d %H:%M:%S")
            ));
        }
        let content = if r.text_content.chars().count() > truncate_at {
            let truncated: String = r.text_content.chars().take(truncate_at).collect();
            format!("{}...", truncated)
        } else {
            r.text_content.clone()
        };
        message.push_str(&format!("   Content: {}\n\n", content));
    }
    message
}

/// Unified semantic search dispatcher supporting session, workspace, and global scopes.
pub async fn semantic_search(
    query: String,
    scope: Option<serde_json::Value>,
) -> Result<MCPToolResult> {
    let result = with_storage_timeout(async {
        let system = get_memory_system().await?;

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

/// Search across all sessions using AI semantic understanding (embeddings feature required).
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

    if !system.config.enable_embeddings {
        return Ok(MCPToolResult::error(
            "Embeddings not enabled or initialized".to_string(),
        ));
    }

    let date_range = match parse_date_range(date_from, date_to) {
        Ok(r) => r,
        Err(msg) => return Ok(MCPToolResult::error(msg)),
    };

    // `interaction_type` is plumbed through gRPC/MCP but the underlying engine
    // does not (yet) filter by content type — accept silently rather than
    // pretending to honour it.
    let _ = interaction_type;

    let limit = limit.unwrap_or(10);
    let results = match system
        .semantic_search_global(&query, Some(limit), date_range, recency_bias)
        .await
    {
        Ok(r) => r,
        Err(e) => {
            return Ok(MCPToolResult::error(format!(
                "Global semantic search failed: {}",
                e
            )));
        }
    };

    let formatted: Vec<serde_json::Value> = results
        .iter()
        .map(|r| search_hit_to_json(r, /* include_session_id */ true))
        .collect();
    let message = format_results_message(
        &results,
        format!("Found {} results across all sessions", results.len()),
        /* include_session */ true,
        200,
    );

    Ok(MCPToolResult::success(
        message,
        Some(serde_json::json!({
            "query": query,
            "total_results": formatted.len(),
            "results": formatted
        })),
    ))
}

/// Search within a single session using AI semantic understanding (embeddings feature required).
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
        "MCP-TOOLS: semantic_search_session() called for session {} with query: '{}'",
        session_id, query
    );
    let system = get_memory_system().await?;

    if !system.config.enable_embeddings {
        return Ok(MCPToolResult::error(
            "Embeddings not enabled or initialized".to_string(),
        ));
    }

    let date_range = match parse_date_range(date_from, date_to) {
        Ok(r) => r,
        Err(msg) => return Ok(MCPToolResult::error(msg)),
    };

    let _ = interaction_type; // not yet honoured by the embedding engine

    let limit = limit.unwrap_or(10);
    let results = match system
        .semantic_search_session(session_id, &query, Some(limit), date_range, recency_bias)
        .await
    {
        Ok(r) => r,
        Err(e) => {
            return Ok(MCPToolResult::error(format!(
                "Session semantic search failed: {}",
                e
            )));
        }
    };

    let formatted: Vec<serde_json::Value> = results
        .iter()
        .map(|r| search_hit_to_json(r, /* include_session_id */ false))
        .collect();
    let message = format_results_message(
        &results,
        format!("Found {} results in session {}", results.len(), session_id),
        /* include_session */ false,
        500,
    );

    Ok(MCPToolResult::success(
        message,
        Some(serde_json::json!({
            "session_id": session_id.to_string(),
            "query": query,
            "total_results": formatted.len(),
            "results": formatted
        })),
    ))
}

/// Find content related to a topic within a session (embeddings feature required).
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

                let content = if r.text_content.chars().count() > 500 {
                    let truncated: String = r.text_content.chars().take(500).collect();
                    format!("{}...", truncated)
                } else {
                    r.text_content.clone()
                };
                message.push_str(&format!("   Content: {}\n\n", content));
            }

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

/// Manually trigger embedding vectorization for a session (embeddings feature required).
#[cfg(feature = "embeddings")]
pub async fn vectorize_session(session_id: Uuid) -> Result<MCPToolResult> {
    info!(
        "MCP-TOOLS: vectorize_session() called for session {}",
        session_id
    );
    let system = get_memory_system().await?;
    info!("MCP-TOOLS: Got memory system for vectorize_session");

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

/// Retrieve vectorization statistics for the embedding index (embeddings feature required).
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

/// Check whether the embeddings feature is available and return supported models.
pub async fn enable_embeddings(model_type: Option<String>) -> Result<MCPToolResult> {
    if !cfg!(feature = "embeddings") {
        return Ok(MCPToolResult::error(
            "Embeddings feature not compiled in. Please rebuild with --features embeddings"
                .to_string(),
        ));
    }

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

/// Stub: returns an error when the embeddings feature is not compiled in.
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

/// Stub: returns an error when the embeddings feature is not compiled in.
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

/// Stub: returns an error when the embeddings feature is not compiled in.
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

/// Stub: returns an error when the embeddings feature is not compiled in.
#[cfg(not(feature = "embeddings"))]
pub async fn vectorize_session(_session_id: Uuid) -> Result<MCPToolResult> {
    Ok(MCPToolResult::error(
        "Vectorization requires the 'embeddings' feature to be enabled. Please rebuild with --features embeddings".to_string()
    ))
}

/// Stub: returns an error when the embeddings feature is not compiled in.
#[cfg(not(feature = "embeddings"))]
pub async fn get_vectorization_stats() -> Result<MCPToolResult> {
    Ok(MCPToolResult::error(
        "Vectorization stats require the 'embeddings' feature to be enabled. Please rebuild with --features embeddings".to_string()
    ))
}
