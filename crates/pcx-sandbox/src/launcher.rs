// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! How this platform confines a process: Seatbelt on macOS, the `pcx`
//! helper with Landlock and seccomp on Linux, the `pcx` helper with a
//! jail on FreeBSD; elsewhere processes are refused.

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
use std::path::PathBuf;

use pcx_module_api::sandbox::Policy;
use pcx_module_api::{ModuleError, ModuleErrorKind};

use crate::deny::Deny;
use crate::exec::CommandLine;
use crate::files::Owner;

/// What limits a module's own process (docs/modules/dynamic.md, limits).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Limits {
    /// Its limit of open files.
    pub open_files: u64,
    /// That it starts no process (threads are not processes).
    pub no_processes: bool,
}

/// The OS sandbox of processes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Launcher {
    /// macOS: `sandbox-exec` with a Seatbelt profile.
    #[cfg(target_os = "macos")]
    Seatbelt,
    /// Linux: the `pcx` binary at `helper` in its `sandbox-run` mode.
    #[cfg(target_os = "linux")]
    Landlock {
        /// The `pcx` binary.
        helper: PathBuf,
    },
    /// FreeBSD: the `pcx` binary at `helper` in its `sandbox-run` mode,
    /// which runs each process in a jail as `user` (`[sandbox] user`);
    /// without one every process is refused.
    #[cfg(target_os = "freebsd")]
    Jail {
        /// The `pcx` binary.
        helper: PathBuf,
        /// The unprivileged user a jailed process runs as.
        user: Option<String>,
    },
    /// No OS sandbox: every process is refused.
    Unsupported,
}

impl Launcher {
    /// This platform's launcher; on Linux and FreeBSD the running binary
    /// is the helper (the `pcx` binary that hosts the sandbox), and on
    /// FreeBSD a process runs as `user` (`[sandbox] user`).
    ///
    /// # Errors
    /// On Linux and FreeBSD, the running binary cannot be found; elsewhere
    /// than FreeBSD, a `user` (no other sandbox switches users).
    pub fn platform(user: Option<String>) -> std::io::Result<Self> {
        #[cfg(target_os = "freebsd")]
        {
            Ok(Self::Jail {
                helper: std::env::current_exe()?,
                user,
            })
        }
        #[cfg(not(target_os = "freebsd"))]
        {
            match user {
                Some(user) => Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!(
                        "[sandbox] user {user}: only FreeBSD's jail runs a process as another user"
                    ),
                )),
                None => Self::here(),
            }
        }
    }

    /// The launcher of macOS, Linux or elsewhere.
    #[cfg(not(target_os = "freebsd"))]
    fn here() -> std::io::Result<Self> {
        #[cfg(target_os = "macos")]
        {
            Ok(Self::Seatbelt)
        }
        #[cfg(target_os = "linux")]
        {
            Ok(Self::Landlock {
                helper: std::env::current_exe()?,
            })
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        {
            Ok(Self::Unsupported)
        }
    }

    /// Give the scratch directory `dir` to whoever its processes run as:
    /// on FreeBSD the jail's user (the helper runs as root); elsewhere they
    /// run as this process, which made it.
    ///
    /// # Errors
    /// On FreeBSD, the user is unknown or `dir` cannot change owner.
    #[cfg(target_os = "freebsd")]
    pub fn give(&self, dir: &std::path::Path) -> std::io::Result<()> {
        match self.owner()? {
            Some((uid, gid)) => std::os::unix::fs::chown(dir, Some(uid), Some(gid)),
            None => Ok(()),
        }
    }

    /// Who this launcher's processes run as when that is not this process:
    /// the file tools write as them, so what either makes the other can
    /// change.
    ///
    /// # Errors
    /// The `[sandbox] user` does not exist, or could not be looked up.
    #[cfg(target_os = "freebsd")]
    pub fn owner(&self) -> std::io::Result<Option<Owner>> {
        let Self::Jail {
            user: Some(user), ..
        } = self
        else {
            return Ok(None);
        };
        let found = nix::unistd::User::from_name(user)?.ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("[sandbox] user {user} does not exist"),
            )
        })?;
        Ok(Some((found.uid.as_raw(), found.gid.as_raw())))
    }

    /// Give the scratch directory `dir` to whoever its processes run as:
    /// here they run as this process, which made it.
    ///
    /// # Errors
    /// None here.
    #[cfg(not(target_os = "freebsd"))]
    pub fn give(&self, _dir: &std::path::Path) -> std::io::Result<()> {
        Ok(())
    }

    /// Who this launcher's processes run as when that is not this process:
    /// here they run as this process.
    ///
    /// # Errors
    /// None here.
    #[cfg(not(target_os = "freebsd"))]
    pub fn owner(&self) -> std::io::Result<Option<Owner>> {
        Ok(None)
    }

    /// `program args` inside the OS sandbox of `policy`, the names of
    /// `deny` out of its reach.
    ///
    /// # Errors
    /// As [`Launcher::confine_module`].
    pub fn confine(
        &self,
        policy: &Policy,
        deny: &Deny,
        program: &str,
        args: &[String],
    ) -> Result<CommandLine, ModuleError> {
        self.confined(policy, deny, None, program, args)
    }

    /// A module's own process, `program args`, inside the OS sandbox of
    /// `policy` under `limits`.
    ///
    /// # Errors
    /// `FailedPrecondition` without an OS sandbox (or, on FreeBSD, without
    /// a user to run as); `Internal` when the policy cannot be handed to
    /// the helper.
    pub fn confine_module(
        &self,
        policy: &Policy,
        deny: &Deny,
        limits: &Limits,
        program: &str,
        args: &[String],
    ) -> Result<CommandLine, ModuleError> {
        self.confined(policy, deny, Some(limits), program, args)
    }

    fn confined(
        &self,
        policy: &Policy,
        deny: &Deny,
        limits: Option<&Limits>,
        program: &str,
        args: &[String],
    ) -> Result<CommandLine, ModuleError> {
        match self {
            #[cfg(target_os = "macos")]
            Self::Seatbelt => Ok(crate::seatbelt::command(
                policy, deny, limits, program, args,
            )),
            #[cfg(target_os = "linux")]
            Self::Landlock { helper } => {
                crate::helper::command(helper, policy, deny, None, limits, program, args).map_err(
                    |e| ModuleError::new(ModuleErrorKind::Internal, format!("sandbox policy: {e}")),
                )
            }
            #[cfg(target_os = "freebsd")]
            Self::Jail {
                helper,
                user: Some(user),
            } => crate::helper::command(helper, policy, deny, Some(user), limits, program, args)
                .map_err(|e| {
                    ModuleError::new(ModuleErrorKind::Internal, format!("sandbox policy: {e}"))
                }),
            #[cfg(target_os = "freebsd")]
            Self::Jail { user: None, .. } => Err(ModuleError::new(
                ModuleErrorKind::FailedPrecondition,
                format!(
                    "no [sandbox] user: a FreeBSD jail runs a process only as an unprivileged \
                     user; {program} is refused"
                ),
            )),
            Self::Unsupported => Err(ModuleError::new(
                ModuleErrorKind::FailedPrecondition,
                format!(
                    "no OS sandbox here: {program} is refused ({} roots, {} denied names, limits {limits:?})",
                    policy.roots.len(),
                    deny.patterns().len()
                ),
            )),
        }
    }
}
