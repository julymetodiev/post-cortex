// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! One module's sandbox: its policy, its files beneath its roots, its
//! processes under the platform's OS sandbox.

use std::path::PathBuf;

use async_trait::async_trait;
use pcx_module_api::sandbox::{
    DirEntry, ExecRun, ExecSpec, Metadata, Policy, Root, Sandbox, SandboxPath,
};
use pcx_module_api::{ModuleError, ModuleErrorKind};
use tokio_util::sync::CancellationToken;

use crate::deny::Deny;
use crate::exec::{self, CommandLine};
use crate::files::{self, Owner};
use crate::launcher::Launcher;

/// The variables a process gets pointing at its scratch directory, over
/// whatever the call names.
const SCRATCH_VARIABLES: [&str; 3] = ["TMPDIR", "HOME", "TMUX_TMPDIR"];

/// The sandbox of one policy.
pub struct Bound {
    /// The policy, with the scratch directory as its last root when there
    /// is one.
    policy: Policy,
    /// The policy's denied names, compiled.
    deny: Deny,
    scratch: Option<PathBuf>,
    launcher: Launcher,
    /// Whom what the file tools make belongs to, when not this process.
    owner: Option<Owner>,
}

impl Bound {
    /// The sandbox of `policy` (and its `scratch` directory, writable),
    /// confining processes with `launcher`.
    ///
    /// # Errors
    /// `InvalidArgument` when the policy's denied names are not globs of a
    /// name; `FailedPrecondition` when the user its processes run as cannot
    /// be found.
    pub fn new(
        mut policy: Policy,
        scratch: Option<PathBuf>,
        launcher: Launcher,
    ) -> Result<Self, ModuleError> {
        let deny = Deny::new(&policy.deny).map_err(|e| {
            ModuleError::new(
                ModuleErrorKind::InvalidArgument,
                format!("denied names: {e}"),
            )
        })?;
        let owner = launcher.owner().map_err(|e| {
            ModuleError::new(
                ModuleErrorKind::FailedPrecondition,
                format!("the sandbox's user: {e}"),
            )
        })?;
        if let Some(dir) = &scratch {
            policy.roots.push(Root {
                path: dir.clone(),
                write: true,
            });
        }
        Ok(Self {
            policy,
            deny,
            scratch,
            launcher,
            owner,
        })
    }

    /// Run `files_op` on a blocking thread over `path`'s place beneath its
    /// root.
    async fn blocking<T, F>(&self, path: &SandboxPath, files_op: F) -> Result<T, ModuleError>
    where
        T: Send + 'static,
        F: FnOnce(&files::Beneath<'_>) -> Result<T, ModuleError> + Send + 'static,
    {
        let (policy, deny) = (self.policy.clone(), self.deny.clone());
        let path = path.clone();
        tokio::task::spawn_blocking(move || files_op(&files::beneath(&policy, &deny, &path)?))
            .await
            .map_err(|e| ModuleError::new(ModuleErrorKind::Internal, format!("file task: {e}")))?
    }
}

#[async_trait]
impl Sandbox for Bound {
    fn policy(&self) -> &Policy {
        &self.policy
    }

    async fn read(&self, file: &SandboxPath, limit: usize) -> Result<Vec<u8>, ModuleError> {
        self.blocking(file, move |at| files::read(at, limit)).await
    }

    async fn write(&self, file: &SandboxPath, bytes: Vec<u8>) -> Result<(), ModuleError> {
        let owner = self.owner;
        self.blocking(file, move |at| files::write(at, &bytes, owner))
            .await
    }

    async fn list(&self, dir: &SandboxPath) -> Result<Vec<DirEntry>, ModuleError> {
        self.blocking(dir, files::list).await
    }

    async fn metadata(&self, path: &SandboxPath) -> Result<Metadata, ModuleError> {
        self.blocking(path, files::metadata).await
    }

    async fn remove(&self, path: &SandboxPath) -> Result<(), ModuleError> {
        self.blocking(path, files::remove).await
    }

    async fn rename(&self, from: &SandboxPath, to: &SandboxPath) -> Result<(), ModuleError> {
        let (policy, deny, to) = (self.policy.clone(), self.deny.clone(), to.clone());
        self.blocking(from, move |source| {
            files::rename(source, &files::beneath(&policy, &deny, &to)?)
        })
        .await
    }

    async fn exec(
        &self,
        mut spec: ExecSpec,
        cancel: CancellationToken,
    ) -> Result<ExecRun, ModuleError> {
        let Some(scratch) = self.scratch.as_ref().filter(|_| self.policy.spawn) else {
            return Err(ModuleError::new(
                ModuleErrorKind::Denied,
                "the module may not start processes",
            ));
        };
        let scratch = scratch.to_string_lossy().into_owned();
        spec.env
            .retain(|(name, _)| !SCRATCH_VARIABLES.contains(&name.as_str()));
        spec.env.extend(
            SCRATCH_VARIABLES
                .iter()
                .map(|name| ((*name).to_string(), scratch.clone())),
        );
        let cwd = spec.cwd.clone().unwrap_or_else(|| SandboxPath::new(""));
        let dir = self.blocking(&cwd, files::working_dir).await?;
        let command: CommandLine =
            self.launcher
                .confine(&self.policy, &self.deny, &spec.program, &spec.args)?;
        exec::start(spec, command, dir, cancel)
    }
}
