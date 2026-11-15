// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! A module's turns (ARCHITECTURE section 7, the agent): a turn a module
//! asks of the agent runs under a token of the host's own, and the agent's
//! calls in it reach what that module's `uses` and grant allow, and the
//! workspaces the turn was asked to reach, never more.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use parking_lot::Mutex;
use pcx_module_api::agent::{TurnEnd, TurnEvent, TurnGrant, TurnRequest, Turns};
use pcx_module_api::{ModuleError, ModuleErrorKind, ModuleKind, Scope};
use tokio::sync::mpsc;

use crate::Loaded;
use crate::policy::Guarded;

/// A turn a module asked of the agent that runs now.
#[derive(Clone)]
struct Asked {
    /// The module whose turn it is.
    module: String,
    /// The workspaces the turn was asked to reach; none: every one.
    reach: Option<Vec<String>>,
}

/// The turns modules asked of the agent that run now, by their tokens.
#[derive(Default)]
pub struct Delegations {
    running: Mutex<HashMap<String, Asked>>,
}

impl Delegations {
    /// The turn `token` names; none when no such turn runs.
    fn asked(&self, token: &str) -> Option<Asked> {
        self.running.lock().get(token).cloned()
    }
}

/// A turn's token, known to the host while the turn runs.
struct Running<'a> {
    token: String,
    delegations: &'a Delegations,
}

impl<'a> Running<'a> {
    fn open(delegations: &'a Delegations, module: &str, reach: Option<Vec<String>>) -> Self {
        let token = uuid::Uuid::new_v4().to_string();
        delegations.running.lock().insert(
            token.clone(),
            Asked {
                module: module.to_string(),
                reach,
            },
        );
        Self { token, delegations }
    }
}

impl Drop for Running<'_> {
    fn drop(&mut self) {
        self.delegations.running.lock().remove(&self.token);
    }
}

/// The agent's turns as module `module` asks them.
pub struct Delegated {
    agent: Arc<dyn Turns>,
    module: String,
    delegations: Arc<Delegations>,
}

impl Delegated {
    /// The turns of `agent` for `module`, their tokens kept in
    /// `delegations`.
    pub fn new(agent: Arc<dyn Turns>, module: &str, delegations: Arc<Delegations>) -> Self {
        Self {
            agent,
            module: module.to_string(),
            delegations,
        }
    }
}

#[async_trait]
impl Turns for Delegated {
    /// The agent's turn in `scope` with a new token, whatever token `scope`
    /// carried; the token is forgotten when the turn ends or is dropped.
    /// The host keeps the scope's reach with the token and holds every
    /// call the agent makes in the turn to it.
    async fn run(
        &self,
        scope: &Scope,
        request: &TurnRequest,
        events: &mpsc::Sender<TurnEvent>,
    ) -> Result<TurnEnd, ModuleError> {
        let running = Running::open(&self.delegations, &self.module, scope.reach.clone());
        let scope = Scope {
            turn: Some(running.token.clone()),
            ..scope.clone()
        };
        self.agent.run(&scope, request, events).await
    }

    async fn models(&self) -> Result<Vec<pcx_module_api::model::ModelInfo>, ModuleError> {
        self.agent.models().await
    }
}

/// The entry of `Manifest.uses` that also reaches the modules added in place
/// (`Host::change`): each is running because a person approved its code
/// (owner 2026-11-02, `modules/dynamic.md`, loaded).
const ADDED_IN_PLACE: &str = "dynamic";

/// Whether a module of `uses` reaches module `callee`, with `added` the modules
/// added in place.
pub fn reaches(uses: &[String], added: &[String], callee: &str) -> bool {
    pcx_module_api::uses(uses, callee)
        || (added.iter().any(|id| id == callee) && pcx_module_api::uses(uses, ADDED_IN_PLACE))
}

/// What the calls of module `caller` reach in the turn `turn` names: the
/// agent's in a module's turn that module's; any other its own.
///
/// # Errors
/// `Denied` for a token of no running turn; `FailedPrecondition` when the
/// module whose turn it is no longer runs.
pub fn reach<'a>(
    loaded: &'a Loaded,
    delegations: &Delegations,
    caller: &Guarded,
    turn: Option<&str>,
    author: Option<&str>,
) -> Result<Reach<'a>, ModuleError> {
    let own = || {
        let persons = loaded
            .modules
            .iter()
            .position(|g| g.id() == caller.id())
            .map_or(&[][..], |at| &loaded.configured[at].persons[..]);
        Reach::Own {
            persons,
            author: author.map(str::to_string),
        }
    };
    let Some(token) = turn else {
        return Ok(own());
    };
    if ModuleKind::try_from(caller.module().manifest().kind) != Ok(ModuleKind::Agent) {
        return Ok(own());
    }
    let Asked { module, reach } = delegations.asked(token).ok_or_else(|| {
        ModuleError::new(
            ModuleErrorKind::Denied,
            format!("no turn {token} of a module runs on this host"),
        )
    })?;
    let at = loaded
        .modules
        .iter()
        .position(|g| g.id() == module)
        .ok_or_else(|| {
            ModuleError::new(
                ModuleErrorKind::FailedPrecondition,
                format!("module {module}, whose turn it is, is not running"),
            )
        })?;
    let delegator = &loaded.modules[at];
    Ok(Reach::Turn {
        module: delegator.id(),
        uses: &delegator.module().manifest().uses,
        added: &loaded.added,
        grant: &loaded.configured[at].grant,
        workspaces: reach,
    })
}

/// What one call of a module may reach besides the modules it uses.
pub enum Reach<'a> {
    /// Its own: the tools an agent may call, and those only a person calls
    /// when the call's author is a person the module's settings name
    /// (`PersonGrant`).
    Own {
        /// The persons the module acts for.
        persons: &'a [String],
        /// The call's author; none: unnamed.
        author: Option<String>,
    },
    /// The agent's in a module's turn: that module's uses and grant.
    Turn {
        /// The module whose turn it is.
        module: &'a str,
        /// The modules it uses.
        uses: &'a [String],
        /// The modules added in place.
        added: &'a [String],
        /// Its grant.
        grant: &'a TurnGrant,
        /// The workspaces the turn was asked to reach; none: every one.
        workspaces: Option<Vec<String>>,
    },
}

impl Reach<'_> {
    /// The workspaces every call the agent makes in a turn is held to
    /// (docs/connect.md section 4a, a member's reach); none: no turn's, or
    /// one that reaches every workspace.
    pub fn workspaces(&self) -> Option<&[String]> {
        match self {
            Self::Own { .. } => None,
            Self::Turn { workspaces, .. } => workspaces.as_deref(),
        }
    }

    /// Whether a call may reach tool `tool` of module `callee`, a tool an
    /// agent may call (`for_agents`) or one only a person calls.
    pub fn allows(&self, callee: &str, tool: &str, for_agents: bool) -> bool {
        match self {
            Self::Own { persons, author } => {
                for_agents || author.as_ref().is_some_and(|a| persons.contains(a))
            }
            Self::Turn {
                uses, added, grant, ..
            } => reaches(uses, added, callee) && grant.allows(tool, for_agents),
        }
    }

    /// Refuse a call of tool `tool` of module `callee` it does not allow
    /// (`for_agents` as in [`Reach::allows`]).
    ///
    /// # Errors
    /// `Denied`: a tool only a person calls, a module the turn does not
    /// reach, a tool its grant withholds.
    pub fn admit(&self, callee: &str, tool: &str, for_agents: bool) -> Result<(), ModuleError> {
        if self.allows(callee, tool, for_agents) {
            return Ok(());
        }
        let reason = match self {
            Self::Own { .. } => return Err(crate::only_for_a_person(tool)),
            Self::Turn {
                module,
                uses,
                added,
                ..
            } if !reaches(uses, added, callee) => {
                format!(
                    "the turn of module {module} does not reach module {callee}, which offers {tool}"
                )
            }
            Self::Turn { module, grant, .. } if grant.withheld.iter().any(|t| t == tool) => {
                format!("{tool} is withheld from the turns of module {module}")
            }
            Self::Turn { module, .. } => format!(
                "{tool} is only for a person to call; the grant of module {module} does not name it"
            ),
        };
        Err(ModuleError::new(ModuleErrorKind::Denied, reason))
    }
}

#[cfg(test)]
mod tests;
