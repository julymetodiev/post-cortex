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

const VERSION: &str = env!("CARGO_PKG_VERSION");

use clap::{Parser, Subcommand};
use post_cortex::daemon::{DaemonConfig, is_daemon_running, run_stdio_proxy, start_rmcp_daemon};
#[cfg(feature = "surrealdb-storage")]
use post_cortex::storage::{GraphStorage, Storage, StorageBackendType, SurrealDBStorage};
use post_cortex::storage::{
    CompressionType, ExportOptions, ImportOptions, RealRocksDBStorage, list_export_sessions,
    preview_export_file, read_export_file, write_export_file,
};
use post_cortex::workspace::SessionRole;
use post_cortex::{ConversationMemorySystem, SystemConfig};
use serde::{Deserialize, Serialize};
use std::env;
use tracing::{error, info};
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::{EnvFilter, fmt, prelude::*};
use uuid::Uuid;

#[derive(Parser)]
#[command(name = "pcx")]
#[command(version = VERSION)]
#[command(about = "Post-Cortex - Intelligent conversation memory system")]
#[command(
    long_about = "Post-Cortex unified MCP server supporting stdio and SSE transports.\n\n\
    When run without arguments, starts in stdio mode (for MCP clients).\n\
    The daemon is auto-started in background if not already running."
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Start the MCP server (daemon mode)
    Start {
        /// Run as background daemon (used internally for auto-start)
        #[arg(long)]
        daemon: bool,

        /// Port to listen on (default: 3737)
        #[arg(long, short, default_value = "3737")]
        port: u16,

        /// Host to bind to (default: 127.0.0.1)
        #[arg(long, default_value = "127.0.0.1")]
        host: String,
    },

    /// Check if daemon is running
    Status,

    /// Stop running daemon
    Stop,

    /// Initialize configuration file
    Init,

    /// Vectorize all sessions
    VectorizeAll,

    /// Manage workspaces
    Workspace {
        #[command(subcommand)]
        action: WorkspaceAction,
    },

    /// Manage sessions
    Session {
        #[command(subcommand)]
        action: SessionAction,
    },

    /// Export data to JSON file
    #[command(long_about = "Export sessions and workspaces to a JSON file.\n\n\
        Supports optional compression (gzip, zstd) for smaller file sizes.\n\
        The compression type is auto-detected from the file extension.\n\n\
        EXAMPLES:\n\n\
        Full export (all sessions and workspaces):\n\
        $ pcx export --output backup.json\n\n\
        Export with gzip compression:\n\
        $ pcx export --output backup.json.gz\n\n\
        Export with zstd compression (fastest):\n\
        $ pcx export --output backup.json.zst\n\n\
        Export specific session:\n\
        $ pcx export --output session.json --session <uuid>\n\n\
        Export workspace with all its sessions:\n\
        $ pcx export --output workspace.json --workspace <uuid>\n\n\
        Pretty-printed JSON for debugging:\n\
        $ pcx export --output backup.json --pretty\n\n\
        Force overwrite without asking:\n\
        $ pcx export --output backup.json.gz --force")]
    Export {
        /// Output file path. Extension determines compression:
        /// .json (none), .json.gz (gzip), .json.zst (zstd)
        /// If not specified, generates: export-YYYY-MM-DD_HH-MM-SS.json.gz
        #[arg(short, long, value_name = "FILE")]
        output: Option<String>,

        /// Compression type: none, gzip, zstd.
        /// Auto-detected from file extension if not specified.
        #[arg(short, long, value_name = "TYPE")]
        compress: Option<String>,

        /// Export only specific session(s). Can be repeated.
        #[arg(long, value_name = "UUID")]
        session: Option<Vec<String>>,

        /// Export only specific workspace and all its sessions
        #[arg(long, value_name = "UUID")]
        workspace: Option<String>,

        /// Include checkpoints in export
        #[arg(long)]
        checkpoints: bool,

        /// Pretty print JSON (human-readable, larger file size)
        #[arg(long)]
        pretty: bool,

        /// Overwrite existing file without asking
        #[arg(long, short = 'f')]
        force: bool,
    },

    /// Import data from JSON file
    #[command(long_about = "Import sessions and workspaces from an export file.\n\n\
        Supports automatic decompression based on file extension.\n\
        Use --list to preview contents before importing.\n\n\
        EXAMPLES:\n\n\
        List contents without importing:\n\
        $ pcx import --input backup.json --list\n\n\
        Import everything:\n\
        $ pcx import --input backup.json\n\n\
        Import from compressed file:\n\
        $ pcx import --input backup.json.gz\n\n\
        Import specific session only:\n\
        $ pcx import --input backup.json --session <uuid>\n\n\
        Import and skip existing (no errors):\n\
        $ pcx import --input backup.json --skip-existing\n\n\
        Import and overwrite existing:\n\
        $ pcx import --input backup.json --overwrite")]
    Import {
        /// Input file path (.json, .json.gz, or .json.zst)
        #[arg(short, long, value_name = "FILE")]
        input: String,

        /// Import only specific session(s). Can be repeated.
        #[arg(long, value_name = "UUID")]
        session: Option<Vec<String>>,

        /// Import only specific workspace from the file
        #[arg(long, value_name = "UUID")]
        workspace: Option<String>,

        /// Skip existing sessions/workspaces (no error)
        #[arg(long)]
        skip_existing: bool,

        /// Overwrite existing sessions/workspaces
        #[arg(long)]
        overwrite: bool,

        /// List contents of export file without importing
        #[arg(long)]
        list: bool,
    },

    /// Migrate data between storage backends
    #[cfg(feature = "surrealdb-storage")]
    #[command(long_about = "Migrate data from one storage backend to another.\n\n\
        Currently supports migration from RocksDB to SurrealDB (local or remote).\n\n\
        EXAMPLES:\n\n\
        Migrate to local SurrealDB:\n\
        $ pcx migrate --from rocksdb --to surrealdb\n\n\
        Migrate to remote SurrealDB (Docker):\n\
        $ pcx migrate --from rocksdb --to surrealdb \\\n\
            --remote-endpoint localhost:8000 \\\n\
            --username root --password root\n\n\
        Dry run (show what would be migrated):\n\
        $ pcx migrate --from rocksdb --to surrealdb --dry-run")]
    Migrate {
        /// Source storage backend (rocksdb)
        #[arg(long, value_name = "BACKEND")]
        from: String,

        /// Target storage backend (surrealdb)
        #[arg(long, value_name = "BACKEND")]
        to: String,

        /// Source data path (default: ~/.post-cortex/data)
        #[arg(long, value_name = "PATH")]
        source_path: Option<String>,

        /// Target data path for local SurrealDB (default: ~/.post-cortex/surrealdb)
        #[arg(long, value_name = "PATH")]
        target_path: Option<String>,

        /// Remote SurrealDB endpoint (e.g., localhost:8000 or ws://host:port)
        #[arg(long, value_name = "ENDPOINT")]
        remote_endpoint: Option<String>,

        /// Username for remote SurrealDB authentication
        #[arg(long, value_name = "USER")]
        username: Option<String>,

        /// Password for remote SurrealDB authentication
        #[arg(long, value_name = "PASS")]
        password: Option<String>,

        /// Dry run - show what would be migrated without actually migrating
        #[arg(long)]
        dry_run: bool,
    },
}

#[derive(Subcommand)]
enum WorkspaceAction {
    /// Create a new workspace
    #[command(
        long_about = "Create a new workspace for organizing related sessions.\n\n\
        EXAMPLES:\n\n\
        $ pcx workspace create my-project \"Main project workspace\""
    )]
    Create {
        /// Workspace name
        #[arg(value_name = "NAME")]
        name: String,
        /// Workspace description
        #[arg(default_value = "", value_name = "DESC")]
        description: String,
    },

    /// Delete a workspace
    #[command(long_about = "Delete a workspace by ID.\n\n\
        Note: This does NOT delete the sessions in the workspace.\n\n\
        EXAMPLES:\n\n\
        $ pcx workspace delete <uuid>")]
    Delete {
        /// Workspace ID (UUID)
        #[arg(value_name = "UUID")]
        id: String,
    },

    /// List all workspaces
    #[command(long_about = "List all workspaces with their session counts.")]
    List,

    /// Attach a session to a workspace
    #[command(
        long_about = "Attach a session to a workspace with a specific role.\n\n\
        ROLES:\n\
        - primary:    Main session for this workspace\n\
        - related:    Related/peer session (default)\n\
        - dependency: External dependency documentation\n\
        - shared:     Shared across multiple workspaces\n\n\
        EXAMPLES:\n\n\
        $ pcx workspace attach <workspace-uuid> <session-uuid>\n\
        $ pcx workspace attach <workspace-uuid> <session-uuid> primary"
    )]
    Attach {
        /// Workspace ID (UUID)
        #[arg(value_name = "WORKSPACE_UUID")]
        workspace_id: String,
        /// Session ID (UUID)
        #[arg(value_name = "SESSION_UUID")]
        session_id: String,
        /// Role: primary, related, dependency, shared
        #[arg(default_value = "related", value_name = "ROLE")]
        role: String,
    },
}

#[derive(Subcommand)]
enum SessionAction {
    /// Create a new session
    #[command(
        long_about = "Create a new session for storing conversation context.\n\n\
        EXAMPLES:\n\n\
        $ pcx session create\n\
        $ pcx session create my-session\n\
        $ pcx session create my-session \"Session description\""
    )]
    Create {
        /// Session name (optional)
        #[arg(value_name = "NAME")]
        name: Option<String>,
        /// Session description (optional)
        #[arg(value_name = "DESC")]
        description: Option<String>,
    },

    /// Delete a session
    #[command(long_about = "Delete a session and all its data.\n\n\
        WARNING: This permanently deletes all context updates in the session.\n\n\
        EXAMPLES:\n\n\
        $ pcx session delete <uuid>")]
    Delete {
        /// Session ID (UUID)
        #[arg(value_name = "UUID")]
        id: String,
    },

    /// List all sessions
    #[command(long_about = "List all sessions with their workspace associations.")]
    List,
}

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
            check_status(&config).await
        }

        Some(Commands::Stop) => {
            let config = DaemonConfig::load();
            stop_daemon(&config)
        }

        Some(Commands::Init) => init_config(),

        Some(Commands::VectorizeAll) => {
            init_logging(false, false);
            vectorize_all().await
        }

        Some(Commands::Workspace { action }) => {
            init_logging(false, false);
            handle_workspace_action(action).await
        }

        Some(Commands::Session { action }) => {
            init_logging(false, false);
            handle_session_action(action).await
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
            handle_export(
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
            handle_import(input, session, workspace, skip_existing, overwrite, list).await
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
            handle_migrate(
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

async fn check_status(config: &DaemonConfig) -> Result<(), String> {
    use tokio::net::TcpStream;
    use tokio::time::{Duration, timeout};

    println!(
        "Checking daemon status at {}:{}...",
        config.host, config.port
    );

    let addr = format!("{}:{}", config.host, config.port);

    match timeout(Duration::from_secs(2), TcpStream::connect(&addr)).await {
        Ok(Ok(_)) => {
            println!("Daemon is running");
            println!("  MCP endpoint: http://{}:{}/mcp", config.host, config.port);
            println!(
                "  Health endpoint: http://{}:{}/health",
                config.host, config.port
            );
            Ok(())
        }
        Ok(Err(e)) => {
            println!("Daemon is not running");
            println!("  Error: {}", e);
            Err("Daemon not running".to_string())
        }
        Err(_) => {
            println!("Daemon is not running");
            println!("  Error: Connection timeout");
            Err("Daemon not running".to_string())
        }
    }
}

fn stop_daemon(config: &DaemonConfig) -> Result<(), String> {
    println!("Stopping daemon at {}:{}...", config.host, config.port);
    println!();
    println!("To stop the daemon:");
    println!("1. Press Ctrl+C in the terminal where daemon is running");
    println!("2. Or use: kill $(lsof -t -i:{})", config.port);
    println!("3. Or use: pkill -f 'pcx start'");
    Ok(())
}

fn init_config() -> Result<(), String> {
    println!("Creating example config file...");

    match DaemonConfig::create_example_config() {
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

async fn init_admin_system() -> Result<ConversationMemorySystem, String> {
    let daemon_config = DaemonConfig::load();
    #[allow(unused_mut)]
    let mut config = SystemConfig {
        enable_embeddings: false,
        data_directory: daemon_config.data_directory,
        ..SystemConfig::default()
    };

    // Configure storage backend from daemon config
    #[cfg(feature = "surrealdb-storage")]
    {
        config.storage_backend = match daemon_config.storage_backend.as_str() {
            "surrealdb" => StorageBackendType::SurrealDB,
            _ => StorageBackendType::RocksDB,
        };
        config.surrealdb_endpoint = daemon_config.surrealdb_endpoint;
        config.surrealdb_username = daemon_config.surrealdb_username;
        config.surrealdb_password = daemon_config.surrealdb_password;
        config.surrealdb_namespace = Some(daemon_config.surrealdb_namespace);
        config.surrealdb_database = Some(daemon_config.surrealdb_database);
    }

    ConversationMemorySystem::new(config).await
}

// HTTP API types for CLI
#[derive(Deserialize)]
struct SessionInfo {
    id: String,
    name: String,
    workspace: Option<String>,
}

#[derive(Deserialize)]
struct WorkspaceInfo {
    id: String,
    name: String,
    description: String,
    session_count: usize,
}

#[derive(Serialize)]
struct CreateSessionRequest {
    name: Option<String>,
    description: Option<String>,
}

#[derive(Serialize)]
struct CreateWorkspaceRequest {
    name: String,
    description: Option<String>,
}

#[derive(Serialize)]
struct AttachSessionRequest {
    role: String,
}

/// HTTP client for daemon API
struct DaemonClient {
    base_url: String,
    client: reqwest::Client,
}

impl DaemonClient {
    fn new(config: &DaemonConfig) -> Self {
        Self {
            base_url: format!("http://{}:{}", config.host, config.port),
            client: reqwest::Client::new(),
        }
    }

    async fn list_sessions(&self) -> Result<Vec<SessionInfo>, String> {
        let resp = self
            .client
            .get(format!("{}/api/sessions", self.base_url))
            .send()
            .await
            .map_err(|e| format!("HTTP error: {}", e))?;

        if !resp.status().is_success() {
            return Err(format!("API error: {}", resp.status()));
        }

        resp.json().await.map_err(|e| format!("JSON error: {}", e))
    }

    async fn create_session(
        &self,
        name: Option<String>,
        description: Option<String>,
    ) -> Result<SessionInfo, String> {
        let resp = self
            .client
            .post(format!("{}/api/sessions", self.base_url))
            .json(&CreateSessionRequest { name, description })
            .send()
            .await
            .map_err(|e| format!("HTTP error: {}", e))?;

        if !resp.status().is_success() {
            return Err(format!("API error: {}", resp.status()));
        }

        resp.json().await.map_err(|e| format!("JSON error: {}", e))
    }

    async fn delete_session(&self, id: &str) -> Result<(), String> {
        let resp = self
            .client
            .delete(format!("{}/api/sessions/{}", self.base_url, id))
            .send()
            .await
            .map_err(|e| format!("HTTP error: {}", e))?;

        if !resp.status().is_success() {
            return Err(format!("API error: {}", resp.status()));
        }

        Ok(())
    }

    async fn list_workspaces(&self) -> Result<Vec<WorkspaceInfo>, String> {
        let resp = self
            .client
            .get(format!("{}/api/workspaces", self.base_url))
            .send()
            .await
            .map_err(|e| format!("HTTP error: {}", e))?;

        if !resp.status().is_success() {
            return Err(format!("API error: {}", resp.status()));
        }

        resp.json().await.map_err(|e| format!("JSON error: {}", e))
    }

    async fn create_workspace(
        &self,
        name: String,
        description: String,
    ) -> Result<WorkspaceInfo, String> {
        let resp = self
            .client
            .post(format!("{}/api/workspaces", self.base_url))
            .json(&CreateWorkspaceRequest {
                name,
                description: Some(description),
            })
            .send()
            .await
            .map_err(|e| format!("HTTP error: {}", e))?;

        if !resp.status().is_success() {
            return Err(format!("API error: {}", resp.status()));
        }

        resp.json().await.map_err(|e| format!("JSON error: {}", e))
    }

    async fn delete_workspace(&self, id: &str) -> Result<(), String> {
        let resp = self
            .client
            .delete(format!("{}/api/workspaces/{}", self.base_url, id))
            .send()
            .await
            .map_err(|e| format!("HTTP error: {}", e))?;

        if !resp.status().is_success() {
            return Err(format!("API error: {}", resp.status()));
        }

        Ok(())
    }

    async fn attach_session(
        &self,
        workspace_id: &str,
        session_id: &str,
        role: &str,
    ) -> Result<(), String> {
        let resp = self
            .client
            .post(format!(
                "{}/api/workspaces/{}/sessions/{}",
                self.base_url, workspace_id, session_id
            ))
            .json(&AttachSessionRequest {
                role: role.to_string(),
            })
            .send()
            .await
            .map_err(|e| format!("HTTP error: {}", e))?;

        if !resp.status().is_success() {
            return Err(format!("API error: {}", resp.status()));
        }

        Ok(())
    }
}

async fn handle_workspace_action(action: WorkspaceAction) -> Result<(), String> {
    let config = DaemonConfig::load();
    let use_daemon = is_daemon_running(&config.host, config.port).await;

    match action {
        WorkspaceAction::Create { name, description } => {
            if use_daemon {
                let client = DaemonClient::new(&config);
                let ws = client.create_workspace(name, description).await?;
                println!("Workspace created:");
                println!("  ID:          {}", ws.id);
                println!("  Name:        {}", ws.name);
                println!("  Description: {}", ws.description);
            } else {
                let system = init_admin_system().await?;
                let id = Uuid::new_v4();
                system
                    .get_storage()
                    .save_workspace_metadata(id, &name, &description, &[])
                    .await?;
                println!("Workspace created:");
                println!("  ID:          {}", id);
                println!("  Name:        {}", name);
                println!("  Description: {}", description);
            }
            Ok(())
        }

        WorkspaceAction::Delete { id } => {
            if use_daemon {
                let client = DaemonClient::new(&config);
                client.delete_workspace(&id).await?;
            } else {
                let uuid = Uuid::parse_str(&id).map_err(|e| format!("Invalid UUID: {}", e))?;
                let system = init_admin_system().await?;
                system.get_storage().delete_workspace(uuid).await?;
            }
            println!("Workspace {} deleted", id);
            Ok(())
        }

        WorkspaceAction::List => {
            if use_daemon {
                let client = DaemonClient::new(&config);
                let workspaces = client.list_workspaces().await?;
                println!("Workspaces ({})", workspaces.len());
                println!("{:<38} {:<20} {}", "ID", "Name", "Sessions");
                println!("{:-<38} {:-<20} {:-<10}", "", "", "");
                for ws in workspaces {
                    println!("{:<38} {:<20} {}", ws.id, ws.name, ws.session_count);
                }
            } else {
                let system = init_admin_system().await?;
                let workspaces = system.get_storage().list_all_workspaces().await?;
                let existing_sessions: std::collections::HashSet<_> =
                    system.list_sessions().await?.into_iter().collect();
                println!("Workspaces ({})", workspaces.len());
                println!("{:<38} {:<20} {}", "ID", "Name", "Sessions");
                println!("{:-<38} {:-<20} {:-<10}", "", "", "");
                for ws in workspaces {
                    let actual_count = ws
                        .sessions
                        .iter()
                        .filter(|(id, _)| existing_sessions.contains(id))
                        .count();
                    println!("{:<38} {:<20} {}", ws.id, ws.name, actual_count);
                }
            }
            Ok(())
        }

        WorkspaceAction::Attach {
            workspace_id,
            session_id,
            role,
        } => {
            if use_daemon {
                let client = DaemonClient::new(&config);
                client
                    .attach_session(&workspace_id, &session_id, &role)
                    .await?;
            } else {
                let ws_id = Uuid::parse_str(&workspace_id)
                    .map_err(|e| format!("Invalid workspace UUID: {}", e))?;
                let sess_id = Uuid::parse_str(&session_id)
                    .map_err(|e| format!("Invalid session UUID: {}", e))?;
                let role_enum = match role.to_lowercase().as_str() {
                    "primary" => SessionRole::Primary,
                    "related" => SessionRole::Related,
                    "dependency" => SessionRole::Dependency,
                    "shared" => SessionRole::Shared,
                    _ => {
                        return Err(format!(
                            "Invalid role: {}. Use: primary, related, dependency, shared",
                            role
                        ));
                    }
                };
                let system = init_admin_system().await?;
                system
                    .get_storage()
                    .add_session_to_workspace(ws_id, sess_id, role_enum)
                    .await?;
            }
            println!(
                "Session {} attached to workspace {} as {}",
                session_id, workspace_id, role
            );
            Ok(())
        }
    }
}

async fn handle_session_action(action: SessionAction) -> Result<(), String> {
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
                system.get_storage().delete_session(uuid).await?;
            }
            println!("Session {} deleted", id);
            Ok(())
        }

        SessionAction::List => {
            if use_daemon {
                let client = DaemonClient::new(&config);
                let sessions = client.list_sessions().await?;
                println!("Sessions ({})", sessions.len());
                println!("{:<38} {:<20} {}", "ID", "Workspace", "Name");
                println!("{:-<38} {:-<20} {:-<30}", "", "", "");
                for s in sessions {
                    let ws = s.workspace.as_deref().unwrap_or("-");
                    println!("{:<38} {:<20} {}", s.id, ws, s.name);
                }
            } else {
                let system = init_admin_system().await?;
                let ids = system.list_sessions().await?;

                let workspaces = system.workspace_manager.list_workspaces();
                let mut session_workspace_map = std::collections::HashMap::new();

                for ws in workspaces {
                    for (session_id, _role) in ws.get_all_sessions() {
                        session_workspace_map.insert(session_id, ws.name.clone());
                    }
                }

                println!("Sessions ({})", ids.len());
                println!("{:<38} {:<20} {}", "ID", "Workspace", "Name");
                println!("{:-<38} {:-<20} {:-<30}", "", "", "");

                for id in ids {
                    let workspace_name = session_workspace_map
                        .get(&id)
                        .map(|s| s.as_str())
                        .unwrap_or("-");
                    let session_name = match system.get_session(id).await {
                        Ok(session_arc) => {
                            let session = session_arc.load();
                            session.name().unwrap_or("Unnamed".to_string())
                        }
                        Err(_) => "Error loading".to_string(),
                    };
                    println!("{:<38} {:<20} {}", id, workspace_name, session_name);
                }
            }
            Ok(())
        }
    }
}

#[cfg(feature = "embeddings")]
async fn vectorize_all() -> Result<(), String> {
    println!("Starting vectorization of all sessions...");

    let daemon_config = DaemonConfig::load();
    #[allow(unused_mut)]
    let mut config = SystemConfig {
        enable_embeddings: true,
        auto_vectorize_on_update: false,
        data_directory: daemon_config.data_directory.clone(),
        ..SystemConfig::default()
    };

    // Configure storage backend from daemon config
    #[cfg(feature = "surrealdb-storage")]
    {
        config.storage_backend = match daemon_config.storage_backend.as_str() {
            "surrealdb" => StorageBackendType::SurrealDB,
            _ => StorageBackendType::RocksDB,
        };
        config.surrealdb_endpoint = daemon_config.surrealdb_endpoint.clone();
        config.surrealdb_username = daemon_config.surrealdb_username.clone();
        config.surrealdb_password = daemon_config.surrealdb_password.clone();
        config.surrealdb_namespace = Some(daemon_config.surrealdb_namespace.clone());
        config.surrealdb_database = Some(daemon_config.surrealdb_database.clone());

        if config.storage_backend == StorageBackendType::SurrealDB {
            println!(
                "Target: SurrealDB at {}",
                config.surrealdb_endpoint.as_deref().unwrap_or("default")
            );
        } else {
            println!("Target: RocksDB at {}", config.data_directory);
        }
    }

    #[cfg(not(feature = "surrealdb-storage"))]
    println!("Target: RocksDB at {}", config.data_directory);

    let system = ConversationMemorySystem::new(config)
        .await
        .map_err(|e| format!("Failed to initialize: {}", e))?;

    match system.vectorize_all_sessions().await {
        Ok((total, successful, failed)) => {
            println!("Vectorization complete!");
            println!("  Total items:  {}", total);
            println!("  Successful:   {}", successful);
            println!("  Failed:       {}", failed);
            Ok(())
        }
        Err(e) => {
            error!("Vectorization failed: {}", e);
            Err(e)
        }
    }
}

#[cfg(not(feature = "embeddings"))]
async fn vectorize_all() -> Result<(), String> {
    eprintln!("Vectorization requires the 'embeddings' feature");
    eprintln!("Rebuild with: cargo build --release --features embeddings");
    Err("Embeddings feature not enabled".to_string())
}

// ============================================================================
// Export/Import Handlers
// ============================================================================

use post_cortex::daemon::DaemonConfig as DaemonConfigAlias;

async fn export_from_rocksdb(
    daemon_config: &DaemonConfigAlias,
    options: &ExportOptions,
    workspace: Option<String>,
    session: Option<Vec<String>>,
) -> Result<post_cortex::storage::export_import::ExportData, String> {
    let data_dir = &daemon_config.data_directory;
    println!("Source: RocksDB at {}", data_dir);

    let storage = RealRocksDBStorage::new(data_dir).await.map_err(|e| {
        let err_str = e.to_string();
        if err_str.contains("LOCK") || err_str.contains("Resource temporarily unavailable") {
            format!(
                "Database is locked by another process (likely the daemon).\n\
                     \n\
                     Please stop the daemon first:\n\
                     \n\
                     Option 1: Stop via CLI\n\
                     $ pkill -f 'pcx start'\n\
                     \n\
                     Option 2: Stop via launchctl (macOS)\n\
                     $ launchctl unload ~/Library/LaunchAgents/com.juliusml.post-cortex.plist\n\
                     \n\
                     Then retry the export command."
            )
        } else {
            format!("Failed to open storage: {}", e)
        }
    })?;

    if let Some(workspace_id) = workspace {
        let uuid =
            Uuid::parse_str(&workspace_id).map_err(|e| format!("Invalid workspace UUID: {}", e))?;
        println!("Exporting workspace {}...", workspace_id);
        storage
            .export_workspace(uuid, options)
            .await
            .map_err(|e| format!("Export failed: {}", e))
    } else if let Some(session_ids) = session {
        let uuids: Result<Vec<Uuid>, _> = session_ids
            .iter()
            .map(|s| Uuid::parse_str(s).map_err(|e| format!("Invalid session UUID {}: {}", s, e)))
            .collect();
        let uuids = uuids?;
        println!("Exporting {} session(s)...", uuids.len());
        storage
            .export_sessions(uuids, options)
            .await
            .map_err(|e| format!("Export failed: {}", e))
    } else {
        println!("Exporting all data...");
        storage
            .export_full(options)
            .await
            .map_err(|e| format!("Export failed: {}", e))
    }
}

async fn handle_export(
    output: Option<String>,
    compress: Option<String>,
    session: Option<Vec<String>>,
    workspace: Option<String>,
    checkpoints: bool,
    pretty: bool,
    force: bool,
) -> Result<(), String> {
    use std::io::{self, Write};
    use std::path::Path;

    // Generate default filename if not provided
    let output = output.unwrap_or_else(|| {
        let now = chrono::Local::now();
        format!("export-{}.json.gz", now.format("%Y-%m-%d_%H-%M-%S"))
    });

    let path = Path::new(&output);

    // Check if file exists and ask for confirmation
    if path.exists() && !force {
        print!("File '{}' already exists. Overwrite? [y/N] ", output);
        io::stdout().flush().ok();

        let mut input = String::new();
        io::stdin()
            .read_line(&mut input)
            .map_err(|e| format!("Failed to read input: {}", e))?;

        if !input.trim().eq_ignore_ascii_case("y") {
            println!("Export cancelled.");
            return Ok(());
        }
    }

    println!("Initializing export...");

    let daemon_config = DaemonConfig::load();

    // Determine compression
    let compression = if let Some(ref c) = compress {
        CompressionType::from_str(c)
            .ok_or_else(|| format!("Invalid compression type: {}. Use: none, gzip, zstd", c))?
    } else {
        CompressionType::from_path(Path::new(&output))
    };

    let options = ExportOptions {
        compression,
        include_checkpoints: checkpoints,
        pretty,
    };

    // Check storage backend configuration
    #[cfg(feature = "surrealdb-storage")]
    let use_surrealdb = daemon_config.storage_backend == "surrealdb";
    #[cfg(not(feature = "surrealdb-storage"))]
    let _use_surrealdb = false;

    // Perform export based on storage backend
    #[cfg(feature = "surrealdb-storage")]
    let export_data = if use_surrealdb {
        let endpoint = daemon_config
            .surrealdb_endpoint
            .as_ref()
            .ok_or("SurrealDB endpoint not configured in daemon.toml")?;

        println!("Source: SurrealDB at {}", endpoint);

        let storage = SurrealDBStorage::new(
            endpoint,
            daemon_config.surrealdb_username.as_deref(),
            daemon_config.surrealdb_password.as_deref(),
            Some(&daemon_config.surrealdb_namespace),
            Some(&daemon_config.surrealdb_database),
        )
        .await
        .map_err(|e| format!("Failed to connect to SurrealDB: {}", e))?;

        if let Some(workspace_id) = workspace {
            let uuid = Uuid::parse_str(&workspace_id)
                .map_err(|e| format!("Invalid workspace UUID: {}", e))?;
            println!("Exporting workspace {}...", workspace_id);
            storage
                .export_workspace(uuid, &options)
                .await
                .map_err(|e| format!("Export failed: {}", e))?
        } else if let Some(session_ids) = session {
            let uuids: Result<Vec<Uuid>, _> = session_ids
                .iter()
                .map(|s| {
                    Uuid::parse_str(s).map_err(|e| format!("Invalid session UUID {}: {}", s, e))
                })
                .collect();
            let uuids = uuids?;
            println!("Exporting {} session(s)...", uuids.len());
            storage
                .export_sessions(uuids, &options)
                .await
                .map_err(|e| format!("Export failed: {}", e))?
        } else {
            println!("Exporting all data...");
            storage
                .export_full(&options)
                .await
                .map_err(|e| format!("Export failed: {}", e))?
        }
    } else {
        export_from_rocksdb(&daemon_config, &options, workspace, session).await?
    };

    #[cfg(not(feature = "surrealdb-storage"))]
    let export_data = export_from_rocksdb(&daemon_config, &options, workspace, session).await?;

    // Write to file
    let path = Path::new(&output);
    let stats = write_export_file(&export_data, path, &options)
        .map_err(|e| format!("Failed to write export file: {}", e))?;

    println!();
    println!("Export complete!");
    println!("  File:        {}", output);
    println!("  Format:      {}", export_data.format_version);
    println!("  Compression: {:?}", compression);
    println!("  Sessions:    {}", export_data.metadata.session_count);
    println!("  Workspaces:  {}", export_data.metadata.workspace_count);
    println!("  Updates:     {}", export_data.metadata.update_count);
    println!("  Checkpoints: {}", export_data.metadata.checkpoint_count);
    println!("  Embeddings:  {}", export_data.metadata.embedding_count);

    // Show size info based on compression
    if compression == CompressionType::None {
        println!("  Size:        {} bytes", stats.file_size);
    } else {
        let compression_ratio =
            (1.0 - (stats.file_size as f64 / stats.uncompressed_size as f64)) * 100.0;
        println!(
            "  Size:        {} bytes ({:.0}% compression, from {} uncompressed)",
            stats.file_size, compression_ratio, stats.uncompressed_size
        );
    }

    Ok(())
}

async fn handle_import(
    input: String,
    session: Option<Vec<String>>,
    workspace: Option<String>,
    skip_existing: bool,
    overwrite: bool,
    list: bool,
) -> Result<(), String> {
    use std::path::Path;

    let path = Path::new(&input);

    if !path.exists() {
        return Err(format!("File not found: {}", input));
    }

    // List mode - just show contents
    if list {
        println!("Reading export file: {}", input);
        println!();

        let metadata =
            preview_export_file(path).map_err(|e| format!("Failed to read export: {}", e))?;

        println!("Export Metadata:");
        println!("  Format Version:  {}", metadata.post_cortex_version);
        println!("  Exported At:     {}", metadata.exported_at);
        println!("  Export Type:     {:?}", metadata.export_type);
        println!("  Sessions:        {}", metadata.session_count);
        println!("  Workspaces:      {}", metadata.workspace_count);
        println!("  Updates:         {}", metadata.update_count);
        println!("  Checkpoints:     {}", metadata.checkpoint_count);
        println!("  Embeddings:      {}", metadata.embedding_count);
        println!();

        let sessions =
            list_export_sessions(path).map_err(|e| format!("Failed to list sessions: {}", e))?;

        if !sessions.is_empty() {
            println!("Sessions in export:");
            println!("{:<38} {:<30} {}", "ID", "Name", "Updates");
            println!("{:-<38} {:-<30} {:-<10}", "", "", "");
            for (id, name, updates) in sessions {
                println!("{:<38} {:<30} {}", id, name, updates);
            }
        }

        return Ok(());
    }

    // Import mode
    println!("Reading export file: {}", input);

    let export_data =
        read_export_file(path).map_err(|e| format!("Failed to read export: {}", e))?;

    println!("  Format:      {}", export_data.format_version);
    println!("  Sessions:    {}", export_data.sessions.len());
    println!("  Workspaces:  {}", export_data.workspaces.len());
    println!();

    // Parse filters
    let session_filter = if let Some(ref ids) = session {
        let uuids: Result<Vec<Uuid>, _> = ids
            .iter()
            .map(|s| Uuid::parse_str(s).map_err(|e| format!("Invalid session UUID {}: {}", s, e)))
            .collect();
        Some(uuids?)
    } else {
        None
    };

    let workspace_filter = if let Some(ref id) = workspace {
        let uuid = Uuid::parse_str(id).map_err(|e| format!("Invalid workspace UUID: {}", e))?;
        Some(vec![uuid])
    } else {
        None
    };

    let options = ImportOptions {
        session_filter,
        workspace_filter,
        skip_existing,
        overwrite,
    };

    println!("Initializing import...");
    let daemon_config = DaemonConfig::load();

    // Check storage backend configuration
    #[cfg(feature = "surrealdb-storage")]
    let use_surrealdb = daemon_config.storage_backend == "surrealdb";
    #[cfg(not(feature = "surrealdb-storage"))]
    let use_surrealdb = false;

    #[cfg(feature = "surrealdb-storage")]
    if use_surrealdb {
        // Import to SurrealDB
        let endpoint = daemon_config
            .surrealdb_endpoint
            .as_ref()
            .ok_or("SurrealDB endpoint not configured in daemon.toml")?;

        println!("Target: SurrealDB at {}", endpoint);
        println!(
            "  Namespace: {}, Database: {}",
            daemon_config.surrealdb_namespace, daemon_config.surrealdb_database
        );

        let storage = SurrealDBStorage::new(
            endpoint,
            daemon_config.surrealdb_username.as_deref(),
            daemon_config.surrealdb_password.as_deref(),
            Some(&daemon_config.surrealdb_namespace),
            Some(&daemon_config.surrealdb_database),
        )
        .await
        .map_err(|e| format!("Failed to connect to SurrealDB: {}", e))?;

        println!("Importing data to SurrealDB...");
        let result = storage
            .import_data(export_data, &options)
            .await
            .map_err(|e| format!("Import failed: {}", e))?;

        println!();
        println!("Import complete!");
        println!("  Sessions imported:   {}", result.sessions_imported);
        println!("  Sessions skipped:    {}", result.sessions_skipped);
        println!("  Workspaces imported: {}", result.workspaces_imported);
        println!("  Workspaces skipped:  {}", result.workspaces_skipped);
        println!("  Updates imported:    {}", result.updates_imported);
        println!("  Checkpoints:         {}", result.checkpoints_imported);
        println!("  Embeddings:          {}", result.embeddings_imported);

        if !result.errors.is_empty() {
            println!();
            println!("Errors ({}):", result.errors.len());
            for err in &result.errors {
                println!("  - {}", err);
            }
        }

        return Ok(());
    }

    // Import to RocksDB (default)
    if use_surrealdb {
        return Err("SurrealDB storage backend requires surrealdb-storage feature".to_string());
    }

    let data_dir = daemon_config.data_directory;
    println!("Target: RocksDB at {}", data_dir);

    let storage = RealRocksDBStorage::new(&data_dir).await.map_err(|e| {
        let err_str = e.to_string();
        if err_str.contains("LOCK") || err_str.contains("Resource temporarily unavailable") {
            format!(
                "Database is locked by another process (likely the daemon).\n\
                     \n\
                     Please stop the daemon first:\n\
                     \n\
                     Option 1: Stop via CLI\n\
                     $ pkill -f 'pcx start'\n\
                     \n\
                     Option 2: Stop via launchctl (macOS)\n\
                     $ launchctl unload ~/Library/LaunchAgents/com.juliusml.post-cortex.plist\n\
                     \n\
                     Then retry the import command."
            )
        } else {
            format!("Failed to open storage: {}", e)
        }
    })?;

    println!("Importing data to RocksDB...");
    let result = storage
        .import_data(export_data, &options)
        .await
        .map_err(|e| format!("Import failed: {}", e))?;

    println!();
    println!("Import complete!");
    println!("  Sessions imported:   {}", result.sessions_imported);
    println!("  Sessions skipped:    {}", result.sessions_skipped);
    println!("  Workspaces imported: {}", result.workspaces_imported);
    println!("  Workspaces skipped:  {}", result.workspaces_skipped);
    println!("  Updates imported:    {}", result.updates_imported);
    println!("  Checkpoints:         {}", result.checkpoints_imported);
    println!("  Embeddings:          {}", result.embeddings_imported);

    if !result.errors.is_empty() {
        println!();
        println!("Errors ({}):", result.errors.len());
        for err in &result.errors {
            println!("  - {}", err);
        }
    }

    Ok(())
}

// ============================================================================
// Migration Handler
// ============================================================================

#[cfg(feature = "surrealdb-storage")]
async fn handle_migrate(
    from: String,
    to: String,
    source_path: Option<String>,
    target_path: Option<String>,
    remote_endpoint: Option<String>,
    username: Option<String>,
    password: Option<String>,
    dry_run: bool,
) -> Result<(), String> {
    use std::path::PathBuf;

    // Validate backends
    let from_backend = StorageBackendType::from_str(&from)
        .ok_or_else(|| format!("Invalid source backend: {}. Use: rocksdb", from))?;
    let to_backend = StorageBackendType::from_str(&to)
        .ok_or_else(|| format!("Invalid target backend: {}. Use: surrealdb", to))?;

    if from_backend == to_backend {
        return Err("Source and target backends must be different".to_string());
    }

    // Only rocksdb -> surrealdb is supported for now
    if from_backend != StorageBackendType::RocksDB || to_backend != StorageBackendType::SurrealDB {
        return Err("Only migration from RocksDB to SurrealDB is currently supported".to_string());
    }

    // Determine source path
    let daemon_config = DaemonConfig::load();
    let source = source_path
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(&daemon_config.data_directory));

    // Determine target (local path or remote endpoint)
    let is_remote = remote_endpoint.is_some();
    let target = target_path.map(PathBuf::from).unwrap_or_else(|| {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".post-cortex/surrealdb")
    });

    println!("Migration: {} -> {}", from, to);
    println!("  Source path: {}", source.display());
    if is_remote {
        println!(
            "  Target: remote SurrealDB at {}",
            remote_endpoint.as_ref().unwrap()
        );
    } else {
        println!("  Target path: {}", target.display());
    }
    println!();

    // Open source storage
    println!("Opening source storage (RocksDB)...");
    let source_storage = RealRocksDBStorage::new(&source).await.map_err(|e| {
        let err_str = e.to_string();
        if err_str.contains("LOCK") || err_str.contains("Resource temporarily unavailable") {
            format!(
                "Source database is locked by another process (likely the daemon).\n\
                     Please stop the daemon first: pkill -f 'pcx start'"
            )
        } else {
            format!("Failed to open source storage: {}", e)
        }
    })?;

    // Get source statistics
    let sessions = source_storage
        .list_sessions()
        .await
        .map_err(|e| format!("Failed to list sessions: {}", e))?;
    let workspaces = source_storage
        .list_workspaces()
        .await
        .map_err(|e| format!("Failed to list workspaces: {}", e))?;
    let checkpoints = source_storage
        .list_checkpoints()
        .await
        .map_err(|e| format!("Failed to list checkpoints: {}", e))?;

    println!("Source statistics:");
    println!("  Sessions:    {}", sessions.len());
    println!("  Workspaces:  {}", workspaces.len());
    println!("  Checkpoints: {}", checkpoints.len());
    println!();

    if dry_run {
        println!("DRY RUN - No data will be migrated.");
        println!();
        println!("Sessions to migrate:");
        for session_id in &sessions {
            if let Ok(session) = source_storage.load_session(*session_id).await {
                let name = session
                    .name()
                    .clone()
                    .unwrap_or_else(|| "Unnamed".to_string());
                let update_count = source_storage
                    .load_session_updates(*session_id)
                    .await
                    .map(|u| u.len())
                    .unwrap_or(0);
                println!("  {} - {} ({} updates)", session_id, name, update_count);
            }
        }
        return Ok(());
    }

    // Open target storage (local or remote)
    let target_storage = if let Some(endpoint) = &remote_endpoint {
        println!("Connecting to remote SurrealDB at {}...", endpoint);
        SurrealDBStorage::new(
            endpoint,
            username.as_deref(),
            password.as_deref(),
            None,
            None,
        )
        .await
        .map_err(|e| format!("Failed to connect to remote SurrealDB: {}", e))?
    } else {
        println!("Opening local SurrealDB at {}...", target.display());
        SurrealDBStorage::new(target.to_str().unwrap_or_default(), None, None, None, None)
            .await
            .map_err(|e| format!("Failed to open target storage: {}", e))?
    };

    // Migrate sessions
    println!();
    println!("Migrating sessions...");
    let mut migrated_sessions = 0;
    let mut migrated_updates = 0;
    let mut migrated_entities = 0;
    let mut migrated_relationships = 0;

    for session_id in &sessions {
        match source_storage.load_session(*session_id).await {
            Ok(session) => {
                let name = session
                    .name()
                    .clone()
                    .unwrap_or_else(|| "Unnamed".to_string());

                // Save session to target
                if let Err(e) = target_storage.save_session(&session).await {
                    println!("  [ERROR] {} - {}: {}", session_id, name, e);
                    continue;
                }

                // Get and save updates
                let update_count = match source_storage.load_session_updates(*session_id).await {
                    Ok(updates) => {
                        let count = updates.len();
                        if !updates.is_empty() {
                            if let Err(e) = target_storage
                                .batch_save_updates(*session_id, updates)
                                .await
                            {
                                println!(
                                    "  [WARN] {} - {}: Failed to save updates: {}",
                                    session_id, name, e
                                );
                            } else {
                                migrated_updates += count;
                            }
                        }
                        count
                    }
                    Err(e) => {
                        println!(
                            "  [WARN] {} - {}: Failed to load updates: {}",
                            session_id, name, e
                        );
                        0
                    }
                };

                // Extract and save entities from entity_graph to native SurrealDB graph
                let entities = session.entity_graph.get_all_entities();
                let entity_count = entities.len();
                for entity in &entities {
                    if let Err(e) = target_storage.upsert_entity(*session_id, entity).await {
                        println!(
                            "  [WARN] {} - {}: Failed to save entity '{}': {}",
                            session_id, name, entity.name, e
                        );
                    } else {
                        migrated_entities += 1;
                    }
                }

                // Extract and save relationships to native SurrealDB RELATE
                let relationships = session.entity_graph.get_all_relationships();
                let rel_count = relationships.len();
                for rel in &relationships {
                    if let Err(e) = target_storage.create_relationship(*session_id, rel).await {
                        println!(
                            "  [WARN] {} - {}: Failed to save relationship '{}'->'{}': {}",
                            session_id, name, rel.from_entity, rel.to_entity, e
                        );
                    } else {
                        migrated_relationships += 1;
                    }
                }

                println!(
                    "  [OK] {} - {} ({} updates, {} entities, {} relations)",
                    session_id, name, update_count, entity_count, rel_count
                );
                migrated_sessions += 1;
            }
            Err(e) => {
                println!("  [ERROR] {} - Failed to load: {}", session_id, e);
            }
        }
    }

    // Migrate workspaces
    println!();
    println!("Migrating workspaces...");
    let mut migrated_workspaces = 0;

    for ws in &workspaces {
        let session_ids: Vec<Uuid> = ws.sessions.iter().map(|(id, _)| *id).collect();
        if let Err(e) = target_storage
            .save_workspace_metadata(ws.id, &ws.name, &ws.description, &session_ids)
            .await
        {
            println!("  [ERROR] {} - {}: {}", ws.id, ws.name, e);
            continue;
        }
        println!(
            "  [OK] {} - {} ({} sessions)",
            ws.id,
            ws.name,
            ws.sessions.len()
        );
        migrated_workspaces += 1;
    }

    // Migrate checkpoints
    println!();
    println!("Migrating checkpoints...");
    let mut migrated_checkpoints = 0;

    for checkpoint in &checkpoints {
        if let Err(e) = target_storage.save_checkpoint(checkpoint).await {
            println!("  [ERROR] {} - {}", checkpoint.id, e);
            continue;
        }
        println!(
            "  [OK] {} (session: {})",
            checkpoint.id, checkpoint.session_id
        );
        migrated_checkpoints += 1;
    }

    // Migrate embeddings
    println!();
    println!("Migrating embeddings...");
    let mut migrated_embeddings = 0;

    use post_cortex::storage::traits::VectorStorage;
    let embeddings = source_storage
        .load_all_embeddings()
        .await
        .map_err(|e| format!("Failed to load embeddings: {}", e))?;

    let total_embeddings = embeddings.len();
    println!("  Found {} embeddings to migrate", total_embeddings);

    for (i, embedding) in embeddings.into_iter().enumerate() {
        let metadata = embedding.to_metadata();
        if let Err(e) = target_storage
            .add_vector(embedding.vector, metadata.clone())
            .await
        {
            println!(
                "  [WARN] Failed to migrate embedding {}: {}",
                metadata.id, e
            );
        } else {
            migrated_embeddings += 1;
        }

        // Progress every 100 embeddings
        if (i + 1) % 100 == 0 {
            println!(
                "  Progress: {}/{} embeddings migrated",
                i + 1,
                total_embeddings
            );
        }
    }
    println!("  [OK] {} embeddings migrated", migrated_embeddings);

    // Summary
    println!();
    println!("Migration complete!");
    println!(
        "  Sessions migrated:      {}/{}",
        migrated_sessions,
        sessions.len()
    );
    println!("  Updates migrated:       {}", migrated_updates);
    println!("  Entities migrated:      {}", migrated_entities);
    println!("  Relationships migrated: {}", migrated_relationships);
    println!(
        "  Workspaces migrated:    {}/{}",
        migrated_workspaces,
        workspaces.len()
    );
    println!(
        "  Checkpoints migrated:   {}/{}",
        migrated_checkpoints,
        checkpoints.len()
    );
    println!(
        "  Embeddings migrated:    {}/{}",
        migrated_embeddings, total_embeddings
    );

    Ok(())
}
