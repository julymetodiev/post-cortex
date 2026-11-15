// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The command line of the `pcx` binary's helper mode on Linux and FreeBSD
//! (`pcx sandbox-run --policy <json> -- program args`): the policy as the
//! helper reads it.

use std::path::{Path, PathBuf};

use pcx_module_api::sandbox::{Policy, Root};
use serde::{Deserialize, Serialize};

use crate::deny::Deny;
use crate::exec::CommandLine;
use crate::launcher::Limits;

/// The policy as the helper reads it.
#[derive(Debug, Serialize, Deserialize)]
pub struct Wire {
    /// The roots, each with whether it may be written.
    pub roots: Vec<(PathBuf, bool)>,
    /// Whether the program may use the network.
    pub network: bool,
    /// The denied names.
    pub deny: Vec<String>,
    /// The user the program runs as (FreeBSD's jail; none elsewhere).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    /// The limits of a module's own process.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limits: Option<Limits>,
}

impl Wire {
    /// The roots of the policy.
    #[must_use]
    pub fn roots(&self) -> Vec<Root> {
        self.roots
            .iter()
            .map(|(path, write)| Root {
                path: path.clone(),
                write: *write,
            })
            .collect()
    }
}

/// `program args` through the helper `pcx` binary at `helper`, run as
/// `user` where the helper switches users.
///
/// # Errors
/// The policy cannot be encoded.
pub fn command(
    helper: &Path,
    policy: &Policy,
    deny: &Deny,
    user: Option<&str>,
    limits: Option<&Limits>,
    program: &str,
    args: &[String],
) -> Result<CommandLine, serde_json::Error> {
    let wire = Wire {
        roots: policy
            .roots
            .iter()
            .map(|r| (r.path.clone(), r.write))
            .collect(),
        network: policy.network,
        deny: deny.patterns().to_vec(),
        user: user.map(str::to_string),
        limits: limits.copied(),
    };
    let mut wrapped = vec![
        "sandbox-run".to_string(),
        "--policy".to_string(),
        serde_json::to_string(&wire)?,
        "--".to_string(),
        program.to_string(),
    ];
    wrapped.extend(args.iter().cloned());
    Ok(CommandLine {
        program: helper.to_path_buf(),
        args: wrapped,
    })
}
