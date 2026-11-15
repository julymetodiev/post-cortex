// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Where a member works on a task of a board with a repository
//! (ARCHITECTURE section 8, operations, W32): a worktree of its own inside
//! the repository, `.worktrees/<task>`, on the branch `task/<task>-<slug of
//! its title>`, which the task links, and what its turn is told of it.
//! Pure.

/// Characters of a task's id in its worktree and branch.
const ID_CHARS: usize = 8;

/// Characters of a branch's slug of the task's title.
const SLUG_CHARS: usize = 40;

/// The short id of task `task`: its first characters that are letters or
/// digits.
fn short(task: &str) -> String {
    task.chars()
        .filter(char::is_ascii_alphanumeric)
        .take(ID_CHARS)
        .collect::<String>()
        .to_ascii_lowercase()
}

/// `title` as a branch's slug: lowercase letters and digits, words joined
/// by '-'.
fn slug(title: &str) -> String {
    let words: Vec<String> = title
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_ascii_lowercase)
        .collect();
    let mut slug = String::new();
    for word in words {
        if slug.len() + word.len() + 1 > SLUG_CHARS {
            break;
        }
        if !slug.is_empty() {
            slug.push('-');
        }
        slug.push_str(&word);
    }
    slug
}

/// The worktree (inside the repository) and branch of task `task` titled
/// `title`.
#[must_use]
pub fn place(task: &str, title: &str) -> (String, String) {
    let id = short(task);
    let slug = slug(title);
    let branch = if slug.is_empty() {
        format!("task/{id}")
    } else {
        format!("task/{id}-{slug}")
    };
    (format!(".worktrees/{id}"), branch)
}

/// What a member's turn is told of where it works.
#[must_use]
pub fn told(worktree: &str, branch: &str) -> String {
    format!(
        "Work in your worktree of the task's repository, {worktree}, on its branch {branch}: read, change and commit there only (the task links the branch, so its commits show on the task); never change another branch or the repository's main checkout."
    )
}

#[cfg(test)]
mod tests;
