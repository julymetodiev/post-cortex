// Copyright (c) 2025, 2026 Julius ML
// MIT License

//! The daemon's commands and their arguments.

use clap::Subcommand;

/// The daemon's commands: run it, and the admin commands over its storage
/// (through the running daemon, else offline).
#[derive(Subcommand)]
pub enum DaemonCommand {
    /// Start the MCP server (daemon mode)
    Start {
        /// Run as background daemon (used internally for auto-start)
        #[arg(long)]
        daemon: bool,

        /// Port to listen on (default: daemon.toml / PC_PORT, else 3737)
        #[arg(long, short)]
        port: Option<u16>,

        /// Host to bind to (default: daemon.toml / PC_HOST, else 127.0.0.1)
        #[arg(long)]
        host: Option<String>,
    },

    /// Check if daemon is running
    Status,

    /// The launcher of the pot `pcx-modules` (docs/modules/dynamic.md):
    /// starts dynamic modules' processes for the hosts that reach its socket.
    #[command(name = "module-launcher", hide = true)]
    ModuleLauncher(Box<ModuleLauncherArgs>),

    /// Run a program confined by a sandbox policy (Linux: Landlock and
    /// seccomp); the daemon starts its modules' processes through it.
    #[command(hide = true)]
    SandboxRun {
        /// The policy, as the sandbox encodes it.
        #[arg(long)]
        policy: String,
        /// The program and its arguments, after `--`.
        #[arg(last = true, required = true)]
        command: Vec<String>,
    },

    /// Stop running daemon
    Stop,

    /// Initialize configuration file
    Init,

    /// Vectorize all sessions, then repair the message vectors of each
    /// (stale ones removed, missing chunks embedded again)
    VectorizeAll {
        /// Only repair the message vectors (no record re-vectorization)
        #[arg(long)]
        messages: bool,
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
        Without the captured message streams (they can be large):\n\
        $ pcx export --output backup.json.gz --no-messages\n\n\
        Pretty-printed JSON for debugging:\n\
        $ pcx export --output backup.json --pretty\n\n\
        Force overwrite without asking:\n\
        $ pcx export --output backup.json.gz --force")]
    Export(ExportArgs),

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
        $ pcx import --input backup.json --overwrite\n\n\
        Keep what is here, but take a card the file has written later:\n\
        $ pcx import --input backup.json --skip-existing --cards-newer")]
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

        /// Overwrite existing sessions/workspaces, and replace the history of
        /// a card that diverged from the file's
        #[arg(long)]
        overwrite: bool,

        /// A card whose history diverged from the file's: take the file's
        /// latest version when it was written later (it becomes the next
        /// version here). Without it the card here stays.
        #[arg(long)]
        cards_newer: bool,

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
    Migrate(MigrateArgs),
}

/// Arguments of `pcx export`.
#[derive(clap::Args)]
pub struct ExportArgs {
    /// Output file path. Extension determines compression:
    /// .json (none), .json.gz (gzip), .json.zst (zstd)
    /// If not specified, generates: export-YYYY-MM-DD_HH-MM-SS.json.gz
    #[arg(short, long, value_name = "FILE")]
    pub output: Option<String>,

    /// Compression type: none, gzip, zstd.
    /// Auto-detected from file extension if not specified.
    #[arg(short, long, value_name = "TYPE")]
    pub compress: Option<String>,

    /// Export only specific session(s). Can be repeated.
    #[arg(long, value_name = "UUID")]
    pub session: Option<Vec<String>>,

    /// Export only specific workspace and all its sessions
    #[arg(long, value_name = "UUID")]
    pub workspace: Option<String>,

    /// Include checkpoints in export
    #[arg(long)]
    pub checkpoints: bool,

    /// Pretty print JSON (human-readable, larger file size)
    #[arg(long)]
    pub pretty: bool,

    /// Leave the captured message streams (and their vectors) out
    #[arg(long)]
    pub no_messages: bool,

    /// Overwrite existing file without asking
    #[arg(long, short = 'f')]
    pub force: bool,
}

/// Arguments of `pcx migrate`.
#[derive(clap::Args)]
pub struct MigrateArgs {
    /// Source storage backend (rocksdb)
    #[arg(long, value_name = "BACKEND")]
    pub from: String,

    /// Target storage backend (surrealdb)
    #[arg(long, value_name = "BACKEND")]
    pub to: String,

    /// Source data path (default: ~/.post-cortex/data)
    #[arg(long, value_name = "PATH")]
    pub source_path: Option<String>,

    /// Target data path for local SurrealDB (default: ~/.post-cortex/surrealdb)
    #[arg(long, value_name = "PATH")]
    pub target_path: Option<String>,

    /// Remote SurrealDB endpoint (e.g., localhost:8000 or ws://host:port)
    #[arg(long, value_name = "ENDPOINT")]
    pub remote_endpoint: Option<String>,

    /// Username for remote SurrealDB authentication
    #[arg(long, value_name = "USER")]
    pub username: Option<String>,

    /// Password for remote SurrealDB authentication
    #[arg(long, value_name = "PASS")]
    pub password: Option<String>,

    /// Dry run - show what would be migrated without actually migrating
    #[arg(long)]
    pub dry_run: bool,
}

/// What `pcx module-launcher` serves.
#[derive(clap::Args, Debug)]
pub struct ModuleLauncherArgs {
    /// The socket it serves.
    #[arg(long)]
    pub socket: std::path::PathBuf,
    /// The directory it shares with the hosts, of the launches' sockets.
    #[arg(long)]
    pub sockets: std::path::PathBuf,
    /// Where the modules' scratch directories are.
    #[arg(long)]
    pub scratch: std::path::PathBuf,
    /// A runtime it starts a dynamic module with; repeated.
    #[arg(long = "runtime", required = true)]
    pub runtimes: Vec<std::path::PathBuf>,
    /// A program of the cloud's catalog it starts without the network;
    /// repeated.
    #[arg(long = "program")]
    pub programs: Vec<std::path::PathBuf>,
    /// A program of the cloud's catalog it starts with the network (public
    /// addresses only on FreeBSD); repeated.
    #[arg(long = "program-network")]
    pub network_programs: Vec<std::path::PathBuf>,
    /// What every process reads (the runtimes, the bindings); repeated.
    #[arg(long = "read")]
    pub always_read: Vec<std::path::PathBuf>,
    /// Under what a process may read besides; repeated.
    #[arg(long = "store")]
    pub stores: Vec<std::path::PathBuf>,
    /// Where prepared bundles and environments are kept.
    #[arg(long = "prepared")]
    pub prepared: std::path::PathBuf,
    /// The Python that installs a bundle's packages.
    #[arg(long)]
    pub python: std::path::PathBuf,
    /// The native packages built for this platform (FreeBSD: the image's).
    #[arg(long)]
    pub wheelhouse: Option<std::path::PathBuf>,
    /// The protobuf compiler, for a bundle with protos.
    #[arg(long)]
    pub protoc: Option<std::path::PathBuf>,
    /// Its plugin for TypeScript.
    #[arg(long)]
    pub protoc_gen_es: Option<std::path::PathBuf>,
    /// Deno, for a TS bundle with a lock file.
    #[arg(long)]
    pub deno: Option<std::path::PathBuf>,
    /// The import map Deno runs a module with.
    #[arg(long)]
    pub imports: Option<std::path::PathBuf>,
    /// The unprivileged user a process runs as (FreeBSD's jail).
    #[arg(long)]
    pub user: Option<String>,
    /// The group (gid) of the hosts it serves when they do not run as root,
    /// such as the cloud's user: its socket, the directory of the launches'
    /// sockets and each launch's directory are theirs too.
    #[arg(long)]
    pub host_group: Option<u32>,
}
