// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! A scope's workspaces: the one a call works in, and the ones it may
//! reach (`docs/connect.md` section 4a).

use crate::scope::Scope;
use crate::{ModuleError, ModuleErrorKind};

/// The reach of a call that carries `own` (none: every workspace) and is
/// held to `held` as well: the workspaces both name (`docs/connect.md`
/// section 4a, a member's reach).
#[must_use]
pub fn narrower(own: Option<&[String]>, held: &[String]) -> Vec<String> {
    match own {
        None => held.to_vec(),
        Some(own) => own.iter().filter(|w| held.contains(w)).cloned().collect(),
    }
}

impl Scope {
    /// This scope held to `reach` as well: it reaches the workspaces both
    /// name ([`narrower`]).
    #[must_use]
    pub fn held_to(&self, reach: &[String]) -> Self {
        Self {
            reach: Some(narrower(self.reach.as_deref(), reach)),
            ..self.clone()
        }
    }

    /// The workspace a call works in: the scope's workspace, else the
    /// one the request names (`named`; empty: none).
    ///
    /// # Errors
    /// `InvalidArgument` for a call that names no workspace.
    pub fn workspace(&self, named: &str) -> Result<String, ModuleError> {
        match (&self.workspace_id, named) {
            (Some(workspace), _) => Ok(workspace.clone()),
            (None, "") => Err(ModuleError::new(
                ModuleErrorKind::InvalidArgument,
                "the call names no workspace: give workspace_id",
            )),
            (None, named) => Ok(named.to_string()),
        }
    }

    /// The scope of the calls a call makes working in `workspace` (the one
    /// [`Scope::workspace`] gave it): this one, naming that workspace, which
    /// the caller may have named only in its request. A runner's tools take
    /// only a call that names the workspace the runner serves
    /// (`docs/connect.md` section 7), and the host checks it against the
    /// reach.
    #[must_use]
    pub fn working_in(&self, workspace: &str) -> Self {
        Self {
            workspace_id: Some(workspace.to_string()),
            ..self.clone()
        }
    }

    /// Whether the call may reach `workspace`: one of its reach, any when it
    /// carries none.
    #[must_use]
    pub fn reaches(&self, workspace: &str) -> bool {
        self.reach
            .as_ref()
            .is_none_or(|reach| reach.iter().any(|w| w == workspace))
    }

    /// The scope of what a module does for this call in the store its
    /// settings name, the organisation's (a module's data and the reach):
    /// this one without its reach, which holds the caller to their
    /// workspaces' memory, not the module to its own.
    #[must_use]
    pub fn in_own_store(&self) -> Self {
        Self {
            reach: None,
            ..self.clone()
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use tokio_util::sync::CancellationToken;

    use crate::scope::Scope;

    #[test]
    fn a_reach_holds_a_call_to_its_workspaces_and_none_reaches_every_one() {
        let mut scope = Scope::own("m", "w1", Duration::from_secs(1), CancellationToken::new());
        assert!(scope.reaches("w1") && scope.reaches("w2"));
        scope.reach = Some(vec!["w1".to_string()]);
        assert!(scope.reaches("w1"));
        assert!(!scope.reaches("w2"));
        // In the module's own store the caller's reach does not hold.
        let own = scope.in_own_store();
        assert!(own.reaches("w2"));
        assert_eq!(own.author, scope.author);
        assert_eq!(own.workspace_id, scope.workspace_id);
    }

    #[test]
    fn a_scope_held_to_a_reach_reaches_what_both_name() {
        let names = |list: &[&str]| list.iter().map(|w| (*w).to_string()).collect::<Vec<_>>();
        let mut scope = Scope::own("m", "w1", Duration::from_secs(1), CancellationToken::new());
        // It carried none: the reach it is held to.
        assert_eq!(scope.held_to(&names(&["w1"])).reach, Some(names(&["w1"])));
        scope.reach = Some(names(&["w1", "w2"]));
        assert_eq!(
            scope.held_to(&names(&["w2", "w3"])).reach,
            Some(names(&["w2"]))
        );
        // Nothing in both: no workspace, never every one.
        let none = scope.held_to(&names(&["w3"]));
        assert_eq!(none.reach, Some(Vec::new()));
        assert!(!none.reaches("w1") && !none.reaches("w3"));
    }
}
