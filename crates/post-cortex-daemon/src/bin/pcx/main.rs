// Copyright (c) 2025 Julius ML
// MIT License

//! pcx - Post-Cortex unified MCP server
//!
//! A single binary supporting both stdio and SSE transports with auto-daemon management.
//!
//! Usage:
//!   pcx                    - Start in stdio mode (auto-starts daemon if needed)
//!   pcx mcp --profile P    - Stdio mode for a backend profile (cloud by default)
//!   pcx login / logout     - Store or forget a profile's API key
//!   pcx profile            - List, switch, add, remove backend profiles
//!   pcx start              - Start daemon in foreground (SSE mode)
//!   pcx start --daemon     - Start daemon in background
//!   pcx status             - Check daemon status
//!   pcx stop               - Stop running daemon
//!   pcx workspace          - Manage workspaces
//!   pcx session            - Manage sessions

use clap::Parser;
use post_cortex_cli::cli::{Cli, ClientCommand, Commands, DaemonCommand, VERSION};
use post_cortex_daemon::daemon::{DaemonConfig, start_rmcp_daemon};
use tracing::info;
use tracing_appender::non_blocking::{NonBlocking, WorkerGuard};
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::{EnvFilter, fmt, prelude::*};

mod admin;
mod daemon_ops;
mod export;
mod import;
#[cfg(feature = "surrealdb-storage")]
mod migrate;
mod remote;
mod vectorize;

/// Logging as a command asks for. What it returns flushes stderr's lines at exit.
fn init_logging(to_file: bool, also_stderr: bool) -> Option<WorkerGuard> {
    // No logger yet: problems with the log directory go to stderr.
    let home = dirs::home_dir().unwrap_or_else(|| {
        eprintln!("pcx: no home directory; logging under the current directory");
        std::path::PathBuf::from(".")
    });
    let log_dir = home.join(".post-cortex/logs");
    if let Err(e) = std::fs::create_dir_all(&log_dir) {
        eprintln!("pcx: log directory {} not created: {e}", log_dir.display());
    }

    if to_file {
        let file_appender = RollingFileAppender::new(Rotation::DAILY, log_dir, "daemon.log");
        let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| {
            EnvFilter::new(post_cortex_daemon::daemon::observability::DEFAULT_LOG_FILTER)
        });

        if also_stderr {
            // Log to both file and stderr (foreground daemon mode)
            let (stderr, logs) = stderr_logs();
            tracing_subscriber::registry()
                .with(fmt::layer().with_writer(file_appender))
                .with(fmt::layer().with_writer(stderr))
                .with(filter)
                .init();
            Some(logs)
        } else {
            // Log to file only (background daemon mode)
            tracing_subscriber::registry()
                .with(fmt::layer().with_writer(file_appender))
                .with(filter)
                .init();
            None
        }
    } else {
        // Log to stderr only (CLI commands)
        Some(init_cli_logging("info"))
    }
}

/// Stderr through its own thread: a reader of stderr that stops costs log lines,
/// never the process, whose calls would otherwise wait on the full pipe.
fn stderr_logs() -> (NonBlocking, WorkerGuard) {
    tracing_appender::non_blocking(std::io::stderr())
}

/// Log to stderr at `default_level` unless RUST_LOG says otherwise. Commands
/// whose output is meant to be read (setup, cards, listings) use "warn", so
/// library info lines do not mix into it.
fn init_cli_logging(default_level: &str) -> WorkerGuard {
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default_level));
    let (stderr, logs) = stderr_logs();
    tracing_subscriber::registry()
        .with(fmt::layer().with_writer(stderr))
        .with(filter)
        .init();
    logs
}

/// The stack of each runtime thread. An embedded SurrealDB (`mem://`) lowers
/// and drops its query trees by recursion on the calling thread and asks for
/// 10 MiB (surrealdb `engine::local`); tokio's default of 2 MiB aborts the
/// process on a deep query.
const RUNTIME_STACK: usize = 10 * 1024 * 1024;

fn main() -> Result<(), String> {
    let cli = Cli::parse();
    // The sandbox's helper (Linux, FreeBSD) confines its program before
    // any runtime thread exists.
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    if let Some(Commands::Daemon(DaemonCommand::SandboxRun { policy, command })) = &cli.command {
        return sandbox_run(policy, command);
    }
    tokio::runtime::Builder::new_multi_thread()
        .thread_stack_size(RUNTIME_STACK)
        .enable_all()
        .build()
        .map_err(|e| format!("the async runtime did not start: {e}"))?
        .block_on(run(cli))
}

/// `pcx sandbox-run`: on Linux, confine this process by `policy` and become
/// `command`; returns only when it could not.
#[cfg(target_os = "linux")]
fn sandbox_run(policy: &str, command: &[String]) -> Result<(), String> {
    match pcx_sandbox::run_helper(policy, command) {
        Ok(never) => match never {},
        Err(e) => Err(format!("sandbox-run: {e}")),
    }
}

/// `pcx sandbox-run`: on FreeBSD, run `command` in a jail of `policy` and
/// exit as it did; returns only when it could not run.
#[cfg(target_os = "freebsd")]
fn sandbox_run(policy: &str, command: &[String]) -> Result<(), String> {
    match pcx_sandbox::run_helper(policy, command) {
        Ok(code) => std::process::exit(code),
        Err(e) => Err(format!("sandbox-run: {e}")),
    }
}

async fn run(cli: Cli) -> Result<(), String> {
    match cli.command {
        // No command = stdio proxy mode (default for MCP clients)
        None => {
            // Minimal logging for stdio mode (to stderr only)
            let _logs = init_logging(false, false);
            let config = DaemonConfig::load().map_err(|e| e.to_string())?;
            post_cortex_cli::run_stdio_proxy(&config.local()).await
        }
        Some(Commands::Daemon(command)) => run_daemon_command(command).await,
        Some(Commands::Client(command)) => run_client_command(command).await,
    }
}

/// A client command, against the local daemon `daemon.toml` names.
async fn run_client_command(command: ClientCommand) -> Result<(), String> {
    let _logs = command.log_level().map(init_cli_logging);
    let config = DaemonConfig::load().map_err(|e| e.to_string())?;
    post_cortex_cli::run(command, &config.local()).await
}

/// One of the daemon's own commands.
async fn run_daemon_command(command: DaemonCommand) -> Result<(), String> {
    match command {
        DaemonCommand::Start { daemon, port, host } => {
            // Flags override daemon.toml / env only when given; clap defaults
            // used to clobber both, so `host` in daemon.toml was ignored.
            let mut config = DaemonConfig::load().map_err(|e| e.to_string())?;
            if let Some(port) = port {
                config.port = port;
            }
            if let Some(host) = host {
                config.host = host;
            }

            // Background daemon mode logs to its file only, foreground to both.
            let _logs = init_logging(true, !daemon);
            if daemon {
                info!("Starting pcx daemon in background mode");
            } else {
                println!("Starting Post-Cortex daemon...");
                println!("Version: {}", VERSION);
                println!();
            }

            start_rmcp_daemon(config).await
        }

        DaemonCommand::Status => {
            let config = DaemonConfig::load().map_err(|e| e.to_string())?;
            daemon_ops::check_status(&config).await
        }

        DaemonCommand::Stop => {
            let config = DaemonConfig::load().map_err(|e| e.to_string())?;
            daemon_ops::stop_daemon(&config).await
        }

        DaemonCommand::Init => daemon_ops::init_config(),

        // On Linux and FreeBSD `main` runs it before the runtime starts; no
        // other platform's sandbox has a helper.
        DaemonCommand::SandboxRun { .. } => Err(
            "sandbox-run is the Linux and FreeBSD sandbox's helper and runs only there".to_string(),
        ),

        DaemonCommand::ModuleLauncher(args) => {
            let post_cortex_cli::cli::ModuleLauncherArgs {
                socket,
                sockets,
                scratch,
                runtimes,
                programs,
                network_programs,
                always_read,
                stores,
                prepared,
                python,
                wheelhouse,
                protoc,
                protoc_gen_es,
                deno,
                imports,
                user,
                host_group,
            } = *args;
            let _logs = init_logging(false, false);
            let launcher = pcx_sandbox::Launcher::platform(user)
                .map_err(|e| format!("module-launcher: {e}"))?;
            let preparer = pcx_module_launcher::Preparer::new(
                pcx_module_launcher::PrepareConfig {
                    store: prepared,
                    python,
                    read: always_read.clone(),
                    wheelhouse,
                    protoc,
                    protoc_gen_es,
                    deno: deno.zip(imports),
                },
                launcher.clone(),
            );
            let served = pcx_module_launcher::Served::new(
                pcx_module_launcher::ServedConfig {
                    runtimes,
                    programs,
                    network_programs,
                    always_read,
                    stores,
                    sockets,
                    scratch,
                    host_group,
                },
                preparer,
                launcher,
            );
            let stop = tokio_util::sync::CancellationToken::new();
            tokio::spawn({
                let stop = stop.clone();
                async move {
                    post_cortex_daemon::daemon::bootstrap::terminated().await;
                    stop.cancel();
                }
            });
            pcx_module_launcher::serve(served, &socket, stop)
                .await
                .map_err(|e| format!("module-launcher {}: {e}", socket.display()))
        }

        DaemonCommand::VectorizeAll { messages } => {
            let _logs = init_logging(false, false);
            vectorize::vectorize_all(messages).await
        }

        DaemonCommand::Export(args) => {
            let _logs = init_logging(false, false);
            export::handle_export(args).await
        }

        DaemonCommand::Import {
            input,
            session,
            workspace,
            skip_existing,
            overwrite,
            cards_newer,
            list,
        } => {
            let _logs = init_logging(false, false);
            import::handle_import(
                input,
                session,
                workspace,
                import::ImportFlags {
                    skip_existing,
                    overwrite,
                    cards_newer,
                },
                list,
            )
            .await
        }

        #[cfg(feature = "surrealdb-storage")]
        DaemonCommand::Migrate(args) => {
            let _logs = init_logging(false, false);
            migrate::handle_migrate(args).await
        }
    }
}
