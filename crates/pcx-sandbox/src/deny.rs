// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The names a module never reaches under its roots (`Policy.deny`, a
//! runner's `[runner] deny`, docs/connect.md section 7): a path any of
//! whose components matches one is refused, to its files and its
//! processes, so a credential does not reach a model.

use std::ffi::OsStr;
use std::os::fd::AsFd;
use std::path::{Component, Path, PathBuf};

use globset::{GlobBuilder, GlobSet, GlobSetBuilder};

/// The directories not searched for denied names: a repository's objects,
/// build output and installed packages, large and made by tools.
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
const UNSEARCHED: [&str; 3] = [".git", "target", "node_modules"];

/// The characters a regular expression gives a meaning (`*` and `?` are
/// a denied name's own).
pub const REGEX_SPECIAL: &str = ".^$|()[]{}+\\";

/// Why a list of denied names is not one.
#[derive(Debug, thiserror::Error)]
pub enum DenyError {
    /// A name is no glob.
    #[error("{pattern} is no glob: {source}")]
    Pattern {
        /// The name.
        pattern: String,
        /// Why.
        source: globset::Error,
    },
    /// A name holds a separator: it names a path, not a name.
    #[error("{0} names a path; a denied name is one component (`.env`, `*.pem`)")]
    Path(String),
    /// A name uses glob syntax beyond `*` and `?`, which an OS sandbox's
    /// rule could not say the same way.
    #[error("{0}: a denied name takes `*` and `?` only")]
    Syntax(String),
}

/// The denied names, compiled.
#[derive(Debug, Clone)]
pub struct Deny {
    names: GlobSet,
    patterns: Vec<String>,
}

impl Deny {
    /// The names of `patterns` (globs of one component).
    ///
    /// # Errors
    /// See [`DenyError`].
    pub fn new(patterns: &[String]) -> Result<Self, DenyError> {
        let mut set = GlobSetBuilder::new();
        for pattern in patterns {
            if pattern.contains('/') {
                return Err(DenyError::Path(pattern.clone()));
            }
            if pattern.contains(['[', ']', '{', '}', '\\']) {
                return Err(DenyError::Syntax(pattern.clone()));
            }
            let glob = GlobBuilder::new(pattern)
                .literal_separator(true)
                .build()
                .map_err(|source| DenyError::Pattern {
                    pattern: pattern.clone(),
                    source,
                })?;
            set.add(glob);
        }
        let names = set.build().map_err(|source| DenyError::Pattern {
            pattern: patterns.join(", "),
            source,
        })?;
        Ok(Self {
            names,
            patterns: patterns.to_vec(),
        })
    }

    /// Whether no name is denied.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.patterns.is_empty()
    }

    /// The denied names as given.
    #[must_use]
    pub fn patterns(&self) -> &[String] {
        &self.patterns
    }

    /// Whether `name` is denied.
    #[must_use]
    pub fn matches(&self, name: &OsStr) -> bool {
        self.names.is_match(Path::new(name))
    }

    /// Each denied name as a regular expression of one path component
    /// (`*`: any characters but `/`, `?`: one), for an OS sandbox's rules.
    #[must_use]
    pub fn component_regexes(&self) -> Vec<String> {
        self.patterns
            .iter()
            .map(|pattern| {
                pattern
                    .chars()
                    .map(|c| match c {
                        '*' => "[^/]*".to_string(),
                        '?' => "[^/]".to_string(),
                        c if REGEX_SPECIAL.contains(c) => format!("\\{c}"),
                        c => c.to_string(),
                    })
                    .collect()
            })
            .collect()
    }

    /// The first component of `path` that is denied.
    #[must_use]
    pub fn refused(&self, path: &Path) -> Option<String> {
        path.components().find_map(|c| match c {
            Component::Normal(name) if self.matches(name) => {
                Some(name.to_string_lossy().into_owned())
            }
            _ => None,
        })
    }

    /// The first denied component of where the open file or directory
    /// `handle` is, below the root at `root` (resolved: the system's path
    /// of the handle, so a link to a denied name is seen through).
    ///
    /// # Errors
    /// The system cannot say where the handle is.
    pub fn refused_at(&self, root: &Path, handle: impl AsFd) -> std::io::Result<Option<String>> {
        if self.is_empty() {
            return Ok(None);
        }
        let at = path_of(handle)?;
        let root = std::fs::canonicalize(root)?;
        Ok(self.refused(at.strip_prefix(&root).unwrap_or(&at)))
    }

    /// The paths under `dir` with a denied name, each with whether it is
    /// a directory (whose contents are then not searched), for an OS
    /// sandbox to hide; a link is not followed (its target is covered
    /// where it lies), and the `.git`, `target` and `node_modules`
    /// directories are skipped. A
    /// `dir` that does not exist has none.
    ///
    /// # Errors
    /// A directory cannot be read: the error names it.
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    pub fn found_under(&self, dir: &Path) -> std::io::Result<Vec<(PathBuf, bool)>> {
        let mut found = Vec::new();
        if !self.is_empty() {
            self.search(dir, &mut found)?;
        }
        Ok(found)
    }

    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    fn search(&self, dir: &Path, found: &mut Vec<(PathBuf, bool)>) -> std::io::Result<()> {
        let named = |path: &Path, e: std::io::Error| {
            std::io::Error::new(e.kind(), format!("{}: {e}", path.display()))
        };
        let entries = match std::fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(named(dir, e)),
        };
        for entry in entries {
            let entry = entry.map_err(|e| named(dir, e))?;
            let kind = entry.file_type().map_err(|e| named(&entry.path(), e))?;
            let name = entry.file_name();
            if self.matches(&name) {
                if !kind.is_symlink() {
                    found.push((entry.path(), kind.is_dir()));
                }
            } else if kind.is_dir() && !UNSEARCHED.iter().any(|skip| name == *skip) {
                self.search(&entry.path(), found)?;
            }
        }
        Ok(())
    }
}

/// Where the system says the open `handle` is.
#[cfg(target_os = "linux")]
fn path_of(handle: impl AsFd) -> std::io::Result<PathBuf> {
    use std::os::fd::AsRawFd;
    std::fs::read_link(format!("/proc/self/fd/{}", handle.as_fd().as_raw_fd()))
}

/// Where the system says the open `handle` is.
#[cfg(target_os = "macos")]
fn path_of(handle: impl AsFd) -> std::io::Result<PathBuf> {
    let mut path = PathBuf::new();
    nix::fcntl::fcntl(handle, nix::fcntl::FcntlArg::F_GETPATH(&mut path))?;
    Ok(path)
}

/// Where the system says the open `handle` is.
#[cfg(target_os = "freebsd")]
fn path_of(handle: impl AsFd) -> std::io::Result<PathBuf> {
    let mut path = PathBuf::new();
    nix::fcntl::fcntl(handle, nix::fcntl::FcntlArg::F_KINFO(&mut path))?;
    Ok(path)
}

/// Elsewhere the system does not say: a denied name cannot be checked, so
/// nothing is reached (fail closed).
#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "freebsd")))]
fn path_of(_handle: impl AsFd) -> std::io::Result<PathBuf> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "this system does not say where an open file is",
    ))
}

#[cfg(test)]
mod tests;
