use crate::core::memory_system::ConversationMemorySystem;
use crate::core::timeout_utils::with_storage_timeout;
use crate::session::active_session::ActiveSession;
use crate::storage::rocksdb_storage::SessionCheckpoint;
use crate::tools::mcp::{get_memory_system, string_to_anyhow, MCPToolResult};
use anyhow::Result;
use arc_swap::ArcSwap;
use std::sync::Arc;
use tracing::{error, info, instrument};
use uuid::Uuid;

pub async fn create_session_checkpoint_with_system(
    session_id: Uuid,
    system: &ConversationMemorySystem,
) -> Result<MCPToolResult> {
    let session_arc = system
        .get_session(session_id)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to load session: {}", e))?;
    let session = session_arc.load();

    let checkpoint = create_comprehensive_checkpoint(&session).await?;

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

pub async fn create_session_checkpoint(session_id: Uuid) -> Result<MCPToolResult> {
    let result = with_storage_timeout(async {
        let system = get_memory_system().await?;
        let session_arc = system
            .get_session(session_id)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to load session: {}", e))?;
        let session = session_arc.load();

        let checkpoint = create_comprehensive_checkpoint(&session).await?;

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

        let checkpoint = system
            .storage_actor
            .load_checkpoint(checkpoint_id)
            .await
            .map_err(string_to_anyhow)?;

        let mut session = ActiveSession::new(session_id, None, None);
        session.current_state = Arc::new(checkpoint.structured_context);
        session.incremental_updates = Arc::new(checkpoint.recent_updates);
        session.code_references = Arc::new(checkpoint.code_references);
        session.change_history = Arc::new(checkpoint.change_history);

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

    let mut found = false;
    session_arc.rcu(|current| {
        let mut updated = (**current).clone();
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

pub async fn search_sessions(query: String) -> Result<MCPToolResult> {
    let result = with_storage_timeout(async {
        let system = get_memory_system().await?;
        let session_ids = system
            .find_sessions_by_name_or_description(&query)
            .await
            .map_err(string_to_anyhow)?;

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

async fn create_comprehensive_checkpoint(session: &ActiveSession) -> Result<SessionCheckpoint> {
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
