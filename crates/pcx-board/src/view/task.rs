// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! A task's page (ARCHITECTURE section 8, W24): buttons to edit, move and
//! decide on it (each a dialog), its text, its images (drop, paste or
//! choose), what it waits on and what waits on it, its plan's tasks, its
//! comments and decisions (a person approves one, with a note), a box to
//! comment, and its details. `{{row.*}}` is its `TaskRecord`.

use pcx_module_api::view::build::{action, asking, bar, dialog, form, node, source, stack, text};
use pcx_module_api::view::{Component, Fields, Images, Markdown, Source, component, markdown};
use serde_json::json as value;

use crate::domain::Risk;
use crate::specs::path;
use crate::view::{list, task_fields};

/// The thread of the task in `{{row.*}}`, at `pointer`.
fn thread(pointer: &str) -> Source {
    source(&path("GetTask"), value!({ "task": "{{row.id}}" }), pointer)
}

/// The tasks of the thread at `pointer`, titled with their column.
fn tasks(pointer: &str, empty: &str) -> Component {
    list(
        thread(pointer),
        "task.title",
        "task.column",
        Vec::new(),
        empty,
        false,
    )
}

fn edit() -> Component {
    let mut fields = task_fields(false);
    fields["branch"] = text("Its branch in the board's repository (links its commits)");
    fields["pull_request"] = text("Its pull request");
    dialog(
        "Edit",
        fields,
        &["title"],
        Some(thread("/task")),
        action(
            "Save the task",
            &path("UpdateTask"),
            value!({ "task": "{{row.id}}", "version": "{{row.version}}", "title": "{{form.title}}",
                     "text": "{{form.text}}", "parent": "{{form.parent}}",
                     "assignee": "{{form.assignee}}",
                     "depends_on": { "tasks": "{{form.depends_on}}" },
                     "labels": { "labels": "{{form.labels}}" },
                     "priority": "{{form.priority}}", "branch": "{{form.branch}}",
                     "pull_request": "{{form.pull_request}}" }),
            None,
        ),
    )
}

fn decide() -> Component {
    dialog(
        "Decide",
        value!({
            "what": text("What was decided"),
            "why": text("Why"),
            "risk": { "type": "string", "title": "What it risks", "enum": ([Risk::None, Risk::Code, Risk::Money, Risk::Data].map(|r| r.as_str_name())) },
            "amount": { "type": "integer", "title": "For money: how much" },
            "currency": text("For money: the currency"),
            "branch": text("For code: the branch it changes"),
            "reversible": { "type": "boolean", "title": "For data: it can be undone" }
        }),
        &["what", "why", "risk"],
        None,
        action(
            "Record the decision",
            &path("Decide"),
            value!({ "task": "{{row.id}}", "what": "{{form.what}}", "why": "{{form.why}}",
                     "risk": "{{form.risk}}",
                     "stakes": { "amount": "{{form.amount}}", "currency": "{{form.currency}}",
                                 "branch": "{{form.branch}}", "reversible": "{{form.reversible}}" } }),
            Some("Record this decision? One with a risk holds the task until it is approved."),
        ),
    )
}

/// A task's page.
pub fn page() -> Component {
    let mv = dialog(
        "Move",
        value!({ "column": text("To the column") }),
        &["column"],
        None,
        action(
            "Move the task",
            &path("MoveTask"),
            value!({ "task": "{{row.id}}", "version": "{{row.version}}", "column": "{{form.column}}" }),
            None,
        ),
    );
    let images = node(component::Kind::Images(Images {
        source: Some(thread("/task/task/attachments")),
        attach: Some(action(
            "Attach",
            &path("UpdateTask"),
            value!({ "task": "{{row.id}}", "version": "{{row.version}}",
                     "attachments": { "attachments": "{{images}}" } }),
            None,
        )),
    }));
    let decided =
        value!({ "decision": "{{row.id}}", "version": "{{row.version}}", "note": "{{form.note}}" });
    let note = value!({ "note": text("A note for the record (optional)") });
    let approve = asking(
        action(
            "Approve",
            &path("ApproveDecision"),
            decided.clone(),
            Some("Approve this decision? Its task moves again."),
        ),
        note.clone(),
        &[],
    );
    let decline = asking(
        action(
            "Decline",
            &path("DeclineDecision"),
            decided,
            Some("Decline this decision? Who asked it gives up what it asked for."),
        ),
        note,
        &[],
    );
    let comment = form(
        value!({ "text": text("Comment") }),
        &["text"],
        action(
            "Comment",
            &path("AddComment"),
            value!({ "task": "{{row.id}}", "text": "{{form.text}}" }),
            None,
        ),
    );
    let details = node(component::Kind::Fields(Fields {
        source: Some(thread("/task/task")),
        actions: Vec::new(),
    }));
    stack(vec![
        bar(vec![edit(), mv, decide()]),
        node(component::Kind::Markdown(Markdown {
            content: Some(markdown::Content::Source(thread("/task/text"))),
        })),
        images,
        tasks("/waits_on", "It waits on nothing: it can start."),
        tasks("/waited_on_by", "Nothing waits on it."),
        tasks("/plan_tasks", "It is in no plan."),
        list(
            thread("/comments"),
            "text",
            "author",
            Vec::new(),
            "No comments yet.",
            true,
        ),
        list(
            thread("/decisions"),
            "decision.what",
            "decision.why",
            vec![approve, decline],
            "No decisions yet.",
            false,
        ),
        comment,
        details,
    ])
}
