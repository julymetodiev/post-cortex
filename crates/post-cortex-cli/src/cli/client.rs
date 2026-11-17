// Copyright (c) 2025, 2026 Julius ML
// MIT License

//! The client commands.

use clap::Subcommand;

use crate::cli::{CardAction, ProfileAction, SessionAction, WorkspaceAction};

/// The client commands: over the daemon's or the cloud's public APIs only.
#[derive(Subcommand)]
pub enum ClientCommand {
    /// Set up coding agents for post-cortex: once for every project
    /// (default), or for this project with --project
    #[command(long_about = "Set up coding agents for post-cortex.\n\n\
        By default the setup is global: the agents that read a user-level config\n\
        (Claude Code through its plugin, Codex, Cursor, Gemini CLI, Claude Desktop)\n\
        are set up once, and their hooks recognise each project from its git remote\n\
        (or its directory). Nothing is written into the project. For Claude Code it\n\
        prints the two /plugin commands that install the post-cortex plugin.\n\n\
        With --project, the session and workspace are created (or reused) and the\n\
        files go into the current directory with the ids (CLAUDE.md, .claude/,\n\
        AGENTS.md, ...) plus .post-cortex.json, so other agents and teammates resolve\n\
        to the same memory. Agents without a user-level config always get this.\n\n\
        The backend is the active profile (the hosted cloud unless changed with\n\
        `pcx profile use`); a cloud profile without a token runs `pcx login` first.\n\n\
        EXAMPLES:\n\n\
        Global (Claude Code plugin):\n\
        $ pcx setup\n\n\
        Global for Codex and Cursor:\n\
        $ pcx setup --client codex,cursor\n\n\
        This project, non-interactive:\n\
        $ pcx setup --project --name my-project --non-interactive\n\n\
        Against the self-hosted daemon in daemon.toml:\n\
        $ pcx setup --self-hosted")]
    Setup {
        /// Set up this project (files with the session and workspace ids in
        /// the current directory) instead of every project
        #[arg(long)]
        project: bool,

        /// Project name (default: current directory name)
        #[arg(long)]
        name: Option<String>,

        /// Skip interactive prompts, use defaults
        #[arg(long)]
        non_interactive: bool,

        /// Capture every conversation turn into the session (a Stop hook
        /// posting prompts and replies to the daemon); off unless given or
        /// chosen at the prompt
        #[arg(long)]
        capture: bool,

        /// Daemon URL the hooks talk to (cards on session start, capture);
        /// default the local daemon from daemon.toml. Use it when Claude Code
        /// talks to a remote daemon, e.g. `http://ml.example.ts.net:3737`
        #[arg(long, value_name = "URL")]
        daemon_url: Option<String>,

        /// Shorthand for `--capture --daemon-url URL`
        #[arg(long, value_name = "URL", conflicts_with = "daemon_url")]
        capture_url: Option<String>,

        /// Backend profile the clients and hooks use (default: the active
        /// profile, `cloud` unless changed with `pcx profile use`)
        #[arg(long, value_name = "NAME", conflicts_with_all = ["daemon_url", "capture_url"])]
        profile: Option<String>,

        /// Use the self-hosted daemon from daemon.toml (= `--profile self`)
        #[arg(long, conflicts_with = "profile")]
        self_hosted: bool,

        /// Coding agents to set up (repeat or comma-separate): claude-code,
        /// codex, cursor, opencode, gemini, vscode, windsurf, zed, cline,
        /// roo, goose, claude-desktop. Default: claude-code
        #[arg(
            long = "client",
            value_name = "CLIENT",
            value_enum,
            value_delimiter = ','
        )]
        clients: Vec<crate::clients::Client>,
    },

    /// Print a coding agent's MCP server entry for post-cortex, to paste
    /// into its config (`pcx setup --client` writes it for you)
    McpConfig {
        /// The agent: claude-code, codex, cursor, opencode, gemini, vscode,
        /// windsurf, zed, cline, roo, goose, claude-desktop
        #[arg(value_enum)]
        client: crate::clients::Client,

        /// A self-hosted daemon's URL (an HTTP entry for that daemon)
        #[arg(long, value_name = "URL", conflicts_with = "profile")]
        url: Option<String>,

        /// Backend profile (default: the active profile)
        #[arg(long, value_name = "NAME")]
        profile: Option<String>,
    },

    /// Stdio MCP server for a backend profile: forwards stdio MCP to the
    /// profile's `/mcp` with its token (what client configs run for the
    /// cloud). Plain `pcx` stays the local daemon proxy.
    Mcp {
        /// Backend profile (default: PC_PROFILE, else the active profile)
        #[arg(long, value_name = "NAME")]
        profile: Option<String>,

        /// Author of the writes (`X-PCX-Author`; default: PC_AUTHOR)
        #[arg(long, value_name = "NAME")]
        author: Option<String>,
    },

    /// Store an API key for a backend profile (default: the active one)
    #[command(long_about = "Store an API key for a backend profile.\n\n\
        Without --token the key is asked for without echo (or read from stdin\n\
        when it is not a terminal). It is checked against the backend first and\n\
        kept in ~/.post-cortex/credentials.json (mode 0600). Create a key for the\n\
        hosted cloud at https://app.pcx.is.\n\n\
        EXAMPLES:\n\n\
        $ pcx login\n\
        $ pcx login --profile self --token \"$PC_AUTH_TOKEN\"")]
    Login {
        /// Profile to log in to (default: the active profile)
        #[arg(long, value_name = "NAME")]
        profile: Option<String>,

        /// The API key (default: asked for)
        #[arg(long, value_name = "TOKEN")]
        token: Option<String>,
    },

    /// Connect this daemon to an organisation of the cloud: its people
    /// reach it through the cloud, its data staying here
    #[command(
        long_about = "Connect this daemon to an organisation of the cloud.\n\n\
        Prints a code and a link; an owner or admin of the organisation opens the\n\
        link, chooses which of the daemon's workspaces its people see, and approves.\n\
        The daemon's token is kept in connect.json beside daemon.toml (mode 0600);\n\
        from its next start the daemon holds an outbound tunnel to the cloud (no\n\
        open port). The daemon must be running.\n\n\
        With --runner the daemon also offers its tool modules ([runner] in\n\
        daemon.toml) to the organisation's agents; the approver gives the runner to\n\
        a workspace of the organisation.\n\n\
        EXAMPLES:\n\n\
        $ pcx connect\n\
        $ pcx connect --name ops-ml\n\
        $ pcx connect --runner --name ml-runner\n\
        $ pcx connect --off"
    )]
    Connect {
        /// The cloud (default: <https://api.pcx.is>)
        #[arg(long, value_name = "URL")]
        cloud: Option<String>,

        /// The daemon's name, as the organisation sees it (default: the
        /// machine's name)
        #[arg(long, value_name = "NAME")]
        name: Option<String>,

        /// Offer a runner too: the tool modules `[runner]` in daemon.toml
        /// lends, to the organisation's agents
        #[arg(long)]
        runner: bool,

        /// Leave the organisation: the cloud revokes the daemon's token
        #[arg(long, conflicts_with_all = ["cloud", "name", "runner"])]
        off: bool,
    },

    /// Forget the stored API key of a profile (default: the active one)
    Logout {
        /// The profile (default: the active one).
        #[arg(long, value_name = "NAME")]
        profile: Option<String>,
    },

    /// List, switch, add or remove backend profiles
    Profile {
        /// What to do.
        #[command(subcommand)]
        action: ProfileAction,
    },

    /// Run a coding agent's hook (called by the hooks `pcx setup --client`
    /// installs): reads the hook payload on stdin, prints the reply
    #[command(hide = true)]
    Hook {
        /// The coding agent.
        #[arg(value_enum)]
        client: crate::clients::Client,
        /// Hook event (session-start, pre-edit, prompt, stop, ...)
        event: String,
        /// The project's session (project mode); without it and
        /// --workspace, the project is resolved from the payload's cwd
        #[arg(long, requires = "workspace")]
        session: Option<String>,
        /// The project's workspace (with --session).
        #[arg(long, requires = "session")]
        workspace: Option<String>,
        /// Backend profile (hooks written since profiles)
        #[arg(long)]
        profile: Option<String>,
        /// Daemon URL (self-hosted; hooks written by older setups pass only
        /// this)
        #[arg(long)]
        url: Option<String>,
    },

    /// Manage workspaces
    Workspace {
        /// What to do.
        #[command(subcommand)]
        action: WorkspaceAction,
    },

    /// Show or edit the project card or the user card
    #[command(
        long_about = "Show or edit a card: a short markdown note put in front of every \
        session. The project card (stack, conventions, current goals) belongs to a workspace; \
        the user card (preferences) to you. Without --workspace or --user the workspace in \
        ./CLAUDE.md is used. Talks to the daemon in daemon.toml unless --url is given; set \
        PC_AUTH_TOKEN for a daemon with an auth token.\n\n\
        EXAMPLES:\n\n\
        $ pcx card show\n\
        $ pcx card edit --user\n\
        $ pcx card edit --url http://ml.example.ts.net:3737"
    )]
    Card {
        /// What to do.
        #[command(subcommand)]
        action: CardAction,
    },

    /// Manage sessions
    Session {
        /// What to do.
        #[command(subcommand)]
        action: SessionAction,
    },

    /// Ask the daemon's agent: one turn, its answer printed as it comes
    Agent {
        /// What to ask
        prompt: String,

        /// The session whose conversation the turn continues (default: a
        /// new one, whose id is printed)
        #[arg(long)]
        session: Option<String>,
    },
}
