// Copyright (c) 2025 Julius ML
// MIT License

//! `pcx workspace` — create / delete / list / attach.

use post_cortex_core::workspace::SessionRole;
use post_cortex_daemon::daemon::{DaemonConfig, is_daemon_running};
use uuid::Uuid;

use super::admin::init_admin_system;
use super::cli::WorkspaceAction;
use super::daemon_client::DaemonClient;

pub async fn handle_workspace_action(action: WorkspaceAction) -> Result<(), String> {
    let config = DaemonConfig::load();
    let use_daemon = is_daemon_running(&config.host, config.port).await;

    match action {
        WorkspaceAction::Create { name, description } => {
            if use_daemon {
                let client = DaemonClient::new(&config);
                let ws = client.create_workspace(name, description).await?;
                println!("Workspace created:");
                println!("  ID:          {}", ws.id);
                println!("  Name:        {}", ws.name);
                println!("  Description: {}", ws.description);
            } else {
                let system = init_admin_system().await?;
                let id = Uuid::new_v4();
                system
                    .get_storage()
                    .save_workspace_metadata(id, &name, &description, &[])
                    .await?;
                println!("Workspace created:");
                println!("  ID:          {}", id);
                println!("  Name:        {}", name);
                println!("  Description: {}", description);
            }
            Ok(())
        }

        WorkspaceAction::Delete { id } => {
            if use_daemon {
                let client = DaemonClient::new(&config);
                client.delete_workspace(&id).await?;
            } else {
                let uuid = Uuid::parse_str(&id).map_err(|e| format!("Invalid UUID: {}", e))?;
                let system = init_admin_system().await?;
                system.get_storage().delete_workspace(uuid).await?;
            }
            println!("Workspace {} deleted", id);
            Ok(())
        }

        WorkspaceAction::List => {
            if use_daemon {
                let client = DaemonClient::new(&config);
                let workspaces = client.list_workspaces().await?;
                println!("Workspaces ({})", workspaces.len());
                println!("{:<38} {:<20} Sessions", "ID", "Name");
                println!("{:-<38} {:-<20} {:-<10}", "", "", "");
                for ws in workspaces {
                    println!("{:<38} {:<20} {}", ws.id, ws.name, ws.session_count);
                }
            } else {
                let system = init_admin_system().await?;
                let workspaces = system.get_storage().list_all_workspaces().await?;
                let existing_sessions: std::collections::HashSet<_> =
                    system.list_sessions().await?.into_iter().collect();
                println!("Workspaces ({})", workspaces.len());
                println!("{:<38} {:<20} Sessions", "ID", "Name");
                println!("{:-<38} {:-<20} {:-<10}", "", "", "");
                for ws in workspaces {
                    let actual_count = ws
                        .sessions
                        .iter()
                        .filter(|(id, _)| existing_sessions.contains(id))
                        .count();
                    println!("{:<38} {:<20} {}", ws.id, ws.name, actual_count);
                }
            }
            Ok(())
        }

        WorkspaceAction::Attach {
            workspace_id,
            session_id,
            role,
        } => {
            if use_daemon {
                let client = DaemonClient::new(&config);
                client
                    .attach_session(&workspace_id, &session_id, &role)
                    .await?;
            } else {
                let ws_id = Uuid::parse_str(&workspace_id)
                    .map_err(|e| format!("Invalid workspace UUID: {}", e))?;
                let sess_id = Uuid::parse_str(&session_id)
                    .map_err(|e| format!("Invalid session UUID: {}", e))?;
                let role_enum = match role.to_lowercase().as_str() {
                    "primary" => SessionRole::Primary,
                    "related" => SessionRole::Related,
                    "dependency" => SessionRole::Dependency,
                    "shared" => SessionRole::Shared,
                    _ => {
                        return Err(format!(
                            "Invalid role: {}. Use: primary, related, dependency, shared",
                            role
                        ));
                    }
                };
                let system = init_admin_system().await?;
                system
                    .get_storage()
                    .add_session_to_workspace(ws_id, sess_id, role_enum)
                    .await?;
            }
            println!(
                "Session {} attached to workspace {} as {}",
                session_id, workspace_id, role
            );
            Ok(())
        }
    }
}
