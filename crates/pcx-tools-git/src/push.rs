// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Where and what `tools-git` pushes (`push` of its configuration,
//! docs/connect.md section 7): one remote, the branches whose names match
//! its globs, through an ssh-agent's socket, so the key stays in the agent.

use std::path::{Path, PathBuf};

use globset::{GlobBuilder, GlobSet, GlobSetBuilder};
use pcx_module_api::sandbox::Policy;
use pcx_module_api::{ModuleError, ModuleErrorKind};
use post_cortex_proto::tools_git::GitPush;

/// The remote when the configuration names none.
const REMOTE: &str = "origin";

/// The branches when the configuration names none: a task's.
const BRANCHES: &str = "task/*";

fn invalid(reason: String) -> ModuleError {
    ModuleError::new(ModuleErrorKind::InvalidArgument, reason)
}

fn precondition(reason: String) -> ModuleError {
    ModuleError::new(ModuleErrorKind::FailedPrecondition, reason)
}

/// The push of the configuration.
#[derive(Debug, Clone)]
pub struct Push {
    /// The remote.
    pub remote: String,
    branches: GlobSet,
    patterns: Vec<String>,
    /// The ssh-agent's socket.
    pub ssh_auth_sock: PathBuf,
    /// The known_hosts file; none: a new host's key is accepted.
    pub known_hosts: Option<PathBuf>,
}

impl Push {
    /// The push `config` names; none without one.
    ///
    /// # Errors
    /// `InvalidArgument`: a remote that is an option, a glob that is not
    /// one, a socket or known_hosts that is not an absolute path, no socket.
    pub fn of(config: Option<GitPush>) -> Result<Option<Self>, ModuleError> {
        let Some(config) = config else {
            return Ok(None);
        };
        let remote = if config.remote.is_empty() {
            REMOTE.to_string()
        } else {
            config.remote
        };
        if remote.starts_with('-') {
            return Err(invalid(format!(
                "push.remote names a remote, not an option: {remote:?}"
            )));
        }
        let patterns = if config.branches.is_empty() {
            vec![BRANCHES.to_string()]
        } else {
            config.branches
        };
        let mut set = GlobSetBuilder::new();
        for pattern in &patterns {
            let glob = GlobBuilder::new(pattern)
                .literal_separator(true)
                .build()
                .map_err(|e| invalid(format!("push.branches {pattern:?}: {e}")))?;
            set.add(glob);
        }
        let branches = set
            .build()
            .map_err(|e| invalid(format!("push.branches: {e}")))?;
        let absolute = |what: &str, path: &str| {
            // ssh reads GIT_SSH_COMMAND as words: a path with a space in it
            // would be two.
            let path = PathBuf::from(path);
            if path.is_absolute() && !path.to_string_lossy().contains(char::is_whitespace) {
                Ok(path)
            } else {
                Err(invalid(format!(
                    "push.{what} is an absolute path without spaces: {}",
                    path.display()
                )))
            }
        };
        if config.ssh_auth_sock.is_empty() {
            return Err(invalid(
                "push.ssh_auth_sock names the ssh-agent's socket that holds the key".to_string(),
            ));
        }
        let ssh_auth_sock = absolute("ssh_auth_sock", &config.ssh_auth_sock)?;
        let known_hosts = match config.known_hosts.as_str() {
            "" => None,
            path => Some(absolute("known_hosts", path)?),
        };
        Ok(Some(Self {
            remote,
            branches,
            patterns,
            ssh_auth_sock,
            known_hosts,
        }))
    }

    /// `branch` when this push may send it.
    ///
    /// # Errors
    /// `InvalidArgument` for a branch its globs do not name.
    pub fn allowed<'b>(&self, branch: &'b str) -> Result<&'b str, ModuleError> {
        if self.branches.is_match(branch) {
            Ok(branch)
        } else {
            Err(invalid(format!(
                "tools-git pushes only the branches {} (push.branches), not {branch:?}",
                self.patterns.join(", ")
            )))
        }
    }

    /// Whether `policy` lets a push run: the network, and the socket and the
    /// known_hosts beneath a root.
    ///
    /// # Errors
    /// `FailedPrecondition` naming what is missing.
    pub fn reachable(&self, policy: &Policy) -> Result<(), ModuleError> {
        if !policy.network {
            return Err(precondition(
                "a push needs the network grant of tools-git".to_string(),
            ));
        }
        let beneath = |path: &Path| policy.roots.iter().any(|r| path.starts_with(&r.path));
        for path in std::iter::once(&self.ssh_auth_sock).chain(&self.known_hosts) {
            if !beneath(path) {
                return Err(precondition(format!(
                    "{} is under none of tools-git's roots: add its directory as one",
                    path.display()
                )));
            }
        }
        Ok(())
    }

    /// The environment of the push: the agent's socket, and ssh that never
    /// asks and checks the remote's key (against known_hosts when named;
    /// else a new key is accepted and kept in the scratch `HOME`).
    pub fn env(&self) -> Vec<(String, String)> {
        let check = match &self.known_hosts {
            Some(file) => format!(
                "-o StrictHostKeyChecking=yes -o UserKnownHostsFile={}",
                file.display()
            ),
            None => "-o StrictHostKeyChecking=accept-new".to_string(),
        };
        vec![
            (
                "SSH_AUTH_SOCK".to_string(),
                self.ssh_auth_sock.display().to_string(),
            ),
            (
                "GIT_SSH_COMMAND".to_string(),
                format!("ssh -o BatchMode=yes {check}"),
            ),
        ]
    }
}

#[cfg(test)]
mod tests;
