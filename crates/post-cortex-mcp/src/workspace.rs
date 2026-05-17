use post_cortex_memory::ConversationMemorySystem;
use crate::{get_memory_system, MCPToolResult};
use anyhow::Result;
use tracing::{error, info, instrument};
use uuid::Uuid;

/// Reload the current session list for `workspace_id` and re-persist the
/// workspace metadata row. Called after add/remove session mutations so
/// storage stays in sync with the in-memory workspace_manager.
async fn persist_workspace_after_mutation(
    system: &ConversationMemorySystem,
    workspace_id: Uuid,
) -> std::result::Result<(), String> {
    let workspace = system.workspace_manager.get_workspace(&workspace_id);
    let (ws_name, ws_description, sessions) = match &workspace {
        Some(ws) => (
            ws.name.clone(),
            ws.description.clone(),
            ws.get_all_sessions(),
        ),
        None => (String::new(), String::new(), Vec::new()),
    };
    let session_uuids: Vec<Uuid> = sessions.into_iter().map(|(id, _)| id).collect();

    system
        .storage_actor
        .save_workspace_metadata(workspace_id, &ws_name, &ws_description, &session_uuids)
        .await
}

#[instrument(skip_all, fields(workspace_name = %name))]
pub async fn create_workspace(name: String, description: String) -> Result<MCPToolResult> {
    info!("MCP-TOOLS: create_workspace() called with name: '{}'", name);
    let system = get_memory_system().await?;

    let workspace_id = system
        .workspace_manager
        .create_workspace(name.clone(), description.clone());

    if let Err(e) = system
        .storage_actor
        .save_workspace_metadata(
            workspace_id,
            &name,
            &description,
            &Vec::new(),
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

#[instrument(skip_all, fields(workspace_id = %workspace_id))]
pub async fn get_workspace(workspace_id: Uuid) -> Result<MCPToolResult> {
    info!("MCP-TOOLS: get_workspace() called for ID: {}", workspace_id);
    let system = get_memory_system().await?;

    match system.workspace_manager.get_workspace(&workspace_id) {
        Some(workspace) => {
            let sessions = workspace.get_all_sessions();
            let session_details: Vec<serde_json::Value> = sessions
                .into_iter()
                .map(|(id, role)| {
                    serde_json::json!({
                        "session_id": id.to_string(),
                        "role": format!("{:?}", role)
                    })
                })
                .collect();

            Ok(MCPToolResult::success(
                format!("Workspace '{}' retrieved", workspace.name),
                Some(serde_json::json!({
                    "workspace_id": workspace.id.to_string(),
                    "name": workspace.name,
                    "description": workspace.description,
                    "created_at": workspace.created_at,
                    "sessions": session_details,
                    "session_count": workspace.session_ids.len()
                })),
            ))
        }
        None => Ok(MCPToolResult::error(format!(
            "Workspace {} not found",
            workspace_id
        ))),
    }
}

pub async fn list_workspaces() -> Result<MCPToolResult> {
    info!("MCP-TOOLS: list_workspaces() called");
    let system = get_memory_system().await?;

    let workspaces = system.workspace_manager.list_workspaces();

    let workspace_list: Vec<serde_json::Value> = workspaces
        .into_iter()
        .map(|ws| {
            serde_json::json!({
                "workspace_id": ws.id.to_string(),
                "name": ws.name,
                "description": ws.description,
                "session_count": ws.session_ids.len(),
                "created_at": ws.created_at
            })
        })
        .collect();

    Ok(MCPToolResult::success(
        format!("Found {} workspaces", workspace_list.len()),
        Some(serde_json::json!({
            "workspaces": workspace_list
        })),
    ))
}

pub async fn delete_workspace(workspace_id: Uuid) -> Result<MCPToolResult> {
    info!(
        "MCP-TOOLS: delete_workspace() called for ID: {}",
        workspace_id
    );
    let system = get_memory_system().await?;

    match system.workspace_manager.delete_workspace(&workspace_id) {
        Some(workspace) => {
            if let Err(e) = system
                .storage_actor
                .delete_workspace(workspace_id)
                .await
            {
                error!("Failed to delete workspace from storage: {}", e);
                return Ok(MCPToolResult::error(format!(
                    "Failed to delete workspace from storage: {}",
                    e
                )));
            }

            info!("Deleted workspace '{}' ({})", workspace.name, workspace_id);

            Ok(MCPToolResult::success(
                format!("Deleted workspace '{}'", workspace.name),
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

pub async fn add_session_to_workspace(
    workspace_id: Uuid,
    session_id: Uuid,
    role: String,
) -> Result<MCPToolResult> {
    info!(
        "MCP-TOOLS: add_session_to_workspace() called: session {} -> workspace {} (role: {})",
        session_id, workspace_id, role
    );
    let system = get_memory_system().await?;

    let role_enum = match role.to_lowercase().as_str() {
        "primary" => post_cortex_core::workspace::SessionRole::Primary,
        "related" => post_cortex_core::workspace::SessionRole::Related,
        "dependency" => post_cortex_core::workspace::SessionRole::Dependency,
        "shared" => post_cortex_core::workspace::SessionRole::Shared,
        _ => post_cortex_core::workspace::SessionRole::Related,
    };

    match system
        .workspace_manager
        .add_session_to_workspace(&workspace_id, session_id, role_enum)
    {
        Ok(()) => {
            if let Err(e) = persist_workspace_after_mutation(&system, workspace_id).await {
                error!("Failed to update workspace in storage: {}", e);
                return Ok(MCPToolResult::error(format!(
                    "Failed to update workspace in storage: {}",
                    e
                )));
            }

            info!(
                "Added session {} to workspace {} with role {:?}",
                session_id, workspace_id, role_enum
            );

            Ok(MCPToolResult::success(
                "Added session to workspace successfully".to_string(),
                Some(serde_json::json!({
                    "workspace_id": workspace_id.to_string(),
                    "session_id": session_id.to_string(),
                    "role": format!("{:?}", role_enum)
                })),
            ))
        }
        Err(e) => Ok(MCPToolResult::error(format!(
            "Failed to add session to workspace: {}",
            e
        ))),
    }
}

pub async fn remove_session_from_workspace(
    workspace_id: Uuid,
    session_id: Uuid,
) -> Result<MCPToolResult> {
    info!(
        "MCP-TOOLS: remove_session_from_workspace() called: session {} from workspace {}",
        session_id, workspace_id
    );
    let system = get_memory_system().await?;

    match system
        .workspace_manager
        .remove_session_from_workspace(&workspace_id, &session_id)
    {
        Ok(Some(role)) => {
            if let Err(e) = persist_workspace_after_mutation(&system, workspace_id).await {
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
