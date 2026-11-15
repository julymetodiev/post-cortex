// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! What git prints, read: `status --porcelain=v1 -z`, the records of `log`
//! and `branch` in the formats `tools-git` asks for, and Conventional
//! Commits (ported from Axon). Pure functions; no process here.

/// Fields of a record `tools-git` asks git for (`%x1f`).
pub const FIELD: char = '\u{1f}';

/// Records `tools-git` asks git for (`%x1e`).
pub const RECORD: char = '\u{1e}';

/// Output of git that does not read as the format asked for.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("git printed {0}")]
pub struct Unreadable(pub String);

/// A path the working tree or the index changes, a commit, a local
/// branch and a Conventional Commit: the messages of `pcx.tools.git.v1`.
pub use post_cortex_proto::tools_git::{
    Branch, Change, ChangeKind, Commit, Conventional, FileState,
};

/// The state `git status` prints as `c`.
fn state(c: char) -> Option<FileState> {
    Some(match c {
        ' ' => FileState::Unmodified,
        'M' => FileState::Modified,
        'T' => FileState::TypeChanged,
        'A' => FileState::Added,
        'D' => FileState::Deleted,
        'R' => FileState::Renamed,
        'C' => FileState::Copied,
        'U' => FileState::Unmerged,
        '?' => FileState::Untracked,
        '!' => FileState::Ignored,
        _ => return None,
    })
}

/// What a change of states `index` and `worktree` is.
fn kind(index: char, worktree: char) -> ChangeKind {
    match (index, worktree) {
        ('?', '?') => ChangeKind::Untracked,
        ('U', _) | (_, 'U') | ('A', 'A') | ('D', 'D') => ChangeKind::Conflict,
        ('A', _) => ChangeKind::StagedNew,
        ('R', _) => ChangeKind::Renamed,
        ('D', _) => ChangeKind::StagedDeleted,
        (_, 'D') => ChangeKind::Deleted,
        ('M', 'M') => ChangeKind::StagedModified,
        ('M', _) => ChangeKind::Staged,
        _ => ChangeKind::Modified,
    }
}

/// The changes of `git status --porcelain=v1 -z` output: `XY path`, a
/// rename followed by its old path, each ended by NUL.
///
/// # Errors
/// An entry that is not `XY path`, or a rename without its old path.
pub fn status(out: &str) -> Result<Vec<Change>, Unreadable> {
    let mut changes = Vec::new();
    let mut entries = out.split('\0').filter(|e| !e.is_empty());
    while let Some(entry) = entries.next() {
        let unreadable = || Unreadable(format!("the status entry {entry}"));
        let mut chars = entry.chars();
        let (Some(index), Some(worktree), Some(' ')) = (chars.next(), chars.next(), chars.next())
        else {
            return Err(unreadable());
        };
        let path = chars.as_str().to_string();
        let from = if matches!(index, 'R' | 'C') {
            Some(entries.next().ok_or_else(unreadable)?.to_string())
        } else {
            None
        };
        let (Some(in_index), Some(in_worktree)) = (state(index), state(worktree)) else {
            return Err(unreadable());
        };
        changes.push(Change {
            path,
            from,
            index: in_index.into(),
            worktree: in_worktree.into(),
            kind: kind(index, worktree).into(),
        });
    }
    Ok(changes)
}

/// The commits of `log --format=%H%x1f%an%x1f%aI%x1f%s%x1e`.
///
/// # Errors
/// A record without those four fields.
pub fn log(out: &str) -> Result<Vec<Commit>, Unreadable> {
    records(out)
        .map(|fields| match fields.as_slice() {
            [sha, author, date, subject] => Ok(Commit {
                sha: (*sha).to_string(),
                author: (*author).to_string(),
                date: (*date).to_string(),
                subject: (*subject).to_string(),
            }),
            other => Err(Unreadable(format!("the log record {}", other.join("|")))),
        })
        .collect()
}

/// The branches of `branch --format=%(refname:short)%1f%(HEAD)%1f%(upstream:short)%1e`.
///
/// # Errors
/// A record without those three fields.
pub fn branches(out: &str) -> Result<Vec<Branch>, Unreadable> {
    records(out)
        .map(|fields| match fields.as_slice() {
            [name, head, upstream] => Ok(Branch {
                name: (*name).to_string(),
                current: *head == "*",
                upstream: (!upstream.is_empty()).then(|| (*upstream).to_string()),
            }),
            other => Err(Unreadable(format!("the branch record {}", other.join("|")))),
        })
        .collect()
}

/// The fields of each record of `out`.
fn records(out: &str) -> impl Iterator<Item = Vec<&str>> {
    out.split(RECORD)
        .map(|r| r.trim_start_matches('\n'))
        .filter(|r| !r.is_empty())
        .map(|r| r.split(FIELD).collect())
}

/// The types of Conventional Commits.
const TYPES: [&str; 11] = [
    "feat", "fix", "docs", "style", "refactor", "perf", "test", "build", "ci", "chore", "revert",
];

/// `message` as a Conventional Commit; none when it is not one.
#[must_use]
pub fn conventional(message: &str) -> Option<Conventional> {
    let first = message.lines().next()?;
    let (prefix, description) = first.split_once(':')?;
    let description = description.trim();
    if description.is_empty() {
        return None;
    }
    let (prefix, breaking) = match prefix.strip_suffix('!') {
        Some(p) => (p, true),
        None => (prefix, false),
    };
    let (kind, scope) = match prefix.split_once('(') {
        Some((kind, rest)) => (kind, Some(rest.strip_suffix(')')?.to_string())),
        None => (prefix, None),
    };
    TYPES.contains(&kind).then(|| Conventional {
        kind: kind.to_string(),
        scope,
        breaking,
        description: description.to_string(),
    })
}

#[cfg(test)]
mod tests;
