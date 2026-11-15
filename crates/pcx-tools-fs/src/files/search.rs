// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Finding files (glob) and lines (grep): both walk the sandbox from a
//! directory, skipping hidden entries, what the root's `.gitignore`
//! ignores and symbolic links, visiting at most so many entries.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};

use globset::{Glob, GlobMatcher};
use ignore::gitignore::{Gitignore, GitignoreBuilder};
use pcx_module_api::sandbox::{EntryKind, Sandbox, SandboxPath};
use pcx_module_api::text::clip;
use pcx_module_api::{ModuleError, ModuleErrorKind};
use regex::RegexBuilder;

use crate::files::Files;
use crate::files::text::{GrepLine, grep_lines};

/// Entries a walk visits at most.
const MAX_VISITED: usize = 50_000;
/// Bytes of a file grep reads at most.
const MAX_GREP_FILE: usize = 1024 * 1024;

fn invalid(reason: String) -> ModuleError {
    ModuleError::new(ModuleErrorKind::InvalidArgument, reason)
}

/// Where a search starts: a directory relative to a root, or absolute
/// under one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Start {
    /// The root it is relative to; none: the first.
    pub root: Option<PathBuf>,
    /// The directory.
    pub dir: PathBuf,
}

/// A glob search.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GlobRequest {
    /// Where it starts.
    pub start: Start,
    /// The pattern.
    pub pattern: String,
    /// Most files answered.
    pub limit: usize,
}

/// What a glob found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Globbed {
    /// The files, as their root sees them, at most the limit.
    pub files: Vec<String>,
    /// Files matching in all.
    pub total: usize,
    /// Whether the walk stopped at [`MAX_VISITED`].
    pub cut: bool,
}

/// A grep search.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrepRequest {
    /// Where it starts.
    pub start: Start,
    /// The regex, or literal text.
    pub pattern: String,
    /// Whether the pattern is literal text.
    pub literal: bool,
    /// Whether case is ignored.
    pub ignore_case: bool,
    /// Only files matching this glob.
    pub glob: Option<String>,
    /// Lines around each match.
    pub context: usize,
    /// Most matches answered.
    pub limit: usize,
}

/// Characters of a line a search answers at most.
pub const MAX_LINE: usize = 500;

/// A line a grep answers, with its file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrepHit {
    /// The file, as its root sees it.
    pub file: String,
    /// The line.
    pub line: GrepLine,
}

/// What a grep found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Grepped {
    /// The matched lines with their context, in file and line order.
    pub hits: Vec<GrepHit>,
    /// Matches answered.
    pub matches: usize,
    /// Whether there are more than the limit.
    pub more: bool,
    /// Whether the walk stopped at [`MAX_VISITED`].
    pub cut: bool,
}

/// A file a walk found.
struct Found {
    /// Its path from the walk's start.
    rel: PathBuf,
    /// Where it is, for the sandbox.
    at: SandboxPath,
}

/// The files beneath a directory, in walk order.
struct Walk {
    files: Vec<Found>,
    cut: bool,
}

fn matcher(pattern: &str) -> Result<GlobMatcher, ModuleError> {
    Glob::new(pattern)
        .map(|g| g.compile_matcher())
        .map_err(|e| invalid(format!("pattern {pattern}: {e}")))
}

/// Whether `file` matches `glob`: by its path from the walk's start, or by
/// its name for a pattern without `/`.
fn matches(glob: &GlobMatcher, pattern: &str, file: &Found) -> bool {
    glob.is_match(&file.rel)
        || (!pattern.contains('/') && file.rel.file_name().is_some_and(|n| glob.is_match(n)))
}

impl Files<'_> {
    /// The files under `request.start` matching its pattern.
    ///
    /// # Errors
    /// `InvalidArgument` for a bad pattern; as the sandbox's list.
    pub async fn glob(&self, request: &GlobRequest) -> Result<Globbed, ModuleError> {
        let glob = matcher(&request.pattern)?;
        let walk = walk(self.sandbox, &request.start).await?;
        // Asked by an absolute path, it answers absolute paths: they name their
        // root, which a path relative to it does not.
        let absolute = request.start.dir.is_absolute();
        let found: Vec<String> = walk
            .files
            .iter()
            .filter(|f| matches(&glob, &request.pattern, f))
            .map(|f| match &f.at.root {
                Some(root) if absolute => root.join(&f.at.path).display().to_string(),
                _ => f.at.path.display().to_string(),
            })
            .collect();
        let total = found.len();
        Ok(Globbed {
            files: found.into_iter().take(request.limit).collect(),
            total,
            cut: walk.cut,
        })
    }

    /// The lines under `request.start` matching its pattern.
    ///
    /// # Errors
    /// `InvalidArgument` for a bad pattern or glob; as the sandbox's list
    /// and read.
    pub async fn grep(&self, request: &GrepRequest) -> Result<Grepped, ModuleError> {
        let pattern = if request.literal {
            regex::escape(&request.pattern)
        } else {
            request.pattern.clone()
        };
        let regex = RegexBuilder::new(&pattern)
            .case_insensitive(request.ignore_case)
            .build()
            .map_err(|e| invalid(format!("pattern {}: {e}", request.pattern)))?;
        let filter = request
            .glob
            .as_deref()
            .map(|g| Ok::<_, ModuleError>((g, matcher(g)?)))
            .transpose()?;
        let walk = walk(self.sandbox, &request.start).await?;
        let mut hits = Vec::new();
        let mut matched = 0usize;
        let mut more = false;
        for file in &walk.files {
            if let Some((pattern, glob)) = &filter
                && !matches(glob, pattern, file)
            {
                continue;
            }
            let bytes = self.sandbox.read(&file.at, MAX_GREP_FILE).await?;
            // Binary files (a NUL byte) are not searched.
            if bytes.contains(&0) {
                continue;
            }
            let text = String::from_utf8_lossy(&bytes);
            let room = request.limit - matched;
            let (lines, found) = grep_lines(&text, &regex, request.context, room);
            let name = file.at.path.display().to_string();
            hits.extend(lines.into_iter().map(|line| GrepHit {
                file: name.clone(),
                line: GrepLine {
                    text: clip(&line.text, MAX_LINE),
                    ..line
                },
            }));
            if found > room {
                matched = request.limit;
                more = true;
                break;
            }
            matched += found;
            if matched == request.limit {
                break;
            }
        }
        Ok(Grepped {
            hits,
            matches: matched,
            more,
            cut: walk.cut,
        })
    }
}

/// The root's `.gitignore`, if it has one.
async fn gitignore(sandbox: &dyn Sandbox, root: Option<&Path>) -> Result<Gitignore, ModuleError> {
    let base = sandbox
        .policy()
        .roots
        .iter()
        .find(|r| root.is_none_or(|named| named == r.path))
        .map_or_else(PathBuf::new, |r| r.path.clone());
    let mut builder = GitignoreBuilder::new(&base);
    let file = SandboxPath {
        root: root.map(Path::to_path_buf),
        path: PathBuf::from(".gitignore"),
    };
    match sandbox.read(&file, MAX_GREP_FILE).await {
        Ok(bytes) => {
            for line in String::from_utf8_lossy(&bytes).lines() {
                builder
                    .add_line(None, line)
                    .map_err(|e| invalid(format!(".gitignore: {e}")))?;
            }
        }
        Err(e) if e.kind == ModuleErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    builder
        .build()
        .map_err(|e| invalid(format!(".gitignore: {e}")))
}

/// `start` as a directory relative to a root: an absolute one under a
/// root becomes relative to it, so the walk's paths stay relative.
fn relative<'a>(
    sandbox: &'a dyn Sandbox,
    start: &'a Start,
) -> Result<(Option<&'a Path>, PathBuf), ModuleError> {
    let root = start.root.as_deref();
    if !start.dir.is_absolute() {
        return Ok((root, start.dir.clone()));
    }
    sandbox
        .policy()
        .roots
        .iter()
        .filter(|r| root.is_none_or(|named| named == r.path))
        .find_map(|r| {
            let rel = start.dir.strip_prefix(&r.path).ok()?;
            Some((Some(r.path.as_path()), rel.to_path_buf()))
        })
        .ok_or_else(|| {
            ModuleError::new(
                ModuleErrorKind::Denied,
                format!("{} is under none of the roots", start.dir.display()),
            )
        })
}

/// Walk from `start`.
async fn walk(sandbox: &dyn Sandbox, start: &Start) -> Result<Walk, ModuleError> {
    let (root, dir) = relative(sandbox, start)?;
    let ignored = gitignore(sandbox, root).await?;
    let mut files = Vec::new();
    let mut queue = VecDeque::from([dir.clone()]);
    let mut visited = 0usize;
    while let Some(next) = queue.pop_front() {
        let at = SandboxPath {
            root: root.map(Path::to_path_buf),
            path: next.clone(),
        };
        for entry in sandbox.list(&at).await? {
            visited += 1;
            if visited > MAX_VISITED {
                return Ok(Walk { files, cut: true });
            }
            if entry.name.starts_with('.') {
                continue;
            }
            let path = next.join(&entry.name);
            let is_dir = entry.metadata.kind == EntryKind::Dir;
            if ignored
                .matched_path_or_any_parents(&path, is_dir)
                .is_ignore()
            {
                continue;
            }
            match entry.metadata.kind {
                EntryKind::Dir => queue.push_back(path),
                EntryKind::File => files.push(Found {
                    rel: path
                        .strip_prefix(&dir)
                        .map_or_else(|_| path.clone(), Path::to_path_buf),
                    at: SandboxPath {
                        root: root.map(Path::to_path_buf),
                        path,
                    },
                }),
                EntryKind::Symlink | EntryKind::Other => {}
            }
        }
    }
    Ok(Walk { files, cut: false })
}
