// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Linux: a process runs through the `pcx` binary's helper mode
//! (`pcx sandbox-run --policy <json> -- program args`), which applies
//! Landlock (read the system paths and the roots, write the roots that
//! allow it and the terminal devices; TCP only with the network grant) and seccomp (no IPv4 or IPv6
//! socket without the network grant) to itself, then runs the program. The
//! helper refuses to run anything when the kernel cannot enforce Landlock.
//! Landlock only grants, so a denied name under a root is hidden first: in
//! a new user and mount namespace an empty file or directory is mounted
//! over each one found (docs/connect.md section 7); where the namespace
//! cannot be made, nothing runs.

use std::convert::Infallible;
use std::os::fd::AsRawFd;
use std::os::unix::process::CommandExt;
use std::path::Path;

use landlock::{
    ABI, Access, AccessFs, AccessNet, PathBeneath, PathFd, Ruleset, RulesetAttr,
    RulesetCreatedAttr, RulesetStatus,
};
use nix::mount::{MsFlags, mount};
use nix::sched::{CloneFlags, unshare};
use nix::sys::resource::{Resource, setrlimit};
use pcx_module_api::sandbox::Root;
use seccompiler::{
    BpfProgram, SeccompAction, SeccompCmpArgLen, SeccompCmpOp, SeccompCondition, SeccompFilter,
    SeccompRule, TargetArch,
};

use crate::deny::Deny;
use crate::helper::Wire;

/// The system paths a program reads to start and run.
const SYSTEM_READ: [&str; 8] = [
    "/usr", "/bin", "/sbin", "/lib", "/lib64", "/etc", "/dev", "/proc",
];

/// The resolver's configuration, read where its link leads when the
/// network is granted.
const RESOLV_CONF: &str = "/etc/resolv.conf";

/// The devices a program writes and opens terminals on (tmux's panes).
const DEVICES: [&str; 5] = [
    "/dev/null",
    "/dev/zero",
    "/dev/tty",
    "/dev/ptmx",
    "/dev/pts",
];

/// The Landlock version the rules are written for; an older kernel
/// enforces what it knows (the helper refuses one that enforces nothing).
const LANDLOCK: ABI = ABI::V5;

/// Why the helper could not confine and run a program.
#[derive(Debug, thiserror::Error)]
pub enum RunError {
    /// No program follows `--`.
    #[error("usage: pcx sandbox-run --policy <json> -- program [args]")]
    Usage,
    /// The policy does not parse.
    #[error("policy: {0}")]
    Policy(#[from] serde_json::Error),
    /// Landlock could not be set up, or the kernel does not enforce it.
    #[error("landlock: {0}")]
    Landlock(String),
    /// The seccomp filter could not be built or applied.
    #[error("seccomp: {0}")]
    Seccomp(String),
    /// A limit could not be set.
    #[error("limit: {0}")]
    Limit(String),
    /// The denied names could not be hidden (no user namespace here, or a
    /// mount failed).
    #[error("hiding the denied names: {0}")]
    Hide(String),
    /// The policy names a user to run as: only FreeBSD's jail switches
    /// users.
    #[error("the policy runs the program as {0}; Linux runs it as this user")]
    User(String),
    /// The program could not be run.
    #[error("{program}: {source}")]
    Exec {
        /// The program.
        program: String,
        /// Why.
        source: std::io::Error,
    },
}

/// The helper (`pcx sandbox-run --policy <json> -- program args`): confine
/// this process by `policy`, then become `command`'s program with its
/// arguments. Returns only when it could not.
///
/// # Errors
/// See [`RunError`].
pub fn run(policy: &str, command: &[String]) -> Result<Infallible, RunError> {
    let [program, rest @ ..] = command else {
        return Err(RunError::Usage);
    };
    let wire: Wire = serde_json::from_str(policy)?;
    if let Some(user) = &wire.user {
        return Err(RunError::User(user.clone()));
    }
    let roots = wire.roots();
    let deny = Deny::new(&wire.deny).map_err(|e| RunError::Hide(e.to_string()))?;
    // The module's scratch directory, which its processes get as TMPDIR,
    // is its /tmp too, for programs that write there by name.
    let tmp = std::env::var_os("TMPDIR")
        .map(std::path::PathBuf::from)
        .filter(|dir| roots.iter().any(|r| r.write && r.path == *dir));
    if !deny.is_empty() {
        hide(&roots, &deny, tmp.as_deref())?;
    } else if let Some(tmp) = &tmp {
        // Without a name to hide, a host that allows no user namespace
        // still runs the program, with TMPDIR and the system's /tmp.
        if hide(&roots, &deny, Some(tmp)).is_err() {
            tracing::debug!("no mount namespace: /tmp stays the system's");
        }
    }
    landlock(&roots, wire.network)?;
    if !wire.network {
        no_inet_sockets()?;
    }
    if let Some(limits) = &wire.limits {
        setrlimit(
            Resource::RLIMIT_NOFILE,
            limits.open_files,
            limits.open_files,
        )
        .map_err(|e| RunError::Limit(e.to_string()))?;
        if limits.no_processes {
            no_processes()?;
        }
    }
    let error = std::process::Command::new(program).args(rest).exec();
    Err(RunError::Exec {
        program: program.clone(),
        source: error,
    })
}

fn hide_error(e: impl std::fmt::Display) -> RunError {
    RunError::Hide(e.to_string())
}

/// In a new user namespace (this process's own ids mapped to themselves)
/// and mount namespace, mount `tmp` (the scratch directory) over /tmp and
/// over each denied name under `roots`: an empty read-only directory over
/// a directory, `/dev/null` over anything else.
fn hide(roots: &[Root], deny: &Deny, tmp: Option<&Path>) -> Result<(), RunError> {
    let cwd = std::env::current_dir().map_err(hide_error)?;
    let (uid, gid) = (nix::unistd::getuid(), nix::unistd::getgid());
    unshare(CloneFlags::CLONE_NEWUSER | CloneFlags::CLONE_NEWNS).map_err(hide_error)?;
    std::fs::write("/proc/self/setgroups", "deny").map_err(hide_error)?;
    std::fs::write("/proc/self/uid_map", format!("{uid} {uid} 1")).map_err(hide_error)?;
    std::fs::write("/proc/self/gid_map", format!("{gid} {gid} 1")).map_err(hide_error)?;
    // What is mounted here stays here.
    mount(
        None::<&str>,
        "/",
        None::<&str>,
        MsFlags::MS_REC | MsFlags::MS_PRIVATE,
        None::<&str>,
    )
    .map_err(hide_error)?;
    if let Some(tmp) = tmp {
        over_tmp(roots, tmp)?;
    }
    let mut found = Vec::new();
    for root in roots {
        found.extend(deny.found_under(&root.path).map_err(hide_error)?);
    }
    for (path, dir) in found {
        let mounted = if dir {
            mount(
                Some("tmpfs"),
                &path,
                Some("tmpfs"),
                MsFlags::MS_RDONLY | MsFlags::MS_NOSUID | MsFlags::MS_NODEV | MsFlags::MS_NOEXEC,
                Some("size=0,mode=0500"),
            )
        } else {
            mount(
                Some("/dev/null"),
                &path,
                None::<&str>,
                MsFlags::MS_BIND,
                None::<&str>,
            )
        };
        mounted.map_err(|e| RunError::Hide(format!("{}: {e}", path.display())))?;
    }
    // The working directory looked up again, through what is mounted now:
    // the one it was opened as lies beneath none of it.
    std::env::set_current_dir(&cwd)
        .map_err(|e| RunError::Hide(format!("{}: {e}", cwd.display())))?;
    Ok(())
}

/// Mount `tmp` over /tmp, then each root beneath /tmp back at its own path
/// (opened before, so the mount does not hide it), read-only unless it
/// allows writes: beneath the writable /tmp, Landlock alone would let it be
/// written. The FreeBSD jail mounts its roots after its /tmp the same way.
fn over_tmp(roots: &[Root], tmp: &Path) -> Result<(), RunError> {
    let beneath: Vec<(&Root, std::fs::File)> = roots
        .iter()
        .filter(|r| r.path.starts_with("/tmp"))
        .map(|r| std::fs::File::open(&r.path).map(|opened| (r, opened)))
        .collect::<Result<_, _>>()
        .map_err(hide_error)?;
    bind(tmp, Path::new("/tmp"))?;
    for (root, opened) in beneath {
        let path = root.path.as_path();
        std::fs::create_dir_all(path)
            .map_err(|e| RunError::Hide(format!("{}: {e}", path.display())))?;
        let source = format!("/proc/self/fd/{}", opened.as_raw_fd());
        bind(Path::new(&source), path)?;
        if !root.write {
            mount(
                None::<&str>,
                path,
                None::<&str>,
                MsFlags::MS_BIND | MsFlags::MS_REMOUNT | MsFlags::MS_RDONLY,
                None::<&str>,
            )
            .map_err(|e| RunError::Hide(format!("{}: {e}", path.display())))?;
        }
    }
    Ok(())
}

/// `source` bind-mounted at `at`, with what is mounted beneath it.
fn bind(source: &Path, at: &Path) -> Result<(), RunError> {
    mount(
        Some(source),
        at,
        None::<&str>,
        MsFlags::MS_BIND | MsFlags::MS_REC,
        None::<&str>,
    )
    .map_err(|e| RunError::Hide(format!("{}: {e}", at.display())))
}

fn landlock_error(e: impl std::fmt::Display) -> RunError {
    RunError::Landlock(e.to_string())
}

/// Read the system paths and the roots, write the roots that allow it;
/// TCP bind and connect only with `network`.
fn landlock(roots: &[Root], network: bool) -> Result<(), RunError> {
    let mut ruleset = Ruleset::default()
        .handle_access(AccessFs::from_all(LANDLOCK))
        .map_err(landlock_error)?;
    if !network {
        ruleset = ruleset
            .handle_access(AccessNet::from_all(LANDLOCK))
            .map_err(landlock_error)?;
    }
    let mut created = ruleset.create().map_err(landlock_error)?;
    let read = AccessFs::from_read(LANDLOCK);
    // With the network, the resolver's configuration where it really is
    // (`/etc/resolv.conf` is often a link into `/run`), so names resolve.
    let resolver = match (network, std::fs::canonicalize(RESOLV_CONF)) {
        (true, Ok(conf)) => conf.parent().map(Path::to_path_buf),
        // No network, or no resolver configured: nothing more to read.
        (false, _) | (true, Err(_)) => None,
    };
    for path in SYSTEM_READ
        .iter()
        .map(Path::new)
        .chain(resolver.as_deref())
        .filter(|p| p.exists())
    {
        let fd = PathFd::new(path).map_err(landlock_error)?;
        created = created
            .add_rule(PathBeneath::new(fd, read))
            .map_err(landlock_error)?;
    }
    let device = AccessFs::ReadFile | AccessFs::WriteFile | AccessFs::IoctlDev;
    for path in DEVICES.iter().map(Path::new).filter(|p| p.exists()) {
        let fd = PathFd::new(path).map_err(landlock_error)?;
        created = created
            .add_rule(PathBeneath::new(fd, device))
            .map_err(landlock_error)?;
    }
    for root in roots {
        let access = if root.write {
            AccessFs::from_all(LANDLOCK)
        } else {
            read
        };
        let fd = PathFd::new(&root.path).map_err(landlock_error)?;
        created = created
            .add_rule(PathBeneath::new(fd, access))
            .map_err(landlock_error)?;
    }
    let status = created.restrict_self().map_err(landlock_error)?;
    if status.ruleset == RulesetStatus::NotEnforced {
        return Err(RunError::Landlock(
            "the kernel does not enforce Landlock; nothing runs unconfined".to_string(),
        ));
    }
    Ok(())
}

/// Refuse every IPv4 and IPv6 socket (`EPERM`); other calls pass.
fn no_inet_sockets() -> Result<(), RunError> {
    let seccomp = |e: &dyn std::fmt::Display| RunError::Seccomp(e.to_string());
    let family = |af: i32| {
        SeccompCondition::new(
            0,
            SeccompCmpArgLen::Dword,
            SeccompCmpOp::Eq,
            u64::from(af.unsigned_abs()),
        )
        .and_then(|c| SeccompRule::new(vec![c]))
    };
    let rules = vec![
        family(libc::AF_INET).map_err(|e| seccomp(&e))?,
        family(libc::AF_INET6).map_err(|e| seccomp(&e))?,
    ];
    let arch = TargetArch::try_from(std::env::consts::ARCH).map_err(|e| seccomp(&e))?;
    let filter = SeccompFilter::new(
        std::iter::once((libc::SYS_socket, rules)).collect(),
        SeccompAction::Allow,
        SeccompAction::Errno(libc::EPERM.unsigned_abs()),
        arch,
    )
    .map_err(|e| seccomp(&e))?;
    let program: BpfProgram = filter
        .try_into()
        .map_err(|e: seccompiler::BackendError| seccomp(&e))?;
    seccompiler::apply_filter(&program).map_err(|e| seccomp(&e))
}

/// Refuse every new process (`EPERM`); threads pass (`clone` with
/// `CLONE_THREAD`). `clone3` hides its flags from a filter, so it answers
/// `ENOSYS`, on which the C library falls back to `clone`.
fn no_processes() -> Result<(), RunError> {
    let seccomp = |e: &dyn std::fmt::Display| RunError::Seccomp(e.to_string());
    let arch = TargetArch::try_from(std::env::consts::ARCH).map_err(|e| seccomp(&e))?;
    let not_a_thread = SeccompCondition::new(
        0,
        SeccompCmpArgLen::Qword,
        SeccompCmpOp::MaskedEq(u64::from(libc::CLONE_THREAD.unsigned_abs())),
        0,
    )
    .and_then(|c| SeccompRule::new(vec![c]))
    .map_err(|e| seccomp(&e))?;
    let mut refused = std::collections::BTreeMap::new();
    refused.insert(libc::SYS_clone, vec![not_a_thread]);
    #[cfg(target_arch = "x86_64")]
    {
        refused.insert(libc::SYS_fork, Vec::new());
        refused.insert(libc::SYS_vfork, Vec::new());
    }
    let unknown = std::iter::once((libc::SYS_clone3, Vec::new())).collect();
    for (rules, answer) in [(refused, libc::EPERM), (unknown, libc::ENOSYS)] {
        let filter = SeccompFilter::new(
            rules,
            SeccompAction::Allow,
            SeccompAction::Errno(answer.unsigned_abs()),
            arch,
        )
        .map_err(|e| seccomp(&e))?;
        let program: BpfProgram = filter
            .try_into()
            .map_err(|e: seccompiler::BackendError| seccomp(&e))?;
        seccompiler::apply_filter(&program).map_err(|e| seccomp(&e))?;
    }
    Ok(())
}
