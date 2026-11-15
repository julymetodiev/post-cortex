// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The modules built into the daemon, each off until `[[builtins]]` names it.

use std::sync::Arc;

use pcx_agent::AgentModule;
use pcx_board::BoardModule;
use pcx_chief::ChiefModule;
use pcx_context::ContextModule;
use pcx_discord::DiscordModule;
use pcx_http::HttpModule;
use pcx_mail::MailModule;
use pcx_models::ModelsModule;
use pcx_module_api::Module;
use pcx_observe::ObserveModule;
use pcx_ops::OpsModule;
use pcx_scheduler::SchedulerModule;
use pcx_skills::SkillsModule;
use pcx_team::TeamModule;
use pcx_telegram::TelegramModule;
use pcx_tools_fs::FsModule;
use pcx_tools_git::GitModule;
use pcx_tools_shell::ShellModule;
use pcx_webhooks::WebhooksModule;
use pcx_workflow::WorkflowModule;
use serde::{Deserialize, Serialize};

/// A module built into the daemon, off until `[[builtins]]` names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Builtin {
    /// `tools-fs`: read, write, edit, list_dir, grep, glob.
    #[serde(rename = "tools-fs")]
    ToolsFs,
    /// `tools-shell`: bash and tmux sessions.
    #[serde(rename = "tools-shell")]
    ToolsShell,
    /// `tools-git`: status, diff, log, branches, commits.
    #[serde(rename = "tools-git")]
    ToolsGit,
    /// `board`: boards, tasks, their discussion and decisions.
    #[serde(rename = "board")]
    Board,
    /// `skills`: a library of skills, offered as prompts.
    #[serde(rename = "skills")]
    Skills,
    /// `models`: every model of OpenRouter, Anthropic, Z.AI and OpenAI,
    /// each provider on with its API key (with the `network` grant).
    #[serde(rename = "models")]
    Models,
    /// `agent`: turns with models and tools.
    #[serde(rename = "agent")]
    Agent,
    /// `context`: the agent's recall and the summaries of long
    /// conversations.
    #[serde(rename = "context")]
    Context,
    /// `team`: agents with roles, teams, and their members' work on the
    /// board.
    #[serde(rename = "team")]
    Team,
    /// `chief`: the host's main agent, under its grant, audit and budget.
    #[serde(rename = "chief")]
    Chief,
    /// `webhooks`: events from outside (Alertmanager) as board tasks.
    #[serde(rename = "webhooks")]
    Webhooks,
    /// `scheduler`: jobs at set times (a script, or a board task).
    #[serde(rename = "scheduler")]
    Scheduler,
    /// `mail`: mail with the organisation's own credentials, each send
    /// logged (with the `network` grant).
    #[serde(rename = "mail")]
    Mail,
    /// `http`: calls of outside APIs through connections with the
    /// organisation's credentials, each call logged (with the `network`
    /// grant; private networks only when its settings ask).
    #[serde(rename = "http")]
    Http,
    /// `telegram`: the first channel, approvals with buttons, the chief in
    /// a chat (with the `network` grant).
    #[serde(rename = "telegram")]
    Telegram,
    /// `discord`: a channel to a Discord server, what its people write as
    /// events, posts, replies, roles and forum tags (with the `network`
    /// grant).
    #[serde(rename = "discord")]
    Discord,
    /// `observe`: reads metrics, logs, alerts and a cluster, never changes
    /// them (with the `network` grant, and `filesystem` for its
    /// credentials' files).
    #[serde(rename = "observe")]
    Observe,
    /// `ops`: changes a running system through typed actions, each a
    /// board decision (with the `network` grant, and `filesystem` for its
    /// credentials' files).
    #[serde(rename = "ops")]
    Ops,
    /// `workflow`: graphs of steps over every module's tools (with the
    /// `models` grant, and `*` in its `uses`).
    #[serde(rename = "workflow")]
    Workflow,
}

impl Builtin {
    /// Every built-in module.
    pub const ALL: [Self; 19] = [
        Self::ToolsFs,
        Self::ToolsShell,
        Self::ToolsGit,
        Self::Board,
        Self::Skills,
        Self::Models,
        Self::Agent,
        Self::Context,
        Self::Team,
        Self::Chief,
        Self::Webhooks,
        Self::Scheduler,
        Self::Mail,
        Self::Http,
        Self::Telegram,
        Self::Discord,
        Self::Observe,
        Self::Ops,
        Self::Workflow,
    ];

    /// The built-in module of id `id`, if any.
    #[must_use]
    pub fn of(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|b| b.id() == id)
    }

    /// Its module id.
    #[must_use]
    pub fn id(self) -> &'static str {
        match self {
            Self::ToolsFs => FsModule::ID,
            Self::ToolsShell => ShellModule::ID,
            Self::ToolsGit => GitModule::ID,
            Self::Board => BoardModule::ID,
            Self::Skills => SkillsModule::ID,
            Self::Models => ModelsModule::ID,
            Self::Agent => AgentModule::ID,
            Self::Context => ContextModule::ID,
            Self::Team => TeamModule::ID,
            Self::Chief => ChiefModule::ID,
            Self::Webhooks => WebhooksModule::ID,
            Self::Scheduler => SchedulerModule::ID,
            Self::Mail => MailModule::ID,
            Self::Http => HttpModule::ID,
            Self::Telegram => TelegramModule::ID,
            Self::Discord => DiscordModule::ID,
            Self::Observe => ObserveModule::ID,
            Self::Ops => OpsModule::ID,
            Self::Workflow => WorkflowModule::ID,
        }
    }

    /// The module itself, new.
    #[must_use]
    pub fn module(self) -> Arc<dyn Module> {
        match self {
            Self::ToolsFs => Arc::new(FsModule::new()),
            Self::ToolsShell => Arc::new(ShellModule::new()),
            Self::ToolsGit => Arc::new(GitModule::new()),
            Self::Board => Arc::new(BoardModule::new()),
            Self::Skills => Arc::new(SkillsModule::new()),
            Self::Models => Arc::new(ModelsModule::new()),
            Self::Agent => Arc::new(AgentModule::new()),
            Self::Context => Arc::new(ContextModule::new()),
            Self::Team => Arc::new(TeamModule::new()),
            Self::Chief => Arc::new(ChiefModule::new()),
            Self::Webhooks => Arc::new(WebhooksModule::new()),
            Self::Scheduler => Arc::new(SchedulerModule::new()),
            Self::Mail => Arc::new(MailModule::new()),
            // The daemon's own machine: its settings may reach private
            // networks.
            Self::Http => Arc::new(HttpModule::new(false)),
            Self::Telegram => Arc::new(TelegramModule::new()),
            Self::Discord => Arc::new(DiscordModule::new()),
            Self::Observe => Arc::new(ObserveModule::new()),
            Self::Ops => Arc::new(OpsModule::new()),
            Self::Workflow => Arc::new(WorkflowModule::new()),
        }
    }
}
