// Copyright (c) 2025 Julius ML
// MIT License

//! `pcx workspace` — create / delete / list / attach, through the local
//! daemon.

use crate::cli::WorkspaceAction;
use crate::daemon_client::DaemonClient;
use crate::local::Local;

pub async fn handle_workspace_action(action: WorkspaceAction, local: &Local) -> Result<(), String> {
    let client = DaemonClient::connect(local).await?;
    match action {
        WorkspaceAction::Create { name, description } => {
            let ws = client.create_workspace(name, description).await?;
            println!("Workspace created:");
            println!("  ID:          {}", ws.id);
            println!("  Name:        {}", ws.name);
            println!("  Description: {}", ws.description);
        }
        WorkspaceAction::Delete { id } => {
            client.delete_workspace(&id).await?;
            println!("Workspace {} deleted", id);
        }
        WorkspaceAction::List => {
            let workspaces = client.list_workspaces().await?;
            println!("Workspaces ({})", workspaces.len());
            println!("{:<38} {:<20} Sessions", "ID", "Name");
            println!("{:-<38} {:-<20} {:-<10}", "", "", "");
            for ws in workspaces {
                println!("{:<38} {:<20} {}", ws.id, ws.name, ws.session_count);
            }
        }
        WorkspaceAction::Attach {
            workspace_id,
            session_id,
            role,
        } => {
            client
                .attach_session(&workspace_id, &session_id, &role)
                .await?;
            println!(
                "Session {} attached to workspace {} as {}",
                session_id, workspace_id, role
            );
        }
    }
    Ok(())
}
