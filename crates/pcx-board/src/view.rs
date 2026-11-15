// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The board's views (ARCHITECTURE section 8): its boards, with a button to
//! create one; a board's detail is its tasks as cards in lanes by column
//! (W24: one lane or one per plan, the plans' progress and a person as
//! filters, a card dragged to a column, a place in it and another plan),
//! with a button to add a task; a task's detail is its page ([`task`]).
//! The approvals: the workspace's decisions that wait for a person, and
//! those approved, to approve or revoke.

use pcx_module_api::view::build::{action, asking, bar, dialog, lines, node, source, stack, text};
use pcx_module_api::view::{Action, Body, Card, Component, Lanes, List, View, component};
use serde_json::{Value, json as value};

use crate::specs::path;

mod task;

fn list(
    source: pcx_module_api::view::Source,
    title: &str,
    subtitle: &str,
    actions: Vec<Action>,
    empty: &str,
    markdown: bool,
) -> Component {
    node(component::Kind::List(Box::new(List {
        source: Some(source),
        title_field: title.to_string(),
        subtitle_field: subtitle.to_string(),
        row_actions: actions,
        empty: empty.to_string(),
        detail: None,
        markdown,
    })))
}

/// The fields of a task a person writes, as a request nests them;
/// `column`: where it starts too.
fn task_fields(column: bool) -> Value {
    let mut fields = value!({
        "title": text("Task"),
        "text": text("What it is about"),
        "parent": text("Its plan (the plan's task id; empty: none)"),
        "assignee": text("Who works on it (a member's name; @name wakes it)"),
        "depends_on": lines("What it waits on (task ids)"),
        "priority": text("Priority (P0 to P3)"),
        "labels": lines("Labels")
    });
    if column {
        fields["column"] = text("Column (empty: the first)");
    }
    fields
}

/// A board's detail: a button to add a task, and its tasks as cards in
/// lanes; `{{row.*}}` is its `BoardRecord`.
fn board() -> Component {
    let add = dialog(
        "Add a task",
        task_fields(true),
        &["title"],
        None,
        action(
            "Add the task",
            &path("AddTask"),
            value!({ "board": "{{row.id}}", "title": "{{form.title}}", "text": "{{form.text}}",
                     "column": "{{form.column}}", "parent": "{{form.parent}}",
                     "assignee": "{{form.assignee}}", "depends_on": "{{form.depends_on}}",
                     "labels": "{{form.labels}}", "priority": "{{form.priority}}" }),
            None,
        ),
    );
    let tasks = |pointer: &str| {
        source(
            &path("ListTasks"),
            value!({ "board": "{{row.id}}" }),
            pointer,
        )
    };
    let lanes = node(component::Kind::Lanes(Box::new(Lanes {
        source: Some(tasks("/tasks")),
        lane_field: "task.column".to_string(),
        lanes: Some(source(
            &path("ListBoards"),
            value!({ "board": "{{row.id}}" }),
            "/boards/0/board/columns",
        )),
        title_field: "task.title".to_string(),
        subtitle_field: String::new(),
        row_actions: Vec::new(),
        empty: "No tasks.".to_string(),
        detail: Some(Box::new(task::page())),
        card: Some(Card {
            id_field: "id".to_string(),
            labels_field: "task.labels".to_string(),
            priority_field: "task.priority".to_string(),
            alert_field: "held_by".to_string(),
            cover_field: "task.attachments".to_string(),
            link_field: "task.links.pull_request".to_string(),
            person_field: "task.assignee".to_string(),
            count_field: "comments".to_string(),
            waits_field: "task.depends_on".to_string(),
            waited_by_field: "waited_on_by".to_string(),
        }),
        group_field: "task.parent".to_string(),
        groups: Some(tasks("/plans")),
        filter_field: "task.assignee".to_string(),
        r#move: Some(action(
            "Move",
            &path("MoveTask"),
            value!({ "task": "{{row.id}}", "version": "{{row.version}}",
                     "column": "{{drop.lane}}", "before": "{{drop.before}}",
                     "parent": "{{drop.group}}" }),
            None,
        )),
    })));
    stack(vec![bar(vec![add]), lanes])
}

/// The decisions of the workspace that wait for a person (`waiting`), or
/// were approved.
fn approvals(waiting: bool) -> Component {
    let decisions = source(
        &path("ListDecisions"),
        value!({ "waiting": waiting }),
        "/decisions",
    );
    let decided = value!({ "decision": "{{row.decision.id}}", "version": "{{row.decision.version}}",
                           "note": "{{form.note}}" });
    let note = value!({ "note": text("A note for the record (optional)") });
    let acts = if waiting {
        vec![
            asking(
                action(
                    "Approve",
                    &path("ApproveDecision"),
                    decided.clone(),
                    Some("Approve this decision? Its task moves again."),
                ),
                note.clone(),
                &[],
            ),
            asking(
                action(
                    "Decline",
                    &path("DeclineDecision"),
                    decided,
                    Some("Decline this decision? Who asked it gives up what it asked for."),
                ),
                note,
                &[],
            ),
        ]
    } else {
        vec![action(
            "Revoke",
            &path("RevokeApproval"),
            value!({ "decision": "{{row.decision.id}}", "version": "{{row.decision.version}}" }),
            Some("Revoke this approval? Its task is held again."),
        )]
    };
    let (subtitle, empty) = if waiting {
        (
            "decision.decision.held_because",
            "Nothing waits for you: every decision with a risk is approved.",
        )
    } else {
        (
            "decision.decision.approved_by",
            "No decision was approved yet.",
        )
    };
    list(
        decisions,
        "decision.decision.what",
        subtitle,
        acts,
        empty,
        false,
    )
}

/// The board's views.
pub fn views() -> Vec<View> {
    let new_board = dialog(
        "New board",
        value!({
            "name": text("Board's name"),
            "columns": lines("Columns, in order (the last is done)"),
            "repo": text("Repository of its tasks' branches (under tools-git's roots; optional)")
        }),
        &["name", "columns"],
        None,
        action(
            "Create the board",
            &path("CreateBoard"),
            value!({ "name": "{{form.name}}", "columns": "{{form.columns}}", "repo": "{{form.repo}}" }),
            None,
        ),
    );
    let boards = node(component::Kind::List(Box::new(List {
        source: Some(source(&path("ListBoards"), value!({}), "/boards")),
        title_field: "board.name".to_string(),
        subtitle_field: String::new(),
        row_actions: Vec::new(),
        empty: "No boards yet.".to_string(),
        detail: Some(Box::new(board())),
        markdown: false,
    })));
    vec![
        View {
            id: "board".to_string(),
            title: "Board".to_string(),
            description: "Boards, their tasks by column and plan, and each task's page."
                .to_string(),
            body: Some(Body::Root(Box::new(stack(vec![
                bar(vec![new_board]),
                boards,
            ])))),
        },
        View {
            id: "approvals".to_string(),
            title: "Approvals".to_string(),
            description: "Decisions with a risk that wait for a person, and those approved."
                .to_string(),
            body: Some(Body::Root(Box::new(stack(vec![
                approvals(true),
                approvals(false),
            ])))),
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    use pcx_module_api::Module;

    /// The views fit the module's RPCs, as the host checks them at load.
    #[test]
    fn the_views_fit_the_rpcs() {
        let module = crate::BoardModule::new();
        pcx_module_api::view::check(module.manifest()).unwrap();
        assert_eq!(views().len(), 2);
    }
}
