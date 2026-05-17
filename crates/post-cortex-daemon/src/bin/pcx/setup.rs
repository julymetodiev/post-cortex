// Copyright (c) 2025 Julius ML
// MIT License

//! `pcx setup` — interactive project bootstrap wizard.
//!
//! Creates (or reuses) a session + workspace, writes CLAUDE.md / .claude/settings.json
//! / hook scripts into the current directory, and installs the post-cortex-agents
//! sub-agent bundle under `~/.claude/agents/`.

use std::env;

use dialoguer::{Input, Select};
use post_cortex_core::workspace::SessionRole;
use post_cortex_daemon::daemon::{DaemonConfig, is_daemon_running};
use uuid::Uuid;

use super::admin::init_admin_system;
use super::daemon_client::{DaemonClient, SessionInfo, WorkspaceInfo};

pub async fn handle_setup(name: Option<String>, non_interactive: bool) -> Result<(), String> {
    let config = DaemonConfig::load();
    let use_daemon = is_daemon_running(&config.host, config.port).await;

    println!("Post-Cortex Project Setup");
    println!("=========================");
    println!();

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

    // 2. Fetch existing sessions and workspaces
    let (existing_sessions, existing_workspaces) = if use_daemon {
        let client = DaemonClient::new(&config);
        let sessions = client.list_sessions().await.unwrap_or_default();
        let workspaces = client.list_workspaces().await.unwrap_or_default();
        (sessions, workspaces)
    } else {
        let system = init_admin_system().await?;
        let session_ids = system.list_sessions().await.unwrap_or_default();
        let workspaces_raw = system
            .get_storage()
            .list_all_workspaces()
            .await
            .unwrap_or_default();

        let mut sessions = Vec::new();
        for id in session_ids {
            if let Ok(session_arc) = system.get_session(id).await {
                let session = session_arc.load();
                sessions.push(SessionInfo {
                    id: id.to_string(),
                    name: session.name().unwrap_or_else(|| "Unnamed".to_string()),
                    workspace: None,
                });
            }
        }

        let workspaces = workspaces_raw
            .into_iter()
            .map(|ws| WorkspaceInfo {
                id: ws.id.to_string(),
                name: ws.name,
                description: ws.description,
                session_count: ws.sessions.len(),
            })
            .collect();

        (sessions, workspaces)
    };

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
                .items(&["Use existing session", "Create new session"])
                .default(0)
                .interact()
                .map_err(|e| format!("Selection error: {}", e))?;

            if choice == 0 {
                println!("  Session reused: {} ({})", existing.name, existing.id);
                existing.id.clone()
            } else {
                create_session_for_setup(&config, use_daemon, &project_name).await?
            }
        }
    } else {
        create_session_for_setup(&config, use_daemon, &project_name).await?
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
                .items(&["Use existing workspace", "Create new workspace"])
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
                let (id, name) = create_workspace_for_setup(&config, use_daemon, &ws_name).await?;
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
            let (id, name) = create_workspace_for_setup(&config, use_daemon, &ws_name).await?;
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
        let (id, name) = create_workspace_for_setup(&config, use_daemon, &ws_name).await?;
        workspace_id = id;
        workspace_name = name;
    };

    println!("  Workspace: {} ({})", workspace_name, workspace_id);

    // 4. Attach session to workspace
    if use_daemon {
        let client = DaemonClient::new(&config);
        client
            .attach_session(&workspace_id, &session_id, "primary")
            .await?;
    } else {
        let ws_uuid = Uuid::parse_str(&workspace_id).map_err(|e| format!("Invalid UUID: {}", e))?;
        let sess_uuid = Uuid::parse_str(&session_id).map_err(|e| format!("Invalid UUID: {}", e))?;
        let system = init_admin_system().await?;
        system
            .get_storage()
            .add_session_to_workspace(ws_uuid, sess_uuid, SessionRole::Primary)
            .await?;
    }
    println!("  Session attached to workspace (role: primary)");
    println!();

    // 5. Generate files
    let cwd = env::current_dir().map_err(|e| format!("Cannot get cwd: {}", e))?;

    // CLAUDE.md — prepend PCX section if file exists, create if not
    let pcx_section = generate_claude_md(&session_id, &workspace_id);
    let claude_path = cwd.join("CLAUDE.md");
    if claude_path.exists() {
        let existing =
            std::fs::read_to_string(&claude_path).map_err(|e| format!("Read error: {}", e))?;
        if existing.contains("<SESSION_ID>") || existing.contains(&session_id) {
            println!("  CLAUDE.md already has PCX config, skipping");
        } else {
            // Prepend PCX section (without # CLAUDE.md heading), keep existing content
            let pcx_body = pcx_section
                .strip_prefix("# CLAUDE.md\n")
                .unwrap_or(&pcx_section);
            // Insert PCX section after the first heading line
            let merged = if let Some(first_newline) = existing.find('\n') {
                let (heading, rest) = existing.split_at(first_newline);
                format!("{}\n{}\n---\n{}", heading, pcx_body.trim_end(), rest)
            } else {
                format!("{}\n\n{}", existing, pcx_body.trim_end())
            };
            std::fs::write(&claude_path, &merged)
                .map_err(|e| format!("Failed to write CLAUDE.md: {}", e))?;
            println!("  Updated: CLAUDE.md (prepended PCX session config)");
        }
    } else {
        std::fs::write(&claude_path, &pcx_section)
            .map_err(|e| format!("Failed to write CLAUDE.md: {}", e))?;
        println!("  Created: CLAUDE.md");
    }

    // .claude/settings.json — merge hooks if file exists, create if not
    let claude_dir = cwd.join(".claude");
    std::fs::create_dir_all(&claude_dir)
        .map_err(|e| format!("Failed to create .claude/: {}", e))?;
    let settings_path = claude_dir.join("settings.json");
    if settings_path.exists() {
        let existing =
            std::fs::read_to_string(&settings_path).map_err(|e| format!("Read error: {}", e))?;
        match serde_json::from_str::<serde_json::Value>(&existing) {
            Ok(mut existing_json) => {
                if existing_json.get("hooks").is_some() {
                    println!("  .claude/settings.json already has hooks, skipping");
                } else {
                    // Parse PCX hooks and merge into existing
                    let pcx_settings = generate_settings_json(&session_id);
                    if let Ok(pcx_json) = serde_json::from_str::<serde_json::Value>(&pcx_settings)
                        && let Some(hooks) = pcx_json.get("hooks")
                    {
                        existing_json
                            .as_object_mut()
                            .unwrap()
                            .insert("hooks".to_string(), hooks.clone());
                        let merged = serde_json::to_string_pretty(&existing_json)
                            .map_err(|e| format!("JSON error: {}", e))?;
                        std::fs::write(&settings_path, format!("{}\n", merged))
                            .map_err(|e| format!("Write error: {}", e))?;
                        println!("  Updated: .claude/settings.json (added hooks)");
                    }
                }
            }
            Err(_) => {
                println!("  .claude/settings.json exists but invalid JSON, skipping");
            }
        }
    } else {
        let settings_json = generate_settings_json(&session_id);
        std::fs::write(&settings_path, &settings_json)
            .map_err(|e| format!("Failed to write settings.json: {}", e))?;
        println!("  Created: .claude/settings.json");
    }

    // .claude/hooks/pcx-compact-reinject.sh
    let hooks_dir = claude_dir.join("hooks");
    std::fs::create_dir_all(&hooks_dir)
        .map_err(|e| format!("Failed to create .claude/hooks/: {}", e))?;
    let hook_path = hooks_dir.join("pcx-compact-reinject.sh");
    if hook_path.exists() {
        println!("  .claude/hooks/pcx-compact-reinject.sh already exists, skipping");
    } else {
        let hook_script = generate_compact_reinject_script(&session_id, &workspace_id);
        std::fs::write(&hook_path, &hook_script)
            .map_err(|e| format!("Failed to write hook script: {}", e))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&hook_path, std::fs::Permissions::from_mode(0o755))
                .map_err(|e| format!("Failed to set permissions: {}", e))?;
        }
        println!("  Created: .claude/hooks/pcx-compact-reinject.sh");
    }

    // 6. Install agents to ~/.claude/agents/post-cortex-agents/
    let agents_dir = dirs::home_dir()
        .ok_or_else(|| "Cannot determine home directory".to_string())?
        .join(".claude/agents/post-cortex-agents");

    let agent_files: &[(&str, &str)] = &[
        ("SKILL.md", include_str!("../templates/agents/SKILL.md")),
        (
            "error-handling.md",
            include_str!("../templates/agents/error-handling.md"),
        ),
        (
            "agents/context-builder.md",
            include_str!("../templates/agents/agents/context-builder.md"),
        ),
        (
            "agents/knowledge-analyst.md",
            include_str!("../templates/agents/agents/knowledge-analyst.md"),
        ),
        (
            "agents/memory-curator.md",
            include_str!("../templates/agents/agents/memory-curator.md"),
        ),
        (
            "agents/search-specialist.md",
            include_str!("../templates/agents/agents/search-specialist.md"),
        ),
    ];

    std::fs::create_dir_all(agents_dir.join("agents"))
        .map_err(|e| format!("Failed to create agents directory: {}", e))?;

    for (rel_path, content) in agent_files {
        let target = agents_dir.join(rel_path);
        if target.exists() {
            println!(
                "  ~/.claude/agents/post-cortex-agents/{} already exists, skipping",
                rel_path
            );
        } else {
            std::fs::write(&target, content)
                .map_err(|e| format!("Failed to write {}: {}", rel_path, e))?;
            println!(
                "  Created: ~/.claude/agents/post-cortex-agents/{}",
                rel_path
            );
        }
    }

    println!();
    println!("Setup complete!");
    println!();
    println!("  Session ID:   {}", session_id);
    println!("  Workspace ID: {}", workspace_id);
    println!();
    println!("Start coding with: claude");

    Ok(())
}

async fn create_session_for_setup(
    config: &DaemonConfig,
    use_daemon: bool,
    name: &str,
) -> Result<String, String> {
    let id = if use_daemon {
        let client = DaemonClient::new(config);
        let session = client
            .create_session(
                Some(name.to_string()),
                Some(format!("Development session for {}", name)),
            )
            .await?;
        session.id
    } else {
        let system = init_admin_system().await?;
        let id = system
            .create_session(
                Some(name.to_string()),
                Some(format!("Development session for {}", name)),
            )
            .await?;
        id.to_string()
    };
    println!("  Session created: {} ({})", name, id);
    Ok(id)
}

async fn create_workspace_for_setup(
    config: &DaemonConfig,
    use_daemon: bool,
    name: &str,
) -> Result<(String, String), String> {
    if use_daemon {
        let client = DaemonClient::new(config);
        let ws = client
            .create_workspace(name.to_string(), format!("{} workspace", name))
            .await?;
        Ok((ws.id, ws.name))
    } else {
        let system = init_admin_system().await?;
        let id = Uuid::new_v4();
        let description = format!("{} workspace", name);
        system
            .get_storage()
            .save_workspace_metadata(id, name, &description, &[])
            .await?;
        Ok((id.to_string(), name.to_string()))
    }
}

fn generate_claude_md(session_id: &str, workspace_id: &str) -> String {
    include_str!("../templates/CLAUDE.md")
        .replace("<SESSION_ID>", session_id)
        .replace("<WORKSPACE_ID>", workspace_id)
}

fn generate_settings_json(session_id: &str) -> String {
    include_str!("../templates/settings.json").replace("<SESSION_ID>", session_id)
}

fn generate_compact_reinject_script(session_id: &str, workspace_id: &str) -> String {
    include_str!("../templates/pcx-compact-reinject.sh")
        .replace("<SESSION_ID>", session_id)
        .replace("<WORKSPACE_ID>", workspace_id)
}
