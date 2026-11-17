// Copyright (c) 2025, 2026 Julius ML
// MIT License

//! The actions of `pcx profile`, `card`, `workspace` and `session`.

use clap::Subcommand;

/// `pcx profile` actions.
#[derive(Subcommand)]
pub enum ProfileAction {
    /// List the profiles (the active one marked with *)
    List,
    /// Make a profile the active one
    Use {
        /// The profile.
        #[arg(value_name = "NAME")]
        name: String,
    },
    /// Add a profile, or change a profile's URL
    Add {
        /// The profile.
        #[arg(value_name = "NAME")]
        name: String,
        /// The backend's base URL, e.g. <https://pcx.example.com>
        #[arg(long, value_name = "URL")]
        url: String,
    },
    /// Remove a profile and its stored key (for cloud and self: the URL
    /// override)
    Remove {
        /// The profile.
        #[arg(value_name = "NAME")]
        name: String,
    },
}

/// `pcx card` actions.
#[derive(Subcommand)]
pub enum CardAction {
    /// Print the latest version of a card
    Show {
        /// Workspace whose project card to show (default: from ./CLAUDE.md)
        #[arg(long)]
        workspace: Option<String>,
        /// Show the user card instead
        #[arg(long)]
        user: bool,
        /// Daemon URL (default: the daemon in daemon.toml)
        #[arg(long, value_name = "URL")]
        url: Option<String>,
    },
    /// Edit a card in $EDITOR and save it as a new version
    Edit {
        /// Workspace whose project card to edit (default: from ./CLAUDE.md)
        #[arg(long)]
        workspace: Option<String>,
        /// Edit the user card instead
        #[arg(long)]
        user: bool,
        /// Daemon URL (default: the daemon in daemon.toml)
        #[arg(long, value_name = "URL")]
        url: Option<String>,
    },
}

/// `pcx workspace` actions.
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
        /// The profile.
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

/// `pcx session` actions.
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
