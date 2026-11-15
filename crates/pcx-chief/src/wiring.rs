// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The chief over its host handle: the one place its service's ports are
//! made, for its tools and for its turns. The chief acts as itself (its
//! name as the author) in the workspace it is asked from; asked from none,
//! each call of its turns names the one it works in. Its conversations and
//! actions are kept in the workspace it is asked from (asked from none: its
//! own, the one its settings name), its days' use in its own.

use std::sync::Arc;

use parking_lot::Mutex;
use pcx_documents::{HostDocuments, HostSessions, SharedDocuments};
use pcx_module_api::agent::Turns;
use pcx_module_api::grpc::GrpcChannel;
use pcx_module_api::{ModuleError, ModuleErrorKind, ModuleHost, Scope};

use crate::adapters::SystemClock;
use crate::domain::Kind;
use crate::service::Chief;
use crate::settings::Settings;

/// What a started chief works with.
#[derive(Clone)]
pub struct Running {
    /// Its host handle.
    pub host: Arc<dyn ModuleHost>,
    /// Its channel to the services of the modules it uses (W23).
    pub channel: GrpcChannel,
    /// Its settings.
    pub settings: Arc<Settings>,
}

/// The running chief, shared by the module and its turns; none until it
/// starts.
pub type Shared = Arc<Mutex<Option<Running>>>;

/// The running chief of `shared`.
///
/// # Errors
/// `FailedPrecondition` before it starts.
pub fn running(shared: &Shared) -> Result<Running, ModuleError> {
    shared.lock().clone().ok_or_else(|| {
        ModuleError::new(ModuleErrorKind::FailedPrecondition, "chief has not started")
    })
}

/// The chief's ports for one call.
pub struct Wired {
    running: Running,
    own: Scope,
    turns: Arc<dyn Turns>,
}

/// The chief's documents and sessions for one call ([`Wired::ports`]).
pub struct Ports<'a> {
    docs: HostDocuments<'a>,
    sessions: HostSessions<'a>,
    days: SharedDocuments<'a>,
}

impl Wired {
    /// The ports of a call in `scope`.
    ///
    /// # Errors
    /// `Denied` for a call asked from a workspace outside its reach (asked
    /// from none: the chief's own); the host gives the chief no agent.
    pub async fn new(running: Running, scope: &Scope) -> Result<Self, ModuleError> {
        let asked_from = scope
            .workspace_id
            .as_deref()
            .unwrap_or(&running.settings.workspace);
        if !scope.reaches(asked_from) {
            return Err(ModuleError::new(
                ModuleErrorKind::Denied,
                format!("workspace {asked_from} is outside the workspaces you can use"),
            ));
        }
        // It writes as itself, in the workspace it is asked from; asked from
        // none, each call of its turn names the workspace it works in.
        let own = Scope {
            author: Some(running.settings.name.clone()),
            session_id: None,
            ..scope.clone()
        };
        let turns = running.host.turns().await?;
        Ok(Self {
            running,
            own,
            turns,
        })
    }

    /// Its conversations, actions and sessions in the workspace it is asked
    /// from (asked from none: the one its settings name), and its days' use
    /// in the one its settings name, the organisation's: the budget is one
    /// for the host (docs/modules/chief.md, its workspace).
    pub fn ports(&self) -> Ports<'_> {
        let channel = &self.running.channel;
        let its = self.running.settings.workspace.as_str();
        let asked_from = self.own.workspace_id.as_deref().unwrap_or(its);
        Ports {
            docs: HostDocuments::new(channel.clone(), &self.own, asked_from),
            sessions: HostSessions::new(channel.clone(), &self.own, asked_from),
            days: SharedDocuments::new(channel.clone(), &self.own, its, vec![Kind::Day.document()]),
        }
    }

    /// The chief over `ports` (its [`Wired::ports`]).
    pub fn chief<'a>(&'a self, ports: &'a Ports<'a>) -> Chief<'a> {
        Chief {
            docs: &ports.docs,
            sessions: &ports.sessions,
            days: &ports.days,
            turns: self.turns.as_ref(),
            clock: &SystemClock,
            settings: &self.running.settings,
            scope: &self.own,
        }
    }
}
