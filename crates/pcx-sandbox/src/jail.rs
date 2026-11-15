// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! FreeBSD: a process runs through the `pcx` binary's helper mode
//! (`pcx sandbox-run --policy <json> -- program args`), which, as root,
//! runs it in the module's jail and waits for it (docs/host.md, sandbox and
//! tools). A module's jail is its policy's: a root of nullfs mounts (the
//! system paths read-only, devfs with the jail ruleset, each root at its own
//! path, an empty file or directory over each denied name), no IP address
//! without the network grant, the program in its working directory as the
//! unprivileged user the policy names. It is built for the first program
//! and kept while a program runs in it or a process one left lives there
//! (a tmux server), as such a process outlives its program on Linux and
//! macOS; once neither holds, it is removed and its root undone. What a
//! killed helper left, the jail's next helper undoes ([`record`]).

use std::ffi::OsString;
use std::io::Read;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::os::unix::process::ExitStatusExt;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};

use nix::mount::{MntFlags, Nmount};
use nix::sys::signal::{Signal, kill};
use nix::unistd::{Pid, geteuid};
use pcx_module_api::sandbox::Root;

use crate::deny::Deny;
use crate::helper::Wire;
use crate::launcher::Limits;

mod lock;
mod record;

use lock::Locks;
use record::{Record, Step, named};

/// Where the helpers build their jails' roots and keep their records.
const BASE: &str = "/var/run/pcx-sandbox";

/// The empty directory mounted over a denied directory.
const EMPTY_DIR: &str = "empty.d";

/// The empty file mounted over a denied file.
const EMPTY_FILE: &str = "empty";

/// The system paths a program reads to start and run.
const SYSTEM_READ: [&str; 6] = ["/bin", "/sbin", "/lib", "/libexec", "/usr", "/etc"];

/// The run-time linker's search paths (for `/usr/local/lib`).
const HINTS: &str = "/var/run/ld-elf.so.hints";

/// The devfs ruleset of a jail (`devfsrules_jail`: the null, zero and
/// random devices, the standard streams and the pseudo-terminals).
const DEVFS_RULESET: &str = "4";

/// `jail(8)`, `jexec(8)`, `jls(8)`, and `env(1)`, which starts the
/// program in its working directory.
const JAIL: &str = "/usr/sbin/jail";
const JEXEC: &str = "/usr/sbin/jexec";
const JLS: &str = "/usr/sbin/jls";
const PS: &str = "/bin/ps";
const ENV: &str = "/usr/bin/env";
const LIMITS: &str = "/usr/bin/limits";

/// The exit status of a program killed by a signal: 128 and the signal.
const SIGNALLED: i32 = 128;

/// Why the helper could not run a program in a jail.
#[derive(Debug, thiserror::Error)]
pub enum RunError {
    /// No program follows `--`.
    #[error("usage: pcx sandbox-run --policy <json> -- program [args]")]
    Usage,
    /// The policy does not parse.
    #[error("policy: {0}")]
    Policy(#[from] serde_json::Error),
    /// The policy names no user to run the program as.
    #[error("no [sandbox] user: a jail runs a program only as an unprivileged user")]
    NoUser,
    /// The helper is not root, so it can make no jail.
    #[error("the jail helper runs as root")]
    NotRoot,
    /// A root is not an absolute path.
    #[error("{0}: a root of a jail is an absolute path")]
    Relative(PathBuf),
    /// The denied names do not compile.
    #[error("denied names: {0}")]
    Deny(#[from] crate::DenyError),
    /// A file system could not be mounted.
    #[error("mount {at}: {reason}")]
    Mount {
        /// The mount point.
        at: PathBuf,
        /// Why.
        reason: String,
    },
    /// A file or directory of the jail's root, or a record.
    #[error("{0}")]
    Io(#[from] std::io::Error),
    /// The jail could not be made, run or removed.
    #[error("jail: {0}")]
    Jail(String),
}

/// The helper: run `command` (a program and its arguments) in a jail of
/// `policy`, in this process's working directory; its exit status (128
/// and the signal of one killed).
///
/// # Errors
/// See [`RunError`].
pub fn run(policy: &str, command: &[String]) -> Result<i32, RunError> {
    let [program, args @ ..] = command else {
        return Err(RunError::Usage);
    };
    let wire: Wire = serde_json::from_str(policy)?;
    let user = wire.user.clone().ok_or(RunError::NoUser)?;
    if !geteuid().is_root() {
        return Err(RunError::NotRoot);
    }
    let roots = wire.roots();
    if let Some(root) = roots.iter().find(|r| !r.path.is_absolute()) {
        return Err(RunError::Relative(root.path.clone()));
    }
    let deny = Deny::new(&wire.deny)?;
    let cwd = std::env::current_dir()?;
    // The module's scratch directory, which its processes get as TMPDIR,
    // is its /tmp too, for programs that write there by name.
    let tmp = std::env::var_os("TMPDIR")
        .map(PathBuf::from)
        .filter(|dir| roots.iter().any(|r| r.write && r.path == *dir));
    let base = Path::new(BASE);
    prepare(base)?;
    let name = jail_name(policy);
    let root = base.join(&name);
    let locks = Locks::of(base, &name);
    let jail = Jail {
        name: &name,
        root: &root,
        network: wire.network,
        user: &user,
        cwd: &cwd,
        limits: wire.limits.as_ref(),
    };
    let using = {
        let _building = locks.building()?;
        if !there(&name)? {
            let mut record = Record::fresh(base, &name)?;
            let built = build(&mut record, base, &root, &roots, &deny, tmp.as_deref())
                .and_then(|()| jail.make());
            if let Err(failed) = built {
                // Undone now: a launch's jail has a name of its own, so no
                // next helper would come for it.
                drop(record);
                record::undone(base, &name)?;
                locks.remove()?;
                return Err(failed);
            }
        }
        locks.using()?
    };
    let ran = jail.run(program, args);
    drop(using);
    let _building = locks.building()?;
    let removed = match locks.unused()? {
        Some(_unused) if !occupied(&name)? => remove_jail(&name)
            .and_then(|()| record::undone(base, &name))
            .and_then(|()| locks.remove()),
        _ => Ok(()),
    };
    let code = ran?;
    removed?;
    Ok(code)
}

/// The jail of the module whose policy (as the helper reads it) is
/// `policy`: one name per policy, and a module's scratch directory, its
/// last root, makes each module's its own.
fn jail_name(policy: &str) -> String {
    use std::hash::{DefaultHasher, Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    policy.hash(&mut hasher);
    format!("pcx-{:016x}", hasher.finish())
}

/// `base`, only root's, with the empty file and directory mounted over
/// denied names.
fn prepare(base: &Path) -> Result<(), RunError> {
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(base)
        .map_err(|e| named(base, e))?;
    std::fs::set_permissions(base, std::fs::Permissions::from_mode(0o700))
        .map_err(|e| named(base, e))?;
    let dir = base.join(EMPTY_DIR);
    match std::fs::DirBuilder::new().mode(0o500).create(&dir) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(named(&dir, e).into()),
    }
    let file = base.join(EMPTY_FILE);
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o400)
        .open(&file)
    {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(named(&file, e).into()),
    }
    Ok(())
}

/// The jail's root at `root`: the system paths, the run-time linker's
/// hints, devfs, `tmp` (the scratch directory) at /tmp, the roots (outer
/// ones first) and the denied names over them, each step written down
/// before it is taken.
fn build(
    record: &mut Record,
    base: &Path,
    root: &Path,
    roots: &[Root],
    deny: &Deny,
    tmp: Option<&Path>,
) -> Result<(), RunError> {
    make_dirs(record, root)?;
    for system in SYSTEM_READ.iter().map(Path::new).filter(|p| p.exists()) {
        let at = inside(root, system);
        make_dirs(record, &at)?;
        mount_null(record, system, &at, true)?;
    }
    let hints = Path::new(HINTS);
    if hints.exists() {
        let at = inside(root, hints);
        if let Some(parent) = at.parent() {
            make_dirs(record, parent)?;
        }
        record.note(Step::File(at.clone()))?;
        std::fs::File::create(&at).map_err(|e| named(&at, e))?;
        mount_null(record, hints, &at, true)?;
    }
    let dev = root.join("dev");
    make_dirs(record, &dev)?;
    record.note(Step::Mount(dev.clone()))?;
    Nmount::new()
        .str_opt_owned("fstype", "devfs")
        .str_opt_owned("fspath", &dev)
        .str_opt_owned("ruleset", DEVFS_RULESET)
        .nmount(MntFlags::MNT_NOSUID | MntFlags::MNT_NOEXEC)
        .map_err(|e| mount_error(&dev, &e))?;
    if let Some(tmp) = tmp {
        let at = root.join("tmp");
        make_dirs(record, &at)?;
        mount_null(record, tmp, &at, false)?;
    }
    // A root another shows already needs no mount of its own, and nullfs
    // refuses one over its own tree (`EDEADLK`): a read-only one under a
    // system path, one under a root that allows at least as much.
    let mut outer_first: Vec<&Root> = roots
        .iter()
        .filter(|r| r.write || !SYSTEM_READ.iter().any(|system| r.path.starts_with(system)))
        .filter(|r| {
            !roots.iter().any(|outer| {
                outer.path != r.path && r.path.starts_with(&outer.path) && (outer.write || !r.write)
            })
        })
        .collect();
    outer_first.sort_by_key(|r| r.path.components().count());
    for r in &outer_first {
        let at = inside(root, &r.path);
        make_dirs(record, &at)?;
        mount_null(record, &r.path, &at, !r.write)?;
    }
    for r in &outer_first {
        for (path, dir) in deny.found_under(&r.path)? {
            let empty = base.join(if dir { EMPTY_DIR } else { EMPTY_FILE });
            mount_null(record, &empty, &inside(root, &path), true)?;
        }
    }
    Ok(())
}

/// The absolute `path` as it lies under `root`.
fn inside(root: &Path, path: &Path) -> PathBuf {
    root.join(
        path.components()
            .filter(|c| !matches!(c, Component::RootDir))
            .collect::<PathBuf>(),
    )
}

/// The directories of `path` missing, made outer first.
fn make_dirs(record: &mut Record, path: &Path) -> Result<(), RunError> {
    let missing: Vec<&Path> = path.ancestors().take_while(|p| !p.exists()).collect();
    for dir in missing.into_iter().rev() {
        record.note(Step::Dir(dir.to_path_buf()))?;
        std::fs::create_dir(dir).map_err(|e| named(dir, e))?;
    }
    Ok(())
}

/// `target` mounted at `at` with nullfs (read-only when `read_only`),
/// never honouring a set-user-id program.
fn mount_null(
    record: &mut Record,
    target: &Path,
    at: &Path,
    read_only: bool,
) -> Result<(), RunError> {
    record.note(Step::Mount(at.to_path_buf()))?;
    let flags = if read_only {
        MntFlags::MNT_NOSUID | MntFlags::MNT_RDONLY
    } else {
        MntFlags::MNT_NOSUID
    };
    Nmount::new()
        .str_opt_owned("fstype", "nullfs")
        .str_opt_owned("fspath", at)
        .str_opt_owned("target", target)
        .nmount(flags)
        .map_err(|e| mount_error(at, &e))
}

fn mount_error(at: &Path, e: &nix::mount::NmountError) -> RunError {
    RunError::Mount {
        at: at.to_path_buf(),
        reason: match e.errmsg() {
            Some(message) => format!("{message} ({})", e.error()),
            None => e.error().to_string(),
        },
    }
}

/// A module's jail.
struct Jail<'a> {
    name: &'a str,
    root: &'a Path,
    network: bool,
    user: &'a str,
    cwd: &'a Path,
    limits: Option<&'a Limits>,
}

impl Jail<'_> {
    /// Make it over its built root, kept while it has no process.
    fn make(&self) -> Result<(), RunError> {
        let address = if self.network { "inherit" } else { "disable" };
        let mut path = OsString::from("path=");
        path.push(self.root);
        let made = Command::new(JAIL)
            .arg("-c")
            .arg(format!("name={}", self.name))
            .arg(path)
            .arg("host.hostname=pcx")
            .arg(format!("ip4={address}"))
            .arg(format!("ip6={address}"))
            .arg("persist")
            .stdin(Stdio::null())
            .output()
            .map_err(|e| RunError::Jail(format!("{JAIL} did not start: {e}")))?;
        if !made.status.success() {
            return Err(RunError::Jail(format!(
                "{} not made: {}",
                self.name,
                String::from_utf8_lossy(&made.stderr).trim()
            )));
        }
        Ok(())
    }

    /// Run `program args` in it, started by `jexec(8)`, which becomes it, so
    /// its exit status and signal are the program's own.
    fn run(&self, program: &str, args: &[String]) -> Result<i32, RunError> {
        let mut limited = Vec::new();
        if let Some(limits) = self.limits {
            limited.extend([
                LIMITS.to_string(),
                "-n".to_string(),
                limits.open_files.to_string(),
            ]);
            if limits.no_processes {
                // Counted per user, threads aside: every jailed process of
                // this user is limited alike, so none starts another.
                limited.extend(["-u".to_string(), "1".to_string()]);
            }
        }
        let mut command = Command::new(JEXEC);
        command
            .args(["-U", self.user, self.name])
            .args(&limited)
            .args([ENV, "-C"])
            .arg(self.cwd)
            .arg(program)
            .args(args);
        let started = |e: std::io::Error| RunError::Jail(format!("{JEXEC} did not start: {e}"));
        let status = if self.limits.is_some() {
            // A module's process: its host holds this helper's stdin.
            let mut child = command.stdin(Stdio::null()).spawn().map_err(started)?;
            kill_when_stdin_closes(child.id());
            child.wait().map_err(started)?
        } else {
            command.status().map_err(started)?
        };
        Ok(status
            .code()
            .or_else(|| status.signal().map(|signal| SIGNALLED + signal))
            .unwrap_or(SIGNALLED))
    }
}

/// Kill process `pid` once this helper's stdin closes: whatever stops a
/// module's process (a host stopping it, dropping it, or gone) leaves this
/// helper alive to remove its jail, and a killed helper would leave the
/// jail's mounts until its next helper.
fn kill_when_stdin_closes(pid: u32) {
    std::thread::spawn(move || {
        let mut held = [0u8; 16];
        let mut stdin = std::io::stdin();
        while matches!(stdin.read(&mut held), Ok(n) if n > 0) {}
        match i32::try_from(pid) {
            Ok(pid) => {
                if let Err(e) = kill(Pid::from_raw(pid), Signal::SIGKILL) {
                    tracing::debug!(error = %e, "the module's process had ended");
                }
            }
            Err(e) => tracing::warn!(error = %e, pid, "not a process id"),
        }
    });
}

/// Whether jail `name` exists.
fn there(name: &str) -> std::io::Result<bool> {
    Ok(Command::new(JLS)
        .args(["-j", name, "jid"])
        .stdin(Stdio::null())
        .output()
        .map_err(|e| std::io::Error::new(e.kind(), format!("{JLS}: {e}")))?
        .status
        .success())
}

/// Whether a process lives in jail `name` (one a program left there).
fn occupied(name: &str) -> std::io::Result<bool> {
    let listed = Command::new(PS)
        .args(["-x", "-o", "pid=", "-J", name])
        .stdin(Stdio::null())
        .output()
        .map_err(|e| std::io::Error::new(e.kind(), format!("{PS}: {e}")))?;
    Ok(!String::from_utf8_lossy(&listed.stdout).trim().is_empty())
}

/// Remove jail `name` if it is there, with any process left in it.
fn remove_jail(name: &str) -> std::io::Result<()> {
    if !there(name)? {
        return Ok(());
    }
    let removed = Command::new(JAIL)
        .args(["-r", name])
        .stdin(Stdio::null())
        .output()
        .map_err(|e| std::io::Error::new(e.kind(), format!("{JAIL} -r: {e}")))?;
    if removed.status.success() {
        Ok(())
    } else {
        Err(std::io::Error::other(format!(
            "{JAIL} -r {name}: {}",
            String::from_utf8_lossy(&removed.stderr).trim()
        )))
    }
}
