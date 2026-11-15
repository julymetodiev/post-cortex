// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! What the board works out for a task's answer (W24): its standing among
//! its board's tasks (what waits on it, ready, a plan's progress), how many
//! comments it has and the decision that holds it. Not stored.

use pcx_documents::Item;

use crate::domain::{self, Board, Comment, Decision, DecisionRules, Standing, Task};

/// A task with what the board works out for it.
#[derive(Debug, Clone, PartialEq)]
pub struct Placed {
    /// The task.
    pub task: Item<Task>,
    /// What waits on it, whether it is ready and, for a plan, its progress.
    pub standing: Standing,
    /// How many comments it has.
    pub comments: u32,
    /// What the decision that holds it decided; none: none holds it.
    pub held_by: Option<String>,
}

/// Comments and decisions, of one task or of every task.
#[derive(Debug, Default)]
pub struct Talk {
    /// The comments.
    pub comments: Vec<Item<Comment>>,
    /// The decisions, the newest first.
    pub decisions: Vec<Item<Decision>>,
}

/// `items` as the domain reads a board's tasks.
pub fn by_id(items: &[Item<Task>]) -> Vec<(&str, &Task)> {
    items
        .iter()
        .map(|t| (t.document.id.as_str(), &t.data))
        .collect()
}

/// `task` with its standing among `tasks` of `board`, its comments and the
/// decision that holds it among `talk`.
pub fn placed(board: &Board, task: Item<Task>, tasks: &[Item<Task>], talk: &Talk) -> Placed {
    let id = task.document.id.as_str();
    let standing = domain::standing(board, id, &task.data, &by_id(tasks));
    let comments = talk.comments.iter().filter(|c| c.data.task == id).count();
    let held_by = talk
        .decisions
        .iter()
        .find(|d| d.data.task == id && d.data.pending())
        .map(|d| d.data.what.clone());
    Placed {
        standing,
        // A count as the wire carries it, saturating (a task never has more).
        comments: u32::try_from(comments).unwrap_or(u32::MAX),
        held_by,
        task,
    }
}

/// `tasks` of `board` in its columns' order, each column by rank.
pub fn in_order<'a>(board: &Board, tasks: &'a [Item<Task>]) -> Vec<&'a Item<Task>> {
    let mut order = by_id(tasks);
    domain::order(&mut order);
    board
        .columns
        .iter()
        .flat_map(|column| {
            order
                .iter()
                .filter(move |(_, t)| &t.column == column)
                .filter_map(|(id, _)| tasks.iter().find(|t| t.document.id == *id))
        })
        .collect()
}

/// The tasks related to `task` among `tasks` of `board`, each placed.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Related {
    /// Its plan.
    pub plan: Option<Placed>,
    /// Its plan's tasks (or, for a plan, its own), in the board's order.
    pub plan_tasks: Vec<Placed>,
    /// The tasks it waits on.
    pub waits_on: Vec<Placed>,
    /// The tasks that wait on it.
    pub waited_on_by: Vec<Placed>,
}

/// The tasks related to `task` (placed as `placed` answers it) among
/// `tasks` of `board`.
pub fn related(board: &Board, task: &Placed, tasks: &[Item<Task>], talk: &Talk) -> Related {
    let id = task.task.document.id.as_str();
    let place = |item: &Item<Task>| placed(board, item.clone(), tasks, talk);
    let plan_id = match (&task.task.data.parent, &task.standing.progress) {
        (Some(parent), _) => Some(parent.as_str()),
        (None, Some(_)) => Some(id),
        (None, None) => None,
    };
    let ordered = in_order(board, tasks);
    Related {
        plan: task
            .task
            .data
            .parent
            .as_deref()
            .and_then(|parent| tasks.iter().find(|t| t.document.id == parent))
            .map(place),
        plan_tasks: plan_id.map_or_else(Vec::new, |plan| {
            ordered
                .iter()
                .filter(|t| t.data.parent.as_deref() == Some(plan))
                .map(|t| place(t))
                .collect()
        }),
        waits_on: task
            .task
            .data
            .depends_on
            .iter()
            .filter_map(|d| tasks.iter().find(|t| &t.document.id == d))
            .map(place)
            .collect(),
        waited_on_by: task
            .standing
            .waited_on_by
            .iter()
            .filter_map(|w| tasks.iter().find(|t| &t.document.id == w))
            .map(place)
            .collect(),
    }
}

/// `tasks` with `changed` in place of its earlier version (or added).
pub fn with(mut tasks: Vec<Item<Task>>, changed: &Item<Task>) -> Vec<Item<Task>> {
    match tasks
        .iter_mut()
        .find(|t| t.document.id == changed.document.id)
    {
        Some(old) => *old = changed.clone(),
        None => tasks.push(changed.clone()),
    }
    tasks
}

/// A plan or a person named on the wire: empty is none.
pub fn named(name: String) -> Option<String> {
    Some(name.trim().to_string()).filter(|n| !n.is_empty())
}
