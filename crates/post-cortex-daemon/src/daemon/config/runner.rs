// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! What the daemon lends a hosted tenant as a runner (`[runner]`,
//! docs/connect.md section 7): its tool modules, and the names under every
//! module's roots none of them reaches, so a credential does not reach a
//! model.

use serde::{Deserialize, Serialize};

use crate::compose::{Builtin, BuiltinSpec};

/// The names a runner denies when `[runner]` names none: environment
/// files, keys and the directories and files of credentials.
pub const DENY: [&str; 13] = [
    ".env",
    ".env.*",
    "*.pem",
    "*.key",
    "id_rsa*",
    "id_ed25519*",
    ".ssh",
    ".kube",
    "kubeconfig*",
    ".netrc",
    ".aws",
    ".git-credentials",
    ".npmrc",
];

/// `[runner]` of `daemon.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunnerConfig {
    /// The tool modules it lends (`tools-fs`, `tools-shell`, `tools-git`),
    /// each also a `[[builtins]]` entry with its grants and roots.
    pub modules: Vec<Builtin>,
    /// The names (globs) of the files and directories no module of the
    /// daemon reaches (default: `DENY`).
    #[serde(default = "default_deny")]
    pub deny: Vec<String>,
}

fn default_deny() -> Vec<String> {
    DENY.iter().map(|name| (*name).to_string()).collect()
}

impl RunnerConfig {
    /// Whether it lends only tool modules the daemon runs (`builtins`) and
    /// its names are globs.
    ///
    /// # Errors
    /// What is wrong.
    pub fn validate(&self, builtins: &[BuiltinSpec]) -> Result<(), String> {
        for module in &self.modules {
            if !matches!(
                module,
                Builtin::ToolsFs | Builtin::ToolsShell | Builtin::ToolsGit
            ) {
                return Err(format!(
                    "[runner] lends {}, which is no tool module",
                    module.id()
                ));
            }
            if !builtins.iter().any(|b| b.id == *module) {
                return Err(format!(
                    "[runner] lends {}, which no [[builtins]] entry runs",
                    module.id()
                ));
            }
        }
        pcx_sandbox::Deny::new(&self.deny)
            .map(|_| ())
            .map_err(|e| format!("[runner] deny: {e}"))
    }
}
