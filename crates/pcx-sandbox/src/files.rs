// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! A module's files: every path resolves beneath one of its roots through
//! `cap-std`, component by component, so `..` above the root, an absolute
//! path elsewhere and a symbolic link pointing out are refused, with no
//! window between a check and the open. A denied name ([`Deny`]) is
//! refused as the call names the path and again where what it opened is,
//! so a link to one is seen through.

use std::io::{ErrorKind, Read, Write};
use std::path::{Component, Path, PathBuf};

use std::os::fd::AsFd;

use cap_std::ambient_authority;
use cap_std::fs::Dir;
use pcx_module_api::sandbox::{DirEntry, EntryKind, Metadata, Policy, Root, SandboxPath};
use pcx_module_api::{ModuleError, ModuleErrorKind};

use crate::deny::Deny;

/// A path of a call, beneath one root: the root, the path under it, and
/// the names the module never reaches.
pub struct Beneath<'a> {
    /// The root.
    pub root: &'a Root,
    /// The path relative to it (empty: the root itself).
    pub rel: PathBuf,
    /// The denied names.
    pub deny: &'a Deny,
}

impl Beneath<'_> {
    /// Refused when the open `handle` is at a denied name.
    fn reached(&self, handle: impl AsFd) -> Result<(), ModuleError> {
        match self.deny.refused_at(&self.root.path, handle) {
            Ok(None) => Ok(()),
            Ok(Some(name)) => Err(refused_name(&self.rel, &name)),
            Err(e) => Err(io_error(&self.rel, &e)),
        }
    }

    /// Refused when the directory `path` (under `dir`, the root's) holds
    /// is at a denied name; the root itself is not checked.
    fn reached_dir(&self, dir: &Dir, path: &Path) -> Result<(), ModuleError> {
        if self.deny.is_empty() || path.as_os_str().is_empty() || path == Path::new(".") {
            return Ok(());
        }
        let opened = dir.open_dir(path).map_err(|e| io_error(path, &e))?;
        self.reached(&opened)
    }
}

/// The refusal of `path`, whose component `name` is denied.
fn refused_name(path: &Path, name: &str) -> ModuleError {
    denied(format!(
        "{} is out of reach: {name} is a denied name",
        path.display()
    ))
}

fn denied(reason: String) -> ModuleError {
    ModuleError::new(ModuleErrorKind::Denied, reason)
}

/// Where `path` lies: beneath the root it names (or the first) when it is
/// relative, beneath the root it starts with when it is absolute.
///
/// # Errors
/// `Denied` for a policy without roots, a named root that is not one, an
/// absolute path under no root, or a path with a denied name.
pub fn beneath<'a>(
    policy: &'a Policy,
    deny: &'a Deny,
    path: &SandboxPath,
) -> Result<Beneath<'a>, ModuleError> {
    let at = place(policy, deny, path)?;
    match deny.refused(&at.rel) {
        Some(name) => Err(refused_name(&at.rel, &name)),
        None => Ok(at),
    }
}

/// Where `path` lies, before its names are checked.
fn place<'a>(
    policy: &'a Policy,
    deny: &'a Deny,
    path: &SandboxPath,
) -> Result<Beneath<'a>, ModuleError> {
    let Some(first) = policy.roots.first() else {
        return Err(denied("the module has no filesystem roots".to_string()));
    };
    if path.path.is_absolute() {
        let found = policy
            .roots
            .iter()
            .filter(|root| path.root.as_ref().is_none_or(|named| *named == root.path))
            .find_map(|root| {
                let rel = path.path.strip_prefix(&root.path).ok()?;
                Some(Beneath {
                    root,
                    rel: rel.to_path_buf(),
                    deny,
                })
            });
        return found.ok_or_else(|| {
            denied(format!(
                "{} is under none of the roots",
                path.path.display()
            ))
        });
    }
    let root = match &path.root {
        None => first,
        Some(named) => policy
            .roots
            .iter()
            .find(|root| root.path == *named)
            .ok_or_else(|| denied(format!("{} is not a root of the module", named.display())))?,
    };
    Ok(Beneath {
        root,
        rel: path.path.clone(),
        deny,
    })
}

/// The root's directory, opened.
fn open_root(root: &Root) -> Result<Dir, ModuleError> {
    Dir::open_ambient_dir(&root.path, ambient_authority()).map_err(|e| io_error(&root.path, &e))
}

/// The error of an I/O failure on `path`: an escape is `Denied` (cap-std
/// answers it as permission denied, FreeBSD's kernel as not capable), a
/// missing path `NotFound`.
pub fn io_error(path: &Path, e: &std::io::Error) -> ModuleError {
    let kind = match e.kind() {
        _ if not_capable(e) => ModuleErrorKind::Denied,
        ErrorKind::NotFound => ModuleErrorKind::NotFound,
        ErrorKind::PermissionDenied => ModuleErrorKind::Denied,
        ErrorKind::InvalidInput | ErrorKind::IsADirectory | ErrorKind::NotADirectory => {
            ModuleErrorKind::InvalidArgument
        }
        _ => ModuleErrorKind::Unavailable,
    };
    ModuleError::new(kind, format!("{}: {e}", path.display()))
}

/// Whether FreeBSD refused a path that leaves the directory it resolves
/// beneath (`ENOTCAPABLE`, of `O_RESOLVE_BENEATH`).
#[cfg(target_os = "freebsd")]
fn not_capable(e: &std::io::Error) -> bool {
    e.raw_os_error() == Some(nix::errno::Errno::ENOTCAPABLE as i32)
}

/// Elsewhere an escape is answered as permission denied.
#[cfg(not(target_os = "freebsd"))]
fn not_capable(_e: &std::io::Error) -> bool {
    false
}

/// The root's directory and the path under it, for a write.
fn writable(at: &Beneath<'_>) -> Result<Dir, ModuleError> {
    if !at.root.write {
        return Err(denied(format!(
            "{} does not allow writes",
            at.root.path.display()
        )));
    }
    open_root(at.root)
}

/// The path as the root sees it: `.` for the root itself.
fn rel(at: &Beneath<'_>) -> PathBuf {
    if at.rel.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        at.rel.clone()
    }
}

fn metadata_of(meta: &cap_std::fs::Metadata) -> Metadata {
    let kind = if meta.is_symlink() {
        EntryKind::Symlink
    } else if meta.is_dir() {
        EntryKind::Dir
    } else if meta.is_file() {
        EntryKind::File
    } else {
        EntryKind::Other
    };
    Metadata {
        kind,
        size: meta.len(),
        modified: meta
            .modified()
            .ok()
            .map(cap_std::time::SystemTime::into_std),
    }
}

/// Read at most `limit` bytes of the file at `at`.
pub fn read(at: &Beneath<'_>, limit: usize) -> Result<Vec<u8>, ModuleError> {
    let path = rel(at);
    let file = open_root(at.root)?
        .open(&path)
        .map_err(|e| io_error(&path, &e))?;
    at.reached(&file)?;
    let mut bytes = Vec::new();
    file.take(u64::try_from(limit).unwrap_or(u64::MAX))
        .read_to_end(&mut bytes)
        .map_err(|e| io_error(&path, &e))?;
    Ok(bytes)
}

/// Whom a file or directory belongs to: its user's id and its group's.
pub type Owner = (u32, u32);

/// Make `handle` belong to `owner`, when one is named.
fn give(handle: &impl std::os::fd::AsFd, owner: Option<Owner>) -> std::io::Result<()> {
    match owner {
        Some((uid, gid)) => std::os::unix::fs::fchown(handle, Some(uid), Some(gid)),
        None => Ok(()),
    }
}

/// Make the directories of `path` beneath `dir` that are not there, each
/// belonging to `owner`.
fn make_dirs(dir: &Dir, path: &Path, owner: Option<Owner>) -> std::io::Result<()> {
    let mut made = PathBuf::new();
    for part in path.components() {
        made.push(part);
        match dir.create_dir(&made) {
            // Opened to read: `open_dir` answers a path handle on FreeBSD and
            // Linux, whose owner cannot be changed through it.
            Ok(()) => give(&dir.open(&made)?, owner)?,
            Err(e) if e.kind() == ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

/// Replace the file at `at` with `bytes`: written to a temporary file next
/// to it, then renamed over it, its directories made first. It keeps the
/// mode of the file it replaces and belongs to `owner`, when one is named.
pub fn write(at: &Beneath<'_>, bytes: &[u8], owner: Option<Owner>) -> Result<(), ModuleError> {
    let dir = writable(at)?;
    let path = rel(at);
    let name = path.file_name().ok_or_else(|| {
        ModuleError::new(
            ModuleErrorKind::InvalidArgument,
            format!("{} names no file", path.display()),
        )
    })?;
    let parent = path.parent().unwrap_or(Path::new(""));
    if !parent.as_os_str().is_empty() {
        // Followed beneath the root: a link out of it is refused here.
        match dir.metadata(parent) {
            Ok(meta) if meta.is_dir() => {}
            Ok(_) => {
                return Err(ModuleError::new(
                    ModuleErrorKind::InvalidArgument,
                    format!("{} is not a directory", parent.display()),
                ));
            }
            Err(e) if e.kind() == ErrorKind::NotFound => {
                make_dirs(&dir, parent, owner).map_err(|e| io_error(parent, &e))?;
            }
            Err(e) => return Err(io_error(parent, &e)),
        }
    }
    at.reached_dir(&dir, parent)?;
    let (temp, mut file) = temp_file(&dir, parent, name).map_err(|e| io_error(&path, &e))?;
    let written = (|| {
        file.write_all(bytes)?;
        match dir.metadata(&path) {
            Ok(replaced) => file.set_permissions(replaced.permissions())?,
            Err(e) if e.kind() == ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        give(&file, owner)?;
        file.sync_all()?;
        dir.rename(&temp, &dir, &path)
    })();
    if let Err(e) = written {
        if let Err(cleanup) = dir.remove_file(&temp)
            && cleanup.kind() != ErrorKind::NotFound
        {
            tracing::warn!(path = %temp.display(), error = %cleanup, "temporary file left");
        }
        return Err(io_error(&path, &e));
    }
    Ok(())
}

/// A new temporary file next to `name` in `parent`: the first free one of
/// `.<name>.pcx-<pid>-<n>.tmp`.
fn temp_file(
    dir: &Dir,
    parent: &Path,
    name: &std::ffi::OsStr,
) -> std::io::Result<(PathBuf, cap_std::fs::File)> {
    let mut options = cap_std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    let mut n = 0u32;
    loop {
        let mut temp_name = std::ffi::OsString::from(".");
        temp_name.push(name);
        temp_name.push(format!(".pcx-{}-{n}.tmp", std::process::id()));
        let temp = parent.join(temp_name);
        match dir.open_with(&temp, &options) {
            Ok(file) => return Ok((temp, file)),
            Err(e) if e.kind() == ErrorKind::AlreadyExists && n < u32::MAX => n += 1,
            Err(e) => return Err(e),
        }
    }
}

/// The entries of the directory at `at`, by name.
pub fn list(at: &Beneath<'_>) -> Result<Vec<DirEntry>, ModuleError> {
    let path = rel(at);
    let dir = open_root(at.root)?;
    at.reached_dir(&dir, &path)?;
    let mut entries = Vec::new();
    for entry in dir.read_dir(&path).map_err(|e| io_error(&path, &e))? {
        let entry = entry.map_err(|e| io_error(&path, &e))?;
        // A denied name is not shown.
        if at.deny.matches(&entry.file_name()) {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        let meta = entry
            .metadata()
            .map_err(|e| io_error(&path.join(&name), &e))?;
        entries.push(DirEntry {
            name,
            metadata: metadata_of(&meta),
        });
    }
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(entries)
}

/// What the path at `at` holds (a symbolic link is not followed).
pub fn metadata(at: &Beneath<'_>) -> Result<Metadata, ModuleError> {
    let path = rel(at);
    let dir = open_root(at.root)?;
    at.reached_dir(&dir, path.parent().unwrap_or(Path::new("")))?;
    let meta = dir
        .symlink_metadata(&path)
        .map_err(|e| io_error(&path, &e))?;
    Ok(metadata_of(&meta))
}

/// Remove the file or empty directory at `at`; never the root itself.
pub fn remove(at: &Beneath<'_>) -> Result<(), ModuleError> {
    if at.rel.components().all(|c| matches!(c, Component::CurDir)) {
        return Err(denied("a root is not removed".to_string()));
    }
    let dir = writable(at)?;
    let path = rel(at);
    at.reached_dir(&dir, path.parent().unwrap_or(Path::new("")))?;
    let meta = dir
        .symlink_metadata(&path)
        .map_err(|e| io_error(&path, &e))?;
    let removed = if meta.is_dir() {
        dir.remove_dir(&path)
    } else {
        dir.remove_file(&path)
    };
    removed.map_err(|e| io_error(&path, &e))
}

/// Move `from` to `to`, each under a root that allows writes.
pub fn rename(from: &Beneath<'_>, to: &Beneath<'_>) -> Result<(), ModuleError> {
    let (source, target) = (writable(from)?, writable(to)?);
    let (from_path, to_path) = (rel(from), rel(to));
    from.reached_dir(&source, from_path.parent().unwrap_or(Path::new("")))?;
    to.reached_dir(&target, to_path.parent().unwrap_or(Path::new("")))?;
    source
        .rename(&from_path, &target, &to_path)
        .map_err(|e| io_error(&from_path, &e))
}

/// The directory a process starts in: `at`, which must be a directory
/// beneath its root, as the host sees it.
pub fn working_dir(at: &Beneath<'_>) -> Result<PathBuf, ModuleError> {
    at.reached_dir(&open_root(at.root)?, &rel(at))?;
    let meta = metadata(at)?;
    if meta.kind != EntryKind::Dir {
        return Err(ModuleError::new(
            ModuleErrorKind::InvalidArgument,
            format!("{} is not a directory", at.rel.display()),
        ));
    }
    Ok(at.root.path.join(&at.rel))
}

#[cfg(test)]
mod tests;
