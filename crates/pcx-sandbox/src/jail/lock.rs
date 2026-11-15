// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The locks of a module's jail, which its helpers share (docs/host.md,
//! sandbox and tools), files only root reaches (`<base>` is root's): one
//! builds or removes the jail at a time
//! (`<name>.lock`, exclusive), and each helper whose program runs in it
//! holds `<name>.users` shared, so the jail goes only when none does. A
//! helper that dies drops its locks with it. The last to leave removes the
//! files with the jail, as each launch of a module may have a jail of its
//! own; a helper that waited on a removed file opens the new one.

use std::fs::{File, OpenOptions};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use nix::errno::Errno;
use nix::fcntl::{Flock, FlockArg};

use crate::jail::record::named;

/// The two lock files of one jail.
pub struct Locks {
    build: PathBuf,
    users: PathBuf,
}

/// A lock held until it is dropped.
pub type Held = Flock<File>;

impl Locks {
    /// The locks of jail `name`, in `base`.
    pub fn of(base: &Path, name: &str) -> Self {
        Self {
            build: base.join(format!("{name}.lock")),
            users: base.join(format!("{name}.users")),
        }
    }

    /// Building or removing the jail, alone (waits for the helper doing
    /// it now).
    ///
    /// # Errors
    /// The lock file cannot be opened or locked.
    pub fn building(&self) -> std::io::Result<Held> {
        wait_for(&self.build, FlockArg::LockExclusive)
    }

    /// Running a program in the jail, beside the others that do.
    ///
    /// # Errors
    /// The lock file cannot be opened or locked.
    pub fn using(&self) -> std::io::Result<Held> {
        wait_for(&self.users, FlockArg::LockShared)
    }

    /// The jail's users, if no helper runs a program in it now; none
    /// while one does.
    ///
    /// # Errors
    /// The lock file cannot be opened or locked.
    pub fn unused(&self) -> std::io::Result<Option<Held>> {
        match Flock::lock(open(&self.users)?, FlockArg::LockExclusiveNonblock) {
            Ok(held) => Ok(Some(held)),
            Err((_, Errno::EWOULDBLOCK)) => Ok(None),
            Err((_, errno)) => Err(named(&self.users, errno.into())),
        }
    }

    /// Remove the lock files, the jail gone; called holding
    /// [`Locks::building`] and [`Locks::unused`].
    ///
    /// # Errors
    /// A file cannot be removed.
    pub fn remove(&self) -> std::io::Result<()> {
        // A jail that could not be built never had users.
        for file in [&self.users, &self.build] {
            match std::fs::remove_file(file) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(named(file, e)),
            }
        }
        Ok(())
    }
}

/// `path` locked with `arg`, waiting for whoever holds it.
fn wait_for(path: &Path, arg: FlockArg) -> std::io::Result<Held> {
    loop {
        let held = Flock::lock(open(path)?, arg).map_err(|(_, errno)| named(path, errno.into()))?;
        if is_the_file(&held, path) {
            return Ok(held);
        }
    }
}

/// Whether `held` is still the file at `path`, not one removed while it
/// waited.
fn is_the_file(held: &Held, path: &Path) -> bool {
    match (held.metadata(), std::fs::metadata(path)) {
        (Ok(held), Ok(now)) => held.dev() == now.dev() && held.ino() == now.ino(),
        _ => false,
    }
}

/// The lock file `path`, made if missing.
fn open(path: &Path) -> std::io::Result<File> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
        .map_err(|e| named(path, e))
}
