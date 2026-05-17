// Copyright (c) 2025 Julius ML
// MIT License

//! CLI surface: clap definitions for `pcx` and its subcommands.

use clap::{Parser, Subcommand};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Parser)]
#[command(name = "pcx")]
#[command(version = VERSION)]
#[command(about = "Post-Cortex - Intelligent conversation memory system")]
#[command(
    long_about = "Post-Cortex unified MCP server supporting stdio and SSE transports.\n\n\
    When run without arguments, starts in stdio mode (for MCP clients).\n\
    The daemon is auto-started in background if not already running."
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Subcommand)]
pub enum Commands {
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

    /// Set up Post-Cortex for a project (interactive)
    #[command(long_about = "Interactive project setup wizard.\n\n\
        Creates a session, workspace, and generates .claude/ configuration files\n\
        (CLAUDE.md, settings.json, hooks) with the correct IDs.\n\n\
        EXAMPLES:\n\n\
        Interactive setup:\n\
        $ pcx setup\n\n\
        Non-interactive with defaults:\n\
        $ pcx setup --name my-project --non-interactive")]
    Setup {
        /// Project name (default: current directory name)
        #[arg(long)]
        name: Option<String>,

        /// Skip interactive prompts, use defaults
        #[arg(long)]
        non_interactive: bool,
    },

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
pub enum WorkspaceAction {
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
pub enum SessionAction {
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
