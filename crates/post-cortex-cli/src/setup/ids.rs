// Copyright (c) 2025, 2026 Julius ML
// MIT License

//! The session and workspace of a project setup, found or created through
//! the backend.

use std::env;

use dialoguer::{Input, Select};

use crate::daemon_client::DaemonClient;
use crate::profiles::Profile;
use crate::project::Ids;

/// The session and workspace of a project setup: an existing one by name
/// (asked when interactive), else new ones; the session is attached to the
/// workspace.
pub async fn choose_ids(
    backend: &Profile,
    daemon: &DaemonClient,
    name: Option<String>,
    non_interactive: bool,
) -> Result<Ids, String> {
    // 1. Determine project name
    let default_name = name.unwrap_or_else(|| {
        env::current_dir()
            .ok()
            .and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_string()))
            .unwrap_or_else(|| "my-project".to_string())
    });

    let project_name = if non_interactive {
        default_name
    } else {
        Input::new()
            .with_prompt("Session name")
            .default(default_name)
            .interact_text()
            .map_err(|e| format!("Input error: {}", e))?
    };

    // 2. Fetch existing sessions and workspaces. A backend that cannot list
    // cannot create either.
    let reach = |e: String| format!("{} ({}): {e}", backend.name, backend.url);
    let existing_sessions = daemon.list_sessions().await.map_err(reach)?;
    let existing_workspaces = daemon.list_workspaces().await.map_err(reach)?;

    // 3. Session: reuse existing by name or create new
    let matching_session = existing_sessions
        .iter()
        .find(|s| s.name.eq_ignore_ascii_case(&project_name));

    let session_id = if let Some(existing) = matching_session {
        if non_interactive {
            println!("  Session found: {} ({})", existing.name, existing.id);
            existing.id.clone()
        } else {
            let choice = Select::new()
                .with_prompt(format!(
                    "Session '{}' already exists ({}). Use it?",
                    existing.name, existing.id
                ))
                .items(["Use existing session", "Create new session"])
                .default(0)
                .interact()
                .map_err(|e| format!("Selection error: {}", e))?;

            if choice == 0 {
                println!("  Session reused: {} ({})", existing.name, existing.id);
                existing.id.clone()
            } else {
                create_session_for_setup(daemon, &project_name).await?
            }
        }
    } else {
        create_session_for_setup(daemon, &project_name).await?
    };

    // 4. Workspace: reuse existing by name, pick from list, or create new
    let workspace_id: String;
    let workspace_name: String;

    let matching_workspace = existing_workspaces
        .iter()
        .find(|ws| ws.name.eq_ignore_ascii_case(&project_name));

    if let Some(existing) = matching_workspace {
        if non_interactive {
            workspace_id = existing.id.clone();
            workspace_name = existing.name.clone();
            println!("  Workspace found: {} ({})", workspace_name, workspace_id);
        } else {
            let choice = Select::new()
                .with_prompt(format!(
                    "Workspace '{}' already exists ({}). Use it?",
                    existing.name, existing.id
                ))
                .items(["Use existing workspace", "Create new workspace"])
                .default(0)
                .interact()
                .map_err(|e| format!("Selection error: {}", e))?;

            if choice == 0 {
                workspace_id = existing.id.clone();
                workspace_name = existing.name.clone();
                println!("  Workspace reused: {} ({})", workspace_name, workspace_id);
            } else {
                let ws_name: String = Input::new()
                    .with_prompt("Workspace name")
                    .default(project_name.clone())
                    .interact_text()
                    .map_err(|e| format!("Input error: {}", e))?;
                let (id, name) = create_workspace_for_setup(daemon, &ws_name).await?;
                workspace_id = id;
                workspace_name = name;
            }
        }
    } else if !existing_workspaces.is_empty() && !non_interactive {
        // No name match, but workspaces exist — offer selection
        let mut items: Vec<String> = existing_workspaces
            .iter()
            .map(|ws| format!("{} ({})", ws.name, ws.id))
            .collect();
        items.push("Create new workspace".to_string());

        let selection = Select::new()
            .with_prompt("Attach to existing workspace or create new?")
            .items(&items)
            .default(items.len() - 1)
            .interact()
            .map_err(|e| format!("Selection error: {}", e))?;

        if selection < existing_workspaces.len() {
            workspace_id = existing_workspaces[selection].id.clone();
            workspace_name = existing_workspaces[selection].name.clone();
        } else {
            let ws_name: String = Input::new()
                .with_prompt("Workspace name")
                .default(project_name.clone())
                .interact_text()
                .map_err(|e| format!("Input error: {}", e))?;
            let (id, name) = create_workspace_for_setup(daemon, &ws_name).await?;
            workspace_id = id;
            workspace_name = name;
        }
    } else {
        // No workspaces or non-interactive: create new
        let ws_name = if non_interactive {
            project_name.clone()
        } else {
            Input::new()
                .with_prompt("Workspace name")
                .default(project_name.clone())
                .interact_text()
                .map_err(|e| format!("Input error: {}", e))?
        };
        let (id, name) = create_workspace_for_setup(daemon, &ws_name).await?;
        workspace_id = id;
        workspace_name = name;
    };

    println!("  Workspace: {} ({})", workspace_name, workspace_id);

    // 4. Attach session to workspace
    daemon
        .attach_session(&workspace_id, &session_id, "primary")
        .await?;
    println!("  Session attached to workspace (role: primary)");

    Ok(Ids {
        session_id,
        workspace_id,
    })
}

async fn create_session_for_setup(daemon: &DaemonClient, name: &str) -> Result<String, String> {
    let session = daemon
        .create_session(
            Some(name.to_string()),
            Some(format!("Development session for {}", name)),
        )
        .await?;
    println!("  Session created: {} ({})", name, session.id);
    Ok(session.id)
}

async fn create_workspace_for_setup(
    daemon: &DaemonClient,
    name: &str,
) -> Result<(String, String), String> {
    let ws = daemon
        .create_workspace(name.to_string(), format!("{} workspace", name))
        .await?;
    Ok((ws.id, ws.name))
}
