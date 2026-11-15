// Copyright (c) 2025 Julius ML
// MIT License

//! Daemon lifecycle commands: status, stop, init-config.

use std::path::PathBuf;

use post_cortex_daemon::daemon::{DaemonConfig, pidfile};
use tokio::net::TcpStream;
use tokio::time::{Duration, Instant, sleep, timeout};

/// Whether something listens on the daemon's configured address.
pub async fn listening(config: &DaemonConfig) -> bool {
    let addr = format!("{}:{}", config.host, config.port);
    matches!(
        timeout(Duration::from_secs(2), TcpStream::connect(&addr)).await,
        Ok(Ok(_))
    )
}

pub async fn check_status(config: &DaemonConfig) -> Result<(), String> {
    println!(
        "Checking daemon status at {}:{}...",
        config.host, config.port
    );
    if !listening(config).await {
        println!("Daemon is not running");
        return Err("Daemon not running".to_string());
    }
    println!("Daemon is running");
    println!("  MCP endpoint: http://{}:{}/mcp", config.host, config.port);
    println!(
        "  Health endpoint: http://{}:{}/health",
        config.host, config.port
    );
    Ok(())
}

/// How long a daemon may take to stop: its requests drain, then every
/// session is written.
const STOP_WAIT: Duration = Duration::from_secs(60);

/// Stops the daemon of `config`: asks its process to end (SIGTERM) and waits
/// until it has, so its sessions are written and its database is free.
pub async fn stop_daemon(config: &DaemonConfig) -> Result<(), String> {
    if !listening(config).await {
        println!("Daemon is not running");
        return Ok(());
    }
    let pid = pidfile::recorded(&config.data_directory)
        .map_err(|e| format!("Cannot read the daemon's pid file: {e}"))?
        .ok_or_else(|| {
            format!(
                "The daemon at {}:{} left no pid file in {}: it runs with another data \
                 directory, or is a version before this one. Stop it with Ctrl+C in its \
                 terminal.",
                config.host,
                config.port,
                config.data_directory.display()
            )
        })?;
    process::terminate(pid)?;
    println!("Stopping daemon (pid {pid})...");
    let started = Instant::now();
    while process::runs(pid) {
        if started.elapsed() >= STOP_WAIT {
            return Err(format!(
                "The daemon (pid {pid}) still runs after {} seconds",
                STOP_WAIT.as_secs()
            ));
        }
        sleep(Duration::from_millis(200)).await;
    }
    println!("Daemon stopped");
    Ok(())
}

/// A process by its id, as `pcx stop` needs it.
#[cfg(unix)]
mod process {
    use nix::sys::signal::{Signal, kill};
    use nix::unistd::Pid;

    fn of(pid: u32) -> Result<Pid, String> {
        i32::try_from(pid)
            .map(Pid::from_raw)
            .map_err(|_| format!("{pid} is no process id"))
    }

    /// Asks the process to end.
    pub fn terminate(pid: u32) -> Result<(), String> {
        kill(of(pid)?, Signal::SIGTERM)
            .map_err(|e| format!("Cannot signal the daemon (pid {pid}): {e}"))
    }

    /// Whether the process still runs.
    pub fn runs(pid: u32) -> bool {
        of(pid).is_ok_and(|pid| kill(pid, None).is_ok())
    }
}

/// A system without signals has no way to ask a process to end.
#[cfg(not(unix))]
mod process {
    pub fn terminate(pid: u32) -> Result<(), String> {
        Err(format!(
            "pcx stop cannot signal a process on this system; stop the daemon (pid {pid}) \
             with Ctrl+C in its terminal"
        ))
    }

    pub fn runs(_pid: u32) -> bool {
        false
    }
}

pub fn init_config() -> Result<(), String> {
    println!("Creating example config file...");

    match write_example_config() {
        Ok(path) => {
            println!("Config file created at: {:?}", path);
            println!();
            println!("To start the daemon:");
            println!("  pcx start");
            Ok(())
        }
        Err(e) => {
            eprintln!("Failed to create config file: {}", e);
            Err(e)
        }
    }
}

/// Write an example `daemon.toml` (the defaults, with what the
/// environment overrides) at the default location; returns its path.
fn write_example_config() -> Result<PathBuf, String> {
    let config_path = DaemonConfig::path();

    if config_path.exists() {
        return Err(format!("Config file already exists at {:?}", config_path));
    }

    // Ensure parent directory exists
    if let Some(parent) = config_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create config directory: {}", e))?;
    }

    let example_config = DaemonConfig::default();
    let toml_content = toml::to_string_pretty(&example_config)
        .map_err(|e| format!("Failed to serialize config: {}", e))?;

    // Add comments to explain each field
    let commented_toml = format!(
        "# Post-Cortex Daemon Configuration\n\
         # \n\
         # This file configures the HTTP daemon server for multi-client access.\n\
         # Environment variables override these settings:\n\
         #   PC_HOST - Override host\n\
         #   PC_PORT - Override port\n\
         #   PC_DATA_DIR - Override data directory\n\
         #   PC_STORAGE_BACKEND - Storage backend: \"rocksdb\" or \"surrealdb\"\n\
         #   PC_SURREALDB_ENDPOINT - SurrealDB WebSocket endpoint (e.g., \"ws://localhost:8000\")\n\
         #   PC_SURREALDB_USER - SurrealDB username\n\
         #   PC_SURREALDB_PASS - SurrealDB password\n\
         #   PC_AUTH_TOKEN - Bearer token required by HTTP/gRPC clients\n\
         #   PC_REDACT - \"0\" disables redaction of secrets/personal data\n\
         #   PC_ALLOWED_HOSTS - Comma-separated extra allowed Host headers\n\
         # \n\
         # allowed_hosts: Host header values accepted on /mcp and /api/*\n\
         # (DNS rebinding protection). localhost, 127.0.0.1, ::1 and a\n\
         # non-wildcard `host` are always allowed; other requests get 403.\n\
         # When binding to 0.0.0.0, list the names remote clients use:\n\
         #   allowed_hosts = [\"my-mac.tailnet.ts.net\", \"100.64.0.7:3737\"]\n\
         # Entries are host (any port) or host:port; [\"*\"] disables the check.\n\
         # \n\
         # Priority: Environment > Config file > Defaults\n\n\
         {}",
        toml_content
    );

    std::fs::write(&config_path, commented_toml)
        .map_err(|e| format!("Failed to write config file: {}", e))?;

    Ok(config_path)
}
