// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The `pcx` client (ARCHITECTURE section 3, `post-cortex-cli`): backend
//! profiles and login, coding-agent setup and their hooks, the stdio MCP
//! bridge, and the card, session and workspace commands. It talks to a
//! daemon or the cloud over their public APIs only and never opens storage;
//! the `pcx` binary (in `post-cortex-daemon`) runs the daemon's own
//! commands and hands every client command to [`run`].

#![forbid(unsafe_code)]

mod agent;
mod card;
pub mod cli;
mod clients;
mod connect;
mod daemon_client;
mod hook;
mod local;
mod profiles;
mod project;
mod session;
mod setup;
mod stdio_proxy;
mod workspace;

pub use local::{ClientError, Local, bearer_client, env_text};
pub use stdio_proxy::{ensure_daemon_running, is_daemon_running, run_stdio_proxy};

use cli::ClientCommand;

/// Run a client command against `local` (the daemon `daemon.toml` names)
/// or the backend profile it chooses.
///
/// # Errors
/// What the command reports.
pub async fn run(command: ClientCommand, local: &Local) -> Result<(), String> {
    match command {
        ClientCommand::Setup {
            project,
            name,
            non_interactive,
            capture,
            daemon_url,
            capture_url,
            profile,
            self_hosted,
            clients,
        } => {
            let profile = if self_hosted {
                Some(profiles::SELF_HOSTED.to_string())
            } else {
                profile
            };
            let request = setup::Request {
                project,
                name,
                non_interactive,
                capture: capture || capture_url.is_some(),
                daemon_url: capture_url.or(daemon_url),
                profile,
                clients,
            };
            setup::handle_setup(request, local).await
        }
        ClientCommand::McpConfig {
            client,
            url,
            profile,
        } => {
            let (name, daemon_url) = match url {
                Some(url) => (
                    profiles::SELF_HOSTED.to_string(),
                    profiles::validate_url(&url)?,
                ),
                None => {
                    let p = profiles::Store::open(local)?.resolve(profile.as_deref())?;
                    (p.name, p.url)
                }
            };
            let ctx = clients::SetupContext {
                project_dir: std::env::current_dir().map_err(|e| e.to_string())?,
                session_id: "<SESSION_ID>".into(),
                workspace_id: "<WORKSPACE_ID>".into(),
                daemon_url,
                capture: false,
                remote: name != profiles::SELF_HOSTED,
                profile: name,
                global: false,
            };
            print!("{}", clients::mcp_config(client, &ctx));
            Ok(())
        }
        ClientCommand::Mcp { profile, author } => {
            let store = profiles::Store::open(local)?;
            let backend = store.resolve(profile.as_deref())?;
            let author = match author {
                Some(author) => Some(author),
                None => env_text("PC_AUTHOR")?,
            };
            let client = bearer_client(backend.token.as_deref(), author.as_deref(), None)
                .map_err(|e| e.to_string())?;
            if backend.remote() {
                if backend.token.is_none() {
                    return Err(format!(
                        "No API key for profile '{}': run `pcx login --profile {}`",
                        backend.name, backend.name
                    ));
                }
            } else if backend.url == local.url() {
                // The daemon in daemon.toml is started when it is not running.
                ensure_daemon_running(local).await?;
            }
            stdio_proxy::proxy_stdio(backend.mcp_url(), client).await
        }
        ClientCommand::Login { profile, token } => {
            profiles::login(&profiles::Store::open(local)?, profile, token).await
        }
        ClientCommand::Connect {
            cloud,
            name,
            runner,
            off,
        } => connect::run(cloud, name, runner, off, local).await,
        ClientCommand::Logout { profile } => {
            profiles::logout(&profiles::Store::open(local)?, profile)
        }
        ClientCommand::Profile { action } => {
            profiles::handle_profile_action(&profiles::Store::open(local)?, action)
        }
        ClientCommand::Hook {
            client,
            event,
            session,
            workspace,
            profile,
            url,
        } => {
            hook::run(client, &event, session, workspace, profile, url, local).await;
            Ok(())
        }
        ClientCommand::Workspace { action } => {
            workspace::handle_workspace_action(action, local).await
        }
        ClientCommand::Card { action } => card::handle_card_action(action, local).await,
        ClientCommand::Session { action } => session::handle_session_action(action, local).await,
        ClientCommand::Agent { prompt, session } => agent::run(prompt, session, local).await,
    }
}

impl ClientCommand {
    /// The level the binary logs this command at to stderr, if it logs:
    /// commands whose output is meant to be read log only warnings.
    #[must_use]
    pub fn log_level(&self) -> Option<&'static str> {
        match self {
            Self::Setup { .. }
            | Self::Workspace { .. }
            | Self::Card { .. }
            | Self::Session { .. }
            | Self::Agent { .. } => Some("warn"),
            Self::Mcp { .. } => Some("info"),
            Self::McpConfig { .. }
            | Self::Login { .. }
            | Self::Connect { .. }
            | Self::Logout { .. }
            | Self::Profile { .. }
            | Self::Hook { .. } => None,
        }
    }
}
