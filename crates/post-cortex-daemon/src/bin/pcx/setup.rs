// Copyright (c) 2025 Julius ML
// MIT License

//! `pcx setup` — interactive project bootstrap wizard.
//!
//! Creates (or reuses) a session + workspace, writes CLAUDE.md / .claude/settings.json
//! / hook scripts into the current directory, and installs the post-cortex-agents
//! sub-agent bundle under `~/.claude/agents/`. With capture on, a Stop hook also
//! posts every conversation turn to the daemon's message stream.

use std::env;

use dialoguer::{Confirm, Input, Select};
use post_cortex_core::workspace::SessionRole;
use post_cortex_daemon::daemon::{DaemonConfig, is_daemon_running};
use uuid::Uuid;

use super::admin::init_admin_system;
use super::daemon_client::{DaemonClient, SessionInfo, WorkspaceInfo};

pub async fn handle_setup(
    name: Option<String>,
    non_interactive: bool,
    capture: bool,
    daemon_url_flag: Option<String>,
) -> Result<(), String> {
    let daemon_url_arg = daemon_url_flag
        .map(|url| validate_daemon_url(&url))
        .transpose()?;
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

    // Conversation capture is opt-in: it stores prompts and replies.
    let capture = capture
        || (!non_interactive
            && Confirm::new()
                .with_prompt(format!(
                    "Capture every conversation turn into the session? (prompts and \
                     replies, kept {} days; tool output is never sent)",
                    config.message_retention_days
                ))
                .default(false)
                .interact()
                .map_err(|e| format!("Input error: {}", e))?);
    // The daemon the hooks talk to (cards always, capture when on).
    let hooks_url = daemon_url_arg.unwrap_or_else(|| daemon_url(&config));

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

    // .claude/settings.json — add the PCX hooks it lacks, create it if missing
    let claude_dir = cwd.join(".claude");
    std::fs::create_dir_all(&claude_dir)
        .map_err(|e| format!("Failed to create .claude/: {}", e))?;
    let settings_path = claude_dir.join("settings.json");
    let pcx_settings = generate_settings_json(&session_id, &workspace_id, &hooks_url, capture);
    if settings_path.exists() {
        let existing =
            std::fs::read_to_string(&settings_path).map_err(|e| format!("Read error: {}", e))?;
        match serde_json::from_str::<serde_json::Value>(&existing) {
            Ok(mut existing_json) => {
                let added = merge_hooks(&mut existing_json, &pcx_settings["hooks"])?;
                if added == 0 {
                    println!("  .claude/settings.json already has the PCX hooks, skipping");
                } else {
                    let merged = serde_json::to_string_pretty(&existing_json)
                        .map_err(|e| format!("JSON error: {}", e))?;
                    std::fs::write(&settings_path, format!("{}\n", merged))
                        .map_err(|e| format!("Write error: {}", e))?;
                    println!("  Updated: .claude/settings.json (added {} hooks)", added);
                }
            }
            Err(_) => {
                println!("  .claude/settings.json exists but invalid JSON, skipping");
            }
        }
    } else {
        let settings_json = serde_json::to_string_pretty(&pcx_settings)
            .map_err(|e| format!("JSON error: {}", e))?;
        std::fs::write(&settings_path, format!("{}\n", settings_json))
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

    // .claude/hooks/pcx-stop-check.py (Stop hook; session id comes from settings.json)
    let stop_hook_path = hooks_dir.join("pcx-stop-check.py");
    if stop_hook_path.exists() {
        println!("  .claude/hooks/pcx-stop-check.py already exists, skipping");
    } else {
        std::fs::write(
            &stop_hook_path,
            include_str!("../templates/pcx-stop-check.py"),
        )
        .map_err(|e| format!("Failed to write hook script: {}", e))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&stop_hook_path, std::fs::Permissions::from_mode(0o755))
                .map_err(|e| format!("Failed to set permissions: {}", e))?;
        }
        println!("  Created: .claude/hooks/pcx-stop-check.py");
    }

    // .claude/hooks/pcx-card.py (SessionStart hook printing the user and
    // project cards)
    let card_path = hooks_dir.join("pcx-card.py");
    if card_path.exists() {
        println!("  .claude/hooks/pcx-card.py already exists, skipping");
    } else {
        std::fs::write(&card_path, include_str!("../templates/pcx-card.py"))
            .map_err(|e| format!("Failed to write hook script: {}", e))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&card_path, std::fs::Permissions::from_mode(0o755))
                .map_err(|e| format!("Failed to set permissions: {}", e))?;
        }
        println!("  Created: .claude/hooks/pcx-card.py");
    }

    // .claude/hooks/pcx-capture.py (Stop hook posting turns to the daemon)
    if capture {
        let capture_path = hooks_dir.join("pcx-capture.py");
        if capture_path.exists() {
            println!("  .claude/hooks/pcx-capture.py already exists, skipping");
        } else {
            std::fs::write(&capture_path, include_str!("../templates/pcx-capture.py"))
                .map_err(|e| format!("Failed to write hook script: {}", e))?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&capture_path, std::fs::Permissions::from_mode(0o755))
                    .map_err(|e| format!("Failed to set permissions: {}", e))?;
            }
            println!("  Created: .claude/hooks/pcx-capture.py");
        }
    }

    // 6. Install agents to ~/.claude/agents/post-cortex-agents/
    let agents_dir = dirs::home_dir()
        .ok_or_else(|| "Cannot determine home directory".to_string())?
        .join(".claude/agents/post-cortex-agents");

    let agent_files: &[(&str, &str)] = &[
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

    // 7. Install the post-cortex-memory skill to ~/.claude/skills/ so Claude Code
    // can load it in any project. An existing copy is left alone: it may have
    // been edited, and deleting the directory reinstalls the bundled version.
    let skill_dir = dirs::home_dir()
        .ok_or_else(|| "Cannot determine home directory".to_string())?
        .join(".claude/skills/post-cortex-memory");
    if skill_dir.join("SKILL.md").exists() {
        println!(
            "  ~/.claude/skills/post-cortex-memory already exists, skipping (delete it to reinstall)"
        );
    } else {
        for (rel_path, content) in SKILL_FILES {
            let target = skill_dir.join(rel_path);
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| format!("Failed to create {}: {}", parent.display(), e))?;
            }
            std::fs::write(&target, content)
                .map_err(|e| format!("Failed to write {}: {}", rel_path, e))?;
        }
        println!("  Created: ~/.claude/skills/post-cortex-memory/ (post-cortex usage skill)");
    }

    println!();
    println!("Setup complete!");
    println!();
    println!("  Session ID:   {}", session_id);
    println!("  Workspace ID: {}", workspace_id);
    println!("  Daemon:       {} (cards on session start)", hooks_url);
    if capture {
        println!("  Capture:      on (posts turns to the daemon)");
    }
    println!(
        "                if the daemon has an auth_token, export PC_AUTH_TOKEN for Claude Code"
    );
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

/// The post-cortex-memory skill bundled into `pcx`. The canonical copy lives in
/// `skills/post-cortex-memory/` at the repository root; `templates/skills/` is
/// kept identical (checked by `bundled_skill_matches_repository_copy`).
const SKILL_FILES: &[(&str, &str)] = &[
    (
        "SKILL.md",
        include_str!("../templates/skills/post-cortex-memory/SKILL.md"),
    ),
    (
        "references/tools.md",
        include_str!("../templates/skills/post-cortex-memory/references/tools.md"),
    ),
    (
        "references/setup.md",
        include_str!("../templates/skills/post-cortex-memory/references/setup.md"),
    ),
];

fn generate_claude_md(session_id: &str, workspace_id: &str) -> String {
    include_str!("../templates/CLAUDE.md")
        .replace("<SESSION_ID>", session_id)
        .replace("<WORKSPACE_ID>", workspace_id)
}

/// The PCX hooks for `.claude/settings.json`: the template's, the
/// SessionStart hook that prints the cards from the daemon at `url`, and
/// with `capture` the Stop hook that posts conversation turns there.
fn generate_settings_json(
    session_id: &str,
    workspace_id: &str,
    url: &str,
    capture: bool,
) -> serde_json::Value {
    let mut settings: serde_json::Value = serde_json::from_str(
        &include_str!("../templates/settings.json").replace("<SESSION_ID>", session_id),
    )
    .expect("templates/settings.json is valid JSON");
    if let Some(start) = settings["hooks"]["SessionStart"].as_array_mut() {
        start.push(serde_json::json!({
            "hooks": [{
                "type": "command",
                "command": format!(
                    "python3 -I \"$CLAUDE_PROJECT_DIR\"/.claude/hooks/pcx-card.py {workspace_id} {url}"
                ),
            }]
        }));
    }
    if capture && let Some(stop) = settings["hooks"]["Stop"].as_array_mut() {
        stop.push(serde_json::json!({
            "hooks": [{
                "type": "command",
                "command": format!(
                    "python3 -I \"$CLAUDE_PROJECT_DIR\"/.claude/hooks/pcx-capture.py {session_id} {url}"
                ),
            }]
        }));
    }
    settings
}

/// What identifies a PCX hook command across setups: its script name
/// (`pcx-stop-check.py`, ...), or the `[PCX]` banner of the prompt hook.
/// The session id or daemon URL in it may differ.
fn hook_key(command: &str) -> String {
    if let Some(start) = command.find("pcx-") {
        let script: String = command[start..]
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_'))
            .collect();
        if script.ends_with(".py") || script.ends_with(".sh") {
            return script;
        }
    }
    if command.contains("[PCX]") {
        return "[PCX]".to_string();
    }
    command.to_string()
}

/// Add to `settings.hooks` every hook group of `pcx_hooks` whose commands
/// it has none of (by [`hook_key`]), leaving everything else as it is.
/// Returns how many groups were added.
fn merge_hooks(
    settings: &mut serde_json::Value,
    pcx_hooks: &serde_json::Value,
) -> Result<usize, String> {
    let Some(settings) = settings.as_object_mut() else {
        return Err(".claude/settings.json is not a JSON object".to_string());
    };
    let hooks = settings
        .entry("hooks")
        .or_insert_with(|| serde_json::json!({}))
        .as_object_mut()
        .ok_or_else(|| "\"hooks\" in .claude/settings.json is not an object".to_string())?;
    let commands = |group: &serde_json::Value| -> Vec<String> {
        group["hooks"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|h| h["command"].as_str().map(hook_key))
            .collect()
    };
    let mut added = 0;
    for (event, groups) in pcx_hooks.as_object().into_iter().flatten() {
        let existing = hooks
            .entry(event.clone())
            .or_insert_with(|| serde_json::json!([]))
            .as_array_mut()
            .ok_or_else(|| format!("hooks.{event} in .claude/settings.json is not an array"))?;
        for group in groups.as_array().into_iter().flatten() {
            let present: Vec<String> = existing.iter().flat_map(&commands).collect();
            if commands(group).iter().all(|key| !present.contains(key)) {
                existing.push(group.clone());
                added += 1;
            }
        }
    }
    Ok(added)
}

/// A `--daemon-url`: an http(s) base URL, without a trailing slash. It
/// goes into a shell command, so anything but URL characters is refused.
fn validate_daemon_url(url: &str) -> Result<String, String> {
    let url = url.trim().trim_end_matches('/');
    let ok = (url.starts_with("http://") || url.starts_with("https://"))
        && url.len() > "https://".len()
        && url
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-._~:/[]%".contains(c));
    if ok {
        Ok(url.to_string())
    } else {
        Err(format!(
            "--daemon-url must be an http(s) URL such as http://host:3737, got '{url}'"
        ))
    }
}

/// Base URL hooks use to reach the daemon (a wildcard bind is reached on
/// loopback).
pub(crate) fn daemon_url(config: &DaemonConfig) -> String {
    let host = match config.host.as_str() {
        "0.0.0.0" | "::" | "" => "127.0.0.1",
        host => host,
    };
    if host.contains(':') {
        format!("http://[{}]:{}", host, config.port)
    } else {
        format!("http://{}:{}", host, config.port)
    }
}

fn generate_compact_reinject_script(session_id: &str, workspace_id: &str) -> String {
    include_str!("../templates/pcx-compact-reinject.sh")
        .replace("<SESSION_ID>", session_id)
        .replace("<WORKSPACE_ID>", workspace_id)
}

#[cfg(test)]
mod tests {
    use super::{SKILL_FILES, generate_settings_json, merge_hooks, validate_daemon_url};

    #[test]
    fn daemon_url_must_be_a_plain_http_url() {
        assert_eq!(
            validate_daemon_url("http://ml.example.ts.net:3737/").unwrap(),
            "http://ml.example.ts.net:3737"
        );
        assert!(validate_daemon_url("https://[::1]:3737").is_ok());
        assert!(validate_daemon_url("ml:3737").is_err());
        assert!(validate_daemon_url("http://x; rm -rf ~").is_err());
        assert!(validate_daemon_url("http://x$(id)").is_err());
    }
    use std::path::Path;

    /// The bundled skill must not drift from `skills/post-cortex-memory/`.
    /// Skipped when the repository copy is absent (e.g. building from the
    /// crates.io package).
    #[test]
    fn bundled_skill_matches_repository_copy() {
        let repo_skill =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../skills/post-cortex-memory");
        if !repo_skill.exists() {
            return;
        }
        for (rel_path, bundled) in SKILL_FILES {
            let canonical = std::fs::read_to_string(repo_skill.join(rel_path))
                .unwrap_or_else(|e| panic!("read skills/post-cortex-memory/{rel_path}: {e}"));
            assert_eq!(
                *bundled, canonical,
                "templates/skills/post-cortex-memory/{rel_path} differs from skills/post-cortex-memory/{rel_path}; copy it over"
            );
        }
    }

    fn commands(settings: &serde_json::Value, event: &str) -> Vec<String> {
        settings["hooks"][event]
            .as_array()
            .into_iter()
            .flatten()
            .flat_map(|g| g["hooks"].as_array().cloned().unwrap_or_default())
            .filter_map(|h| h["command"].as_str().map(String::from))
            .collect()
    }

    const URL: &str = "http://127.0.0.1:3737";

    #[test]
    fn card_hook_always_capture_hook_when_on() {
        let plain = generate_settings_json("S1", "W1", URL, false);
        assert_eq!(commands(&plain, "Stop").len(), 1);
        let start = commands(&plain, "SessionStart");
        assert_eq!(start.len(), 2, "compaction re-inject and cards");
        assert!(
            start[1].ends_with("pcx-card.py W1 http://127.0.0.1:3737"),
            "{}",
            start[1]
        );

        let with = generate_settings_json("S1", "W1", URL, true);
        let stop = commands(&with, "Stop");
        assert_eq!(stop.len(), 2);
        assert!(
            stop[1].ends_with("pcx-capture.py S1 http://127.0.0.1:3737"),
            "{}",
            stop[1]
        );
    }

    /// Re-running setup adds only the hooks a project lacks: here capture,
    /// on a project set up before it existed (another session id), with a
    /// hook of its own that must stay.
    #[test]
    fn merge_adds_missing_hooks_only() {
        let mut settings = generate_settings_json("OLD", "W", URL, false);
        settings["hooks"]["Stop"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({"hooks": [{"type": "command", "command": "my-own-hook"}]}));
        settings["permissions"] = serde_json::json!({"allow": ["Bash(ls)"]});

        let pcx = generate_settings_json("NEW", "W", URL, true);
        assert_eq!(merge_hooks(&mut settings, &pcx["hooks"]).unwrap(), 1);
        let stop = commands(&settings, "Stop");
        assert_eq!(stop.len(), 3, "{stop:?}");
        assert!(stop[0].contains("pcx-stop-check.py OLD"));
        assert_eq!(stop[1], "my-own-hook");
        assert!(stop[2].contains("pcx-capture.py NEW"));
        assert_eq!(commands(&settings, "UserPromptSubmit").len(), 1);
        assert_eq!(settings["permissions"]["allow"][0], "Bash(ls)");

        assert_eq!(merge_hooks(&mut settings, &pcx["hooks"]).unwrap(), 0);

        let mut empty = serde_json::json!({});
        assert_eq!(merge_hooks(&mut empty, &pcx["hooks"]).unwrap(), 5);
    }
}
