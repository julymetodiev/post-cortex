// Copyright (c) 2025 Julius ML
// MIT License

//! CLI surface: clap definitions for `pcx` and its subcommands.

use clap::{Parser, Subcommand};

/// The `pcx` version.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Parser)]
#[command(name = "pcx")]
#[command(version = VERSION)]
#[command(about = "Post-Cortex - Intelligent conversation memory system")]
#[command(
    long_about = "Post-Cortex unified MCP server supporting stdio and SSE transports.\n\n\
    When run without arguments, starts in stdio mode (for MCP clients).\n\
    The daemon is auto-started in background if not already running.\n\n\
    Clients use a backend profile: the hosted cloud (default, `pcx login`) or a\n\
    self-hosted daemon (`pcx setup --self-hosted`, `pcx profile use self`)."
)]
/// The `pcx` command line.
pub struct Cli {
    /// The command; none runs the stdio MCP bridge to the local daemon.
    #[command(subcommand)]
    pub command: Option<Commands>,
}

/// Every command: the daemon's own, and the client commands.
#[derive(Subcommand)]
pub enum Commands {
    /// The daemon's own commands.
    #[command(flatten)]
    Daemon(DaemonCommand),
    /// The client commands.
    #[command(flatten)]
    Client(ClientCommand),
}

mod actions;
mod client;
mod daemon;

pub use actions::{CardAction, ProfileAction, SessionAction, WorkspaceAction};
pub use client::ClientCommand;
pub use daemon::{DaemonCommand, ExportArgs, MigrateArgs, ModuleLauncherArgs};

#[cfg(test)]
mod tests;
