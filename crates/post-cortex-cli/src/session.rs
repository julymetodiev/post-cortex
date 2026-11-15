// Copyright (c) 2025 Julius ML
// MIT License

//! `pcx session` — create / delete / list, through the local daemon.

use crate::cli::SessionAction;
use crate::daemon_client::DaemonClient;
use crate::local::Local;

pub async fn handle_session_action(action: SessionAction, local: &Local) -> Result<(), String> {
    let client = DaemonClient::connect(local).await?;
    match action {
        SessionAction::Create { name, description } => {
            let session = client.create_session(name, description).await?;
            println!("Session created:");
            println!("  ID:          {}", session.id);
            println!("  Name:        {}", session.name);
        }
        SessionAction::Delete { id } => {
            client.delete_session(&id).await?;
            println!("Session {} deleted", id);
        }
        SessionAction::List => {
            let sessions = client.list_sessions().await?;
            println!("Sessions ({})", sessions.len());
            println!("{:<38} {:<20} Name", "ID", "Workspace");
            println!("{:-<38} {:-<20} {:-<30}", "", "", "");
            for s in sessions {
                let ws = s.workspace.as_deref().unwrap_or("-");
                println!("{:<38} {:<20} {}", s.id, ws, s.name);
            }
        }
    }
    Ok(())
}
