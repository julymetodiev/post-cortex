// Copyright (c) 2025 Julius ML
// MIT License

//! pcx - Post-Cortex unified MCP server
//!
//! A single binary supporting both stdio and SSE transports with auto-daemon management.
//!
//! Usage:
//!   pcx                    - Start in stdio mode (auto-starts daemon if needed)
//!   pcx start              - Start daemon in foreground (SSE mode)
//!   pcx start --daemon     - Start daemon in background
//!   pcx status             - Check daemon status
//!   pcx stop               - Stop running daemon
//!   pcx workspace          - Manage workspaces
//!   pcx session            - Manage sessions

use clap::Parser;
use post_cortex_daemon::daemon::{DaemonConfig, run_stdio_proxy, start_rmcp_daemon};
use tracing::info;
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::{EnvFilter, fmt, prelude::*};

// Submodules live under src/bin/pcx/. For binaries in src/bin/, Cargo's default
// module resolution looks for siblings (src/bin/<name>.rs), so each mod
// declaration carries an explicit #[path].
#[path = "pcx/admin.rs"]
mod admin;
#[path = "pcx/cli.rs"]
mod cli;
#[path = "pcx/daemon_client.rs"]
mod daemon_client;
#[path = "pcx/daemon_ops.rs"]
mod daemon_ops;
#[path = "pcx/export.rs"]
mod export;
#[path = "pcx/import.rs"]
mod import;
#[cfg(feature = "surrealdb-storage")]
#[path = "pcx/migrate.rs"]
mod migrate;
#[path = "pcx/session.rs"]
mod session;
#[path = "pcx/setup.rs"]
mod setup;
#[path = "pcx/vectorize.rs"]
mod vectorize;
#[path = "pcx/workspace.rs"]
mod workspace;

use cli::{Cli, Commands, VERSION};

fn init_logging(to_file: bool, also_stderr: bool) {
    let log_dir = dirs::home_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join(".post-cortex/logs");
    std::fs::create_dir_all(&log_dir).ok();

    if to_file {
        let file_appender = RollingFileAppender::new(Rotation::DAILY, log_dir, "daemon.log");
        // Use RUST_LOG if set, otherwise default to "info"
        let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

        if also_stderr {
            // Log to both file and stderr (foreground daemon mode)
            tracing_subscriber::registry()
                .with(fmt::layer().with_writer(file_appender))
                .with(fmt::layer().with_writer(std::io::stderr))
                .with(filter)
                .init();
        } else {
            // Log to file only (background daemon mode)
            tracing_subscriber::registry()
                .with(fmt::layer().with_writer(file_appender))
                .with(filter)
                .init();
        }
    } else {
        // Log to stderr only (CLI commands)
        let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
        tracing_subscriber::registry()
            .with(fmt::layer().with_writer(std::io::stderr))
            .with(filter)
            .init();
    }
}

#[tokio::main]
async fn main() -> Result<(), String> {
    let cli = Cli::parse();

    match cli.command {
        // No command = stdio proxy mode (default for MCP clients)
        None => {
            // Minimal logging for stdio mode (to stderr only)
            init_logging(false, false);

            let config = DaemonConfig::load();
            run_stdio_proxy(config).await
        }

        Some(Commands::Start { daemon, port, host }) => {
            let mut config = DaemonConfig::load();
            config.port = port;
            config.host = host;

            if daemon {
                // Background daemon mode - log to file only
                init_logging(true, false);
                info!("Starting pcx daemon in background mode");
            } else {
                // Foreground mode - log to file AND stderr
                init_logging(true, true);
                println!("Starting Post-Cortex daemon...");
                println!("Version: {}", VERSION);
                println!();
            }

            start_rmcp_daemon(config).await
        }

        Some(Commands::Status) => {
            let config = DaemonConfig::load();
            daemon_ops::check_status(&config).await
        }

        Some(Commands::Stop) => {
            let config = DaemonConfig::load();
            daemon_ops::stop_daemon(&config)
        }

        Some(Commands::Init) => daemon_ops::init_config(),

        Some(Commands::Setup {
            name,
            non_interactive,
        }) => {
            init_logging(false, false);
            setup::handle_setup(name, non_interactive).await
        }

        Some(Commands::VectorizeAll) => {
            init_logging(false, false);
            vectorize::vectorize_all().await
        }

        Some(Commands::Workspace { action }) => {
            init_logging(false, false);
            workspace::handle_workspace_action(action).await
        }

        Some(Commands::Session { action }) => {
            init_logging(false, false);
            session::handle_session_action(action).await
        }

        Some(Commands::Export {
            output,
            compress,
            session,
            workspace,
            checkpoints,
            pretty,
            force,
        }) => {
            init_logging(false, false);
            export::handle_export(
                output,
                compress,
                session,
                workspace,
                checkpoints,
                pretty,
                force,
            )
            .await
        }

        Some(Commands::Import {
            input,
            session,
            workspace,
            skip_existing,
            overwrite,
            list,
        }) => {
            init_logging(false, false);
            import::handle_import(input, session, workspace, skip_existing, overwrite, list).await
        }

        #[cfg(feature = "surrealdb-storage")]
        Some(Commands::Migrate {
            from,
            to,
            source_path,
            target_path,
            remote_endpoint,
            username,
            password,
            dry_run,
        }) => {
            init_logging(false, false);
            migrate::handle_migrate(
                from,
                to,
                source_path,
                target_path,
                remote_endpoint,
                username,
                password,
                dry_run,
            )
            .await
        }
    }
}
