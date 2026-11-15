// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Plans, dependencies and order (ARCHITECTURE section 8, W24): a plan is
//! a task others name as their parent, one level deep; a task waits on
//! tasks of its board, never in a cycle; a task is ready when each task it
//! waits on is done or canceled (W26); a column sorts by rank. Every
//! rule reads the board's tasks, given as `(id, task)`.

use crate::domain::rank;
use crate::domain::{Board, Broken, Stage, Task, closes, stage};

/// A board's tasks, by id.
pub type Tasks<'a> = [(&'a str, &'a Task)];

/// A plan's progress: its done tasks, of those not canceled.
pub use post_cortex_proto::board::Progress;

/// What the board works out for a task from its board's tasks.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Standing {
    /// The tasks that wait on it.
    pub waited_on_by: Vec<String>,
    /// Whether every task it waits on is done or canceled.
    pub ready: bool,
    /// For a plan: its progress.
    pub progress: Option<Progress>,
}

fn find<'a>(tasks: &Tasks<'a>, id: &str) -> Option<&'a Task> {
    tasks.iter().find(|(i, _)| *i == id).map(|(_, t)| *t)
}

/// Whether task `id` (none: a new task) may have the plan `parent`.
///
/// # Errors
/// The task itself, a task not on the board, a plan that has a plan of
/// its own, or a task that is a plan itself (one level).
pub fn plan(id: Option<&str>, parent: &str, tasks: &Tasks<'_>) -> Result<(), Broken> {
    if id == Some(parent) {
        return Err(Broken::SelfLink);
    }
    let plan = find(tasks, parent).ok_or_else(|| Broken::NotOnBoard {
        task: parent.to_string(),
    })?;
    if plan.parent.is_some() {
        return Err(Broken::Nested {
            plan: parent.to_string(),
        });
    }
    if let Some(id) = id
        && tasks.iter().any(|(_, t)| t.parent.as_deref() == Some(id))
    {
        return Err(Broken::IsPlan {
            task: id.to_string(),
        });
    }
    Ok(())
}

/// The tasks task `id` (none: a new task) waits on, each once, in the
/// order given.
///
/// # Errors
/// The task itself, a task not on the board, or a link that makes a cycle
/// (naming the chain).
pub fn dependencies(
    id: Option<&str>,
    depends_on: &[String],
    tasks: &Tasks<'_>,
) -> Result<Vec<String>, Broken> {
    let mut kept: Vec<String> = Vec::with_capacity(depends_on.len());
    for dependency in depends_on {
        let dependency = dependency.trim();
        if id == Some(dependency) {
            return Err(Broken::SelfLink);
        }
        if find(tasks, dependency).is_none() {
            return Err(Broken::NotOnBoard {
                task: dependency.to_string(),
            });
        }
        if !kept.iter().any(|k| k == dependency) {
            kept.push(dependency.to_string());
        }
    }
    if let Some(id) = id {
        for dependency in &kept {
            if let Some(mut chain) = path(dependency, id, tasks, &mut Vec::new()) {
                chain.insert(0, id.to_string());
                return Err(Broken::Cycle { chain });
            }
        }
    }
    Ok(kept)
}

/// The chain of waits from `from` to `to`, if there is one.
fn path(from: &str, to: &str, tasks: &Tasks<'_>, seen: &mut Vec<String>) -> Option<Vec<String>> {
    if from == to {
        return Some(vec![to.to_string()]);
    }
    if seen.iter().any(|s| s == from) {
        return None;
    }
    seen.push(from.to_string());
    let task = find(tasks, from)?;
    task.depends_on.iter().find_map(|next| {
        path(next, to, tasks, seen).map(|mut chain| {
            chain.insert(0, from.to_string());
            chain
        })
    })
}

/// What `board` works out for task `id` from its `tasks`.
#[must_use]
pub fn standing(board: &Board, id: &str, task: &Task, tasks: &Tasks<'_>) -> Standing {
    let done = |t: &Task| stage(board, &t.column) == Stage::Done;
    let canceled = |t: &Task| stage(board, &t.column) == Stage::Canceled;
    let waited_on_by = tasks
        .iter()
        .filter(|(_, t)| t.depends_on.iter().any(|d| d == id))
        .map(|(i, _)| (*i).to_string())
        .collect();
    // A task it waits on that is gone is not done; one canceled is waited
    // on no longer.
    let ready = task
        .depends_on
        .iter()
        .all(|d| find(tasks, d).is_some_and(|t| closes(board, &t.column)));
    let children: Vec<&Task> = tasks
        .iter()
        .filter(|(_, t)| t.parent.as_deref() == Some(id))
        .map(|(_, t)| *t)
        .collect();
    let counted: Vec<&Task> = children.iter().copied().filter(|t| !canceled(t)).collect();
    let progress = (!children.is_empty()).then(|| Progress {
        done: count(counted.iter().filter(|t| done(t)).count()),
        total: count(counted.len()),
    });
    Standing {
        waited_on_by,
        ready,
        progress,
    }
}

/// A count as the wire carries it, saturating (a board never holds more
/// tasks).
fn count(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

/// `tasks` in their column's order: by rank, the tasks without one (written
/// before W24) after, then by id.
pub fn order(tasks: &mut [(&str, &Task)]) {
    tasks.sort_by(|(a_id, a), (b_id, b)| {
        (a.rank.is_empty(), &a.rank, a_id).cmp(&(b.rank.is_empty(), &b.rank, b_id))
    });
}

/// The rank of task `moving` (none: a new task) put in `column` (its tasks
/// among `tasks`) before task `before` (none: at the end). A place next to
/// a task without a rank is after the ranked ones.
///
/// # Errors
/// `before` is not in the column, or is the task moved.
pub fn place(
    column: &str,
    moving: Option<&str>,
    before: Option<&str>,
    tasks: &Tasks<'_>,
) -> Result<String, Broken> {
    if before.is_some() && before == moving {
        return Err(Broken::SelfLink);
    }
    let mut others: Vec<(&str, &Task)> = tasks
        .iter()
        .filter(|(id, t)| t.column == column && Some(*id) != moving)
        .copied()
        .collect();
    order(&mut others);
    let ranked: Vec<&str> = others
        .iter()
        .map(|(_, t)| t.rank.as_str())
        .filter(|r| !r.is_empty())
        .collect();
    let at = match before {
        None => ranked.len(),
        Some(before) => {
            let index = others
                .iter()
                .position(|(id, _)| *id == before)
                .ok_or_else(|| Broken::NotInColumn {
                    task: before.to_string(),
                    column: column.to_string(),
                })?;
            index.min(ranked.len())
        }
    };
    let low = at.checked_sub(1).map_or("", |i| ranked[i]);
    Ok(rank::between(low, ranked.get(at).copied()))
}

#[cfg(test)]
mod tests;
