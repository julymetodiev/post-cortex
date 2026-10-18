// Copyright (c) 2025 Julius ML
// MIT License

//! `pcx session` — create / delete / list.

use post_cortex_daemon::daemon::{DaemonConfig, is_daemon_running};
use uuid::Uuid;

use super::admin::init_admin_system;
use super::cli::SessionAction;
use super::daemon_client::DaemonClient;

pub async fn handle_session_action(action: SessionAction) -> Result<(), String> {
    let config = DaemonConfig::load();
    let use_daemon = is_daemon_running(&config.host, config.port).await;

    match action {
        SessionAction::Create { name, description } => {
            if use_daemon {
                let client = DaemonClient::new(&config);
                let session = client.create_session(name, description).await?;
                println!("Session created:");
                println!("  ID:          {}", session.id);
                println!("  Name:        {}", session.name);
            } else {
                let system = init_admin_system().await?;
                let id = system
                    .create_session(name.clone(), description.clone())
                    .await?;
                println!("Session created:");
                println!("  ID:          {}", id);
                if let Some(n) = name {
                    println!("  Name:        {}", n);
                }
                if let Some(d) = description {
                    println!("  Description: {}", d);
                }
            }
            Ok(())
        }

        SessionAction::Delete { id } => {
            if use_daemon {
                let client = DaemonClient::new(&config);
                client.delete_session(&id).await?;
            } else {
                let uuid = Uuid::parse_str(&id).map_err(|e| format!("Invalid UUID: {}", e))?;
                let system = init_admin_system().await?;
                system.delete_session(uuid).await?;
            }
            println!("Session {} deleted", id);
            Ok(())
        }

        SessionAction::List => {
            if use_daemon {
                let client = DaemonClient::new(&config);
                let sessions = client.list_sessions().await?;
                println!("Sessions ({})", sessions.len());
                println!("{:<38} {:<20} Name", "ID", "Workspace");
                println!("{:-<38} {:-<20} {:-<30}", "", "", "");
                for s in sessions {
                    let ws = s.workspace.as_deref().unwrap_or("-");
                    println!("{:<38} {:<20} {}", s.id, ws, s.name);
                }
            } else {
                let system = init_admin_system().await?;
                let summaries = system.list_session_summaries().await?;

                let workspaces = system.workspace_manager.list_workspaces();
                let mut session_workspace_map = std::collections::HashMap::new();

                for ws in workspaces {
                    for (session_id, _role) in ws.get_all_sessions() {
                        session_workspace_map.insert(session_id, ws.name.clone());
                    }
                }

                println!("Sessions ({})", summaries.len());
                println!("{:<38} {:<20} Name", "ID", "Workspace");
                println!("{:-<38} {:-<20} {:-<30}", "", "", "");

                for summary in summaries {
                    let workspace_name = session_workspace_map
                        .get(&summary.id)
                        .map(|s| s.as_str())
                        .unwrap_or("-");
                    let session_name = summary.name.as_deref().unwrap_or("Unnamed");
                    println!("{:<38} {:<20} {}", summary.id, workspace_name, session_name);
                }
            }
            Ok(())
        }
    }
}
