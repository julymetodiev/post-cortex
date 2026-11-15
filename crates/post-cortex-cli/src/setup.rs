// Copyright (c) 2025 Julius ML
// MIT License

//! `pcx setup`: sets coding agents up for post-cortex.
//!
//! Global by default: every client with a user-level config gets it there,
//! without ids, and its hooks resolve the project from the directory they
//! run in (`project.rs`). With `--project` (and always for the clients that
//! read their config from the project only) it creates or reuses a session
//! and workspace, pins their ids in `.post-cortex.json` at the project root
//! and writes the clients' project files. Claude Code is the plugin either
//! way (owner 2026-11-06): setup says how to install it, sets its capture
//! switch and takes out what setups before the plugin wrote (`claude.rs`).

use std::env;

use dialoguer::Confirm;

use crate::clients::{self, Client};
use crate::daemon_client::DaemonClient;
use crate::local::Local;
use crate::profiles::{self, KEYS_URL, Profile, SELF_HOSTED, Store};
use crate::project::{self, Ids};
use crate::stdio_proxy::ensure_daemon_running;

mod claude;
mod ids;

pub use claude::SKILL_FILES;

use ids::choose_ids;

/// The backend `pcx setup` uses: a `--daemon-url` / `--capture-url` daemon
/// (self-hosted at that URL), else the profile asked for, else the active
/// one. A remote profile without a key gets `pcx login` first, or an error
/// with `--non-interactive`.
async fn setup_backend(
    store: &Store,
    daemon_url: Option<String>,
    profile: Option<String>,
    non_interactive: bool,
) -> Result<Profile, String> {
    if let Some(url) = daemon_url {
        return Ok(Profile {
            name: SELF_HOSTED.to_string(),
            url,
            token: store.token(SELF_HOSTED)?,
        });
    }
    let backend = store.resolve(profile.as_deref())?;
    if !backend.remote() || backend.token.is_some() {
        return Ok(backend);
    }
    if non_interactive {
        return Err(format!(
            "No API key for profile '{}': run `pcx login --profile {}` first (create a key at {KEYS_URL}), or use --self-hosted",
            backend.name, backend.name
        ));
    }
    println!("No API key for '{}' ({}) yet.", backend.name, backend.url);
    profiles::login_to(store, &backend.name, None).await?;
    println!();
    store.resolve(Some(&backend.name))
}

/// What `pcx setup` was asked for.
pub struct Request {
    /// Set up this project (with ids) rather than every project.
    pub project: bool,
    /// The session name (default: the directory's).
    pub name: Option<String>,
    /// Ask nothing.
    pub non_interactive: bool,
    /// Capture every turn.
    pub capture: bool,
    /// `--daemon-url` (or `--capture-url`).
    pub daemon_url: Option<String>,
    /// The backend profile.
    pub profile: Option<String>,
    /// The coding agents (default: Claude Code).
    pub clients: Vec<Client>,
}

pub async fn handle_setup(request: Request, local: &Local) -> Result<(), String> {
    let Request {
        project,
        name,
        non_interactive,
        capture,
        daemon_url: daemon_url_flag,
        profile,
        clients,
    } = request;
    let mut clients = if clients.is_empty() {
        vec![Client::ClaudeCode]
    } else {
        clients
    };
    clients.dedup();
    let daemon_url_arg = daemon_url_flag
        .map(|url| validate_daemon_url(&url))
        .transpose()?;
    let store = Store::open(local)?;
    let backend = setup_backend(&store, daemon_url_arg, profile, non_interactive).await?;
    // Global unless --project, for the clients that have a user-level config.
    let (global, project_only): (Vec<Client>, Vec<Client>) =
        clients.iter().partition(|c| !project && c.global());

    println!("Post-Cortex Setup");
    println!("=================");
    println!("  Backend: {} ({})", backend.name, backend.url);
    if !project && !project_only.is_empty() {
        let names: Vec<&str> = project_only.iter().map(|c| c.label()).collect();
        println!(
            "  {} read their config from the project only: set up for this project.",
            names.join(", ")
        );
    }
    println!();

    // Conversation capture is opt-in: it stores prompts and replies.
    let capture = capture
        || (!non_interactive
            && Confirm::new()
                .with_prompt(format!(
                    "Capture every conversation turn into the session? (prompts and \
                     replies, kept {} days; tool output is never sent)",
                    local.message_retention_days
                ))
                .default(false)
                .interact()
                .map_err(|e| format!("Input error: {}", e))?);

    let ids = if project_only.is_empty() {
        None
    } else {
        let ids = setup_project(
            &store,
            &backend,
            local,
            name,
            non_interactive,
            capture,
            &project_only,
        )
        .await?;
        Some(ids)
    };
    if !global.is_empty() {
        setup_global(&store, &backend, capture, &global)?;
    }

    println!();
    println!("Setup complete!");
    println!();
    if let Some(ids) = &ids {
        println!("  Session ID:   {}", ids.session_id);
        println!("  Workspace ID: {}", ids.workspace_id);
    }
    println!("  Backend:      {} {}", backend.name, backend.url);
    if capture {
        println!("  Capture:      on (posts turns to the backend)");
    }
    if !backend.remote() && !project_only.is_empty() {
        println!(
            "                if the daemon has an auth_token, export PC_AUTH_TOKEN for Claude Code"
        );
    }
    if !global.is_empty() {
        let names: Vec<&str> = global.iter().map(|c| c.label()).collect();
        println!("  Every project: {}", names.join(", "));
        println!(
            "                 (a project is recognised by its git remote, else its directory; \
             `pcx setup --project` pins it to fixed ids)"
        );
    }
    if !project_only.is_empty() {
        let names: Vec<&str> = project_only.iter().map(|c| c.label()).collect();
        println!("  This project:  {}", names.join(", "));
    }
    Ok(())
}

/// Global setup: each client's user-level files, without ids (its hooks
/// resolve the project from the directory they run in). Claude Code gets
/// the plugin ([`claude::setup_plugin`]).
fn setup_global(
    store: &Store,
    backend: &Profile,
    capture: bool,
    global: &[Client],
) -> Result<(), String> {
    let home = dirs::home_dir().ok_or_else(|| "Cannot determine home directory".to_string())?;
    let ctx = clients::SetupContext {
        project_dir: home,
        session_id: String::new(),
        workspace_id: String::new(),
        daemon_url: backend.url.clone(),
        capture,
        profile: backend.name.clone(),
        remote: backend.remote(),
        global: true,
    };
    for client in global {
        println!("  {} (every project):", client.label());
        if *client == Client::ClaudeCode {
            for line in claude::setup_plugin(store, backend, capture)? {
                println!("  {line}");
            }
            continue;
        }
        match clients::install(*client, &ctx) {
            Ok(lines) => lines.iter().for_each(|line| println!("  {line}")),
            Err(e) => println!("    skipped: {e}"),
        }
    }
    Ok(())
}

/// Project setup: the session and workspace (created or reused), their ids
/// in `.post-cortex.json` at the project root, every client's project
/// files, and for Claude Code the plugin with what setups before it wrote
/// taken out. Returns the ids.
async fn setup_project(
    store: &Store,
    backend: &Profile,
    local: &Local,
    name: Option<String>,
    non_interactive: bool,
    capture: bool,
    project_clients: &[Client],
) -> Result<Ids, String> {
    // A remote backend, or a daemon at another URL than daemon.toml's
    // (`--daemon-url`), is reached as it is; the daemon in daemon.toml is
    // started when it does not run.
    let daemon = if backend.remote() || backend.url != local.url() {
        DaemonClient::for_profile(backend)?
    } else {
        ensure_daemon_running(local).await?;
        DaemonClient::connect(local).await?
    };
    let cwd = env::current_dir().map_err(|e| format!("Cannot get cwd: {}", e))?;
    let ids = choose_ids(backend, &daemon, name, non_interactive).await?;

    let root = project::project_root(&cwd);
    let pinned = project::write_pinned(&root, &ids)?;
    let ctx = clients::SetupContext {
        project_dir: cwd.clone(),
        session_id: ids.session_id.clone(),
        workspace_id: ids.workspace_id.clone(),
        daemon_url: backend.url.clone(),
        capture,
        profile: backend.name.clone(),
        remote: backend.remote(),
        global: false,
    };
    println!(
        "{} (the ids other agents and teammates resolve to)",
        clients::common::report(pinned, &root.join(project::PINNED_FILE), &ctx)
    );

    if project_clients.contains(&Client::ClaudeCode) {
        println!("  Claude Code:");
        let retired = claude::retire_project(&root)?;
        for line in retired
            .iter()
            .chain(&claude::setup_plugin(store, backend, capture)?)
        {
            println!("  {line}");
        }
    }
    for client in project_clients.iter().filter(|c| **c != Client::ClaudeCode) {
        println!("  {}:", client.label());
        match clients::install(*client, &ctx) {
            Ok(lines) => lines.iter().for_each(|line| println!("  {line}")),
            Err(e) => println!("    skipped: {e}"),
        }
    }
    Ok(ids)
}

/// A `--daemon-url`: an http(s) base URL, without a trailing slash. It
/// goes into a shell command, so anything but URL characters is refused.
fn validate_daemon_url(url: &str) -> Result<String, String> {
    profiles::validate_url(url).map_err(|e| format!("--daemon-url: {e}"))
}

#[cfg(test)]
mod tests;
