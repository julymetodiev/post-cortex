// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! What a FreeBSD helper makes and mounts for a module's jail, written down
//! before each step (`<base>/<jail>.mounts`, one step a line), so that what
//! a killed helper left is undone by the next helper of that jail
//! (docs/host.md, sandbox and tools). A step is undone in reverse; undoing
//! one already undone, or never done, is not an error. Nothing is removed
//! recursively: a directory goes only once it is empty, so a root still
//! mounted is never reached.

use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde::{Deserialize, Serialize};

/// The extension of a jail's record.
const EXTENSION: &str = "mounts";

/// `mount(8)` and `umount(8)`.
const MOUNT: &str = "/sbin/mount";
const UMOUNT: &str = "/sbin/umount";

/// One step of building a jail's root.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Step {
    /// A directory made.
    Dir(PathBuf),
    /// An empty file made (a mount point).
    File(PathBuf),
    /// A file system mounted there.
    Mount(PathBuf),
}

/// A jail's record of the steps that built its root.
#[derive(Debug)]
pub struct Record {
    path: PathBuf,
    file: File,
}

impl Record {
    /// A new record for jail `name` in `base`, once what an earlier record
    /// of it holds (a helper killed while it built or removed the jail) is
    /// undone.
    ///
    /// # Errors
    /// The earlier record cannot be read or undone, or the new one made.
    pub fn fresh(base: &Path, name: &str) -> std::io::Result<Self> {
        undone(base, name)?;
        let path = record_path(base, name);
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|e| named(&path, e))?;
        Ok(Self { path, file })
    }

    /// Write `step` down, before it is taken.
    ///
    /// # Errors
    /// It cannot be written.
    pub fn note(&mut self, step: Step) -> std::io::Result<()> {
        let line = serde_json::to_string(&step)?;
        writeln!(self.file, "{line}")
            .and_then(|()| self.file.sync_data())
            .map_err(|e| named(&self.path, e))?;
        Ok(())
    }
}

/// Undo the record of jail `name` in `base`, if there is one, then remove
/// it; on a failure the record stays, for the jail's next helper.
///
/// # Errors
/// The record cannot be read, or a step cannot be undone.
pub fn undone(base: &Path, name: &str) -> std::io::Result<()> {
    let path = record_path(base, name);
    if !path.exists() {
        return Ok(());
    }
    undo(&steps_in(&path)?)?;
    std::fs::remove_file(&path).map_err(|e| named(&path, e))
}

/// The record of jail `name` in `base`.
fn record_path(base: &Path, name: &str) -> PathBuf {
    base.join(format!("{name}.{EXTENSION}"))
}

/// The steps written in the record at `path`; a last line cut short (the
/// helper killed while writing it) is a step never taken.
fn steps_in(path: &Path) -> std::io::Result<Vec<Step>> {
    let file = File::open(path).map_err(|e| named(path, e))?;
    let lines: Vec<String> = BufReader::new(file)
        .lines()
        .collect::<Result<_, _>>()
        .map_err(|e| named(path, e))?;
    let mut steps = Vec::with_capacity(lines.len());
    for (i, line) in lines.iter().enumerate() {
        match serde_json::from_str(line) {
            Ok(step) => steps.push(step),
            Err(_) if i + 1 == lines.len() => {}
            Err(e) => return Err(named(path, e.into())),
        }
    }
    Ok(steps)
}

/// Undo `steps` in reverse.
fn undo(steps: &[Step]) -> std::io::Result<()> {
    let mounted = mount_points()?;
    for step in steps.iter().rev() {
        let (path, done) = match step {
            Step::Mount(path) if is_mounted(&mounted, path) => (path, force_unmount(path)),
            // Not mounted: never, or undone already.
            Step::Mount(_) => continue,
            Step::Dir(path) => (path, std::fs::remove_dir(path)),
            Step::File(path) => (path, std::fs::remove_file(path)),
        };
        match done {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(named(path, e)),
        }
    }
    Ok(())
}

/// The mount points of the system's mounts (`mount -p`), as the system
/// names them: a directory's under the host's path of this jail, a file's
/// as this jail sees it.
fn mount_points() -> std::io::Result<Vec<PathBuf>> {
    let listed = quiet(Command::new(MOUNT).arg("-p"))?;
    Ok(String::from_utf8_lossy(&listed)
        .lines()
        .filter_map(|line| line.split_whitespace().nth(1))
        .map(|point| PathBuf::from(unescaped(point)))
        .collect())
}

/// `field` of `mount -p` with its octal escapes (`\040`, a space) read.
fn unescaped(field: &str) -> String {
    let mut out = String::with_capacity(field.len());
    let mut chars = field.chars().peekable();
    while let Some(c) = chars.next() {
        let digits: String = chars.clone().take(3).collect();
        match (c, u8::from_str_radix(&digits, 8)) {
            ('\\', Ok(byte)) if digits.len() == 3 => {
                out.push(char::from(byte));
                chars.nth(2);
            }
            (c, _) => out.push(c),
        }
    }
    out
}

/// Whether one of `mounted` is `path`, as the host or this jail names it.
fn is_mounted(mounted: &[PathBuf], path: &Path) -> bool {
    let relative = path.strip_prefix("/").unwrap_or(path);
    mounted
        .iter()
        .any(|point| point == path || point.ends_with(relative))
}

/// Unmount what is mounted at `path`, even in use: `umount(8)`, which
/// finds the mount and unmounts it by its id (by path the kernel refuses a
/// mount over a file).
fn force_unmount(path: &Path) -> std::io::Result<()> {
    quiet(Command::new(UMOUNT).arg("-f").arg(path)).map(|_| ())
}

/// The output of `command`, run with nothing reaching this process's
/// streams (they are the jailed program's); its error output in the error
/// when it fails.
fn quiet(command: &mut Command) -> std::io::Result<Vec<u8>> {
    let output = command.stdin(Stdio::null()).output()?;
    if output.status.success() {
        Ok(output.stdout)
    } else {
        Err(std::io::Error::other(format!(
            "{command:?}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )))
    }
}

/// `e`, naming `path`.
pub fn named(path: &Path, e: std::io::Error) -> std::io::Error {
    std::io::Error::new(e.kind(), format!("{}: {e}", path.display()))
}

#[cfg(test)]
mod tests;
