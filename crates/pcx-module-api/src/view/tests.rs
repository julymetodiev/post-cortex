// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use super::*;

use serde_json::json;

use post_cortex_proto::module::Rpc;

use crate::Manifest;

/// The board's tasks: a `TaskList` of `TaskRecord`s, by board.
const ROWS: &str = "/pcx.board.v1.Boards/ListTasks";
/// A task changed: an `UpdateTaskRequest`.
const SAVE: &str = "/pcx.board.v1.Boards/UpdateTask";
/// A task's thread, by task.
const THREAD: &str = "/pcx.board.v1.Boards/GetTask";
/// A task moved: a `MoveTaskRequest`.
const MOVE: &str = "/pcx.board.v1.Boards/MoveTask";

fn source(rpc: &str, pointer: &str) -> Source {
    Source {
        rpc: rpc.to_string(),
        request: None,
        pointer: pointer.to_string(),
    }
}

fn action(rpc: &str, request: Value) -> Action {
    Action {
        label: "Do".to_string(),
        rpc: rpc.to_string(),
        request: request.as_object().cloned().map(json::object_to_wire),
        changes: false,
        confirm: String::new(),
        form: None,
        settings: false,
    }
}

fn kind(kind: component::Kind) -> Component {
    Component { kind: Some(kind) }
}

fn table(row_actions: Vec<Action>) -> Component {
    kind(component::Kind::Table(Box::new(Table {
        source: Some(source(ROWS, "/tasks")),
        columns: vec![Column {
            field: "task.title".to_string(),
            label: "Name".to_string(),
            format: Format::Text.into(),
        }],
        row_actions,
        empty: String::new(),
        detail: None,
    })))
}

fn form(submit: Action) -> Component {
    let schema = json!({ "type": "object", "properties": { "title": { "type": "string" } } });
    kind(component::Kind::Form(Form {
        schema: schema.as_object().cloned().map(json::object_to_wire),
        values: None,
        submit: Some(submit),
        open: String::new(),
    }))
}

fn view(id: &str, root: Component) -> View {
    View {
        id: id.to_string(),
        title: "A view".to_string(),
        description: String::new(),
        body: Some(Body::Root(Box::new(root))),
    }
}

/// The board module's manifest with `views`, its RPCs the three above.
fn manifest(views: Vec<View>) -> Manifest {
    Manifest {
        id: "board".to_string(),
        views,
        descriptors: post_cortex_proto::BOARD_DESCRIPTOR_SET.to_vec(),
        rpcs: [ROWS, SAVE, THREAD, MOVE]
            .map(|method| Rpc {
                method: method.to_string(),
                tool: String::new(),
                visibility: Vec::new(),
                effect: post_cortex_proto::module::Effect::Read.into(),
            })
            .to_vec(),
        ..Manifest::default()
    }
}

fn refused(views: Vec<View>) -> ViewError {
    check(&manifest(views)).unwrap_err()
}

#[test]
fn a_tree_over_the_modules_rpcs_passes() {
    let root = kind(component::Kind::Stack(Stack {
        direction: Direction::Column.into(),
        children: vec![
            table(vec![action(
                SAVE,
                json!({ "task": "{{row.id}}", "version": 1, "labels": { "labels": ["a"] } }),
            )]),
            form(action(SAVE, json!({ "title": "{{form.title}}" }))),
        ],
    }));
    check(&manifest(vec![view("board", root)])).unwrap();
}

#[test]
fn a_source_or_action_on_another_rpc_is_refused_where_it_is() {
    let root = kind(component::Kind::Stack(Stack {
        direction: Direction::Row.into(),
        children: vec![table(vec![]), table(vec![action("elsewhere", json!({}))])],
    }));
    let error = refused(vec![view("board", root)]);
    assert_eq!(error.at, "root.stack[1].table");
    assert!(error.reason.contains("elsewhere"), "{error}");
}

#[test]
fn substitutions_fit_where_they_are() {
    let row_in_form = refused(vec![view(
        "v",
        form(action(SAVE, json!({ "text": "{{row.id}}" }))),
    )]);
    assert!(row_in_form.reason.contains("{{row.id}}"), "{row_in_form}");
    let unknown_field = refused(vec![view(
        "v",
        form(action(SAVE, json!({ "text": "{{form.body}}" }))),
    )]);
    assert!(
        unknown_field.reason.contains("no field body"),
        "{unknown_field}"
    );
    // Not a substitution: kept as written.
    check(&manifest(vec![view(
        "v",
        form(action(SAVE, json!({ "text": "{{other.id}} and text" }))),
    )]))
    .unwrap();
}

/// A request's keys are fields of its message, at every depth; a pointer
/// walks the response's fields, a list's by index.
#[test]
fn requests_and_pointers_fit_the_messages() {
    let unknown = refused(vec![view(
        "v",
        table(vec![action(SAVE, json!({ "nope": 1 }))]),
    )]);
    assert!(
        unknown
            .reason
            .contains("nope is not a field of pcx.board.v1.UpdateTaskRequest"),
        "{unknown}"
    );
    let nested = refused(vec![view(
        "v",
        table(vec![action(SAVE, json!({ "labels": { "nope": [] } }))]),
    )]);
    assert!(nested.reason.contains("labels.nope"), "{nested}");

    let stat = |pointer: &str| {
        vec![view(
            "v",
            kind(component::Kind::Stat(Stat {
                label: "n".to_string(),
                source: Some(source(ROWS, pointer)),
                format: Format::Number.into(),
            })),
        )]
    };
    check(&manifest(stat("/tasks/0/task/labels/1"))).unwrap();
    check(&manifest(stat("/tasks/0/updated_at"))).unwrap();
    for (bad, why) in [
        ("count", "not a JSON Pointer"),
        (
            "/tasks/0/nope",
            "nope is not a field of pcx.board.v1.TaskRecord",
        ),
        ("/tasks/first", "first is not an index"),
        ("/tasks/0/version/x", "x is under a scalar"),
        ("/tasks/0/a~2", "not a JSON Pointer"),
    ] {
        let error = refused(stat(bad));
        assert!(error.reason.contains(why), "{bad}: {error}");
    }
}

#[test]
fn ids_enums_pointers_and_questions_are_checked() {
    let duplicate = refused(vec![view("v", table(vec![])), view("v", table(vec![]))]);
    assert!(duplicate.reason.contains("twice"), "{duplicate}");

    let no_direction = kind(component::Kind::Stack(Stack {
        direction: Direction::Unspecified.into(),
        children: vec![table(vec![])],
    }));
    assert!(
        refused(vec![view("v", no_direction)])
            .reason
            .contains("direction")
    );

    let mut silent = action(SAVE, json!({}));
    silent.changes = true;
    assert!(
        refused(vec![view("v", table(vec![silent]))])
            .reason
            .contains("asks nothing")
    );
}

#[test]
fn an_html_view_needs_ui_html_and_its_own_ui_uri() {
    let html = |uri: &str| View {
        id: "card".to_string(),
        title: "Card".to_string(),
        description: String::new(),
        body: Some(Body::ResourceUri(uri.to_string())),
    };
    let error = refused(vec![html("ui://board/card.html")]);
    assert!(error.reason.contains("ui_html"), "{error}");

    let mut granted = manifest(vec![html("ui://other/card.html")]);
    granted.capabilities = vec![Capability::UiHtml.into()];
    let error = check(&granted).unwrap_err();
    assert!(error.reason.contains("ui://board/"), "{error}");

    granted.views = vec![html("ui://board/card.html")];
    check(&granted).unwrap();
}

fn source_with(rpc: &str, request: Value) -> Source {
    Source {
        rpc: rpc.to_string(),
        request: request.as_object().cloned().map(json::object_to_wire),
        pointer: String::new(),
    }
}

/// Boards, a board's tasks in lanes, a task's thread: each detail's
/// sources and actions take the row picked above it.
fn boards(lane_field: &str, thread: Component) -> Component {
    let lanes = kind(component::Kind::Lanes(Box::new(Lanes {
        source: Some(source_with(ROWS, json!({ "board": "{{row.id}}" }))),
        lane_field: lane_field.to_string(),
        lanes: Some(source_with(ROWS, json!({ "board": "{{row.id}}" }))),
        title_field: "task.title".to_string(),
        subtitle_field: String::new(),
        row_actions: vec![action(SAVE, json!({ "task": "{{row.id}}" }))],
        empty: String::new(),
        detail: Some(Box::new(thread)),
        ..Lanes::default()
    })));
    kind(component::Kind::List(Box::new(List {
        source: Some(source(ROWS, "/tasks")),
        title_field: "task.board".to_string(),
        subtitle_field: String::new(),
        row_actions: Vec::new(),
        empty: String::new(),
        detail: Some(Box::new(lanes)),
        markdown: false,
    })))
}

#[test]
fn lanes_and_details_take_the_picked_row() {
    let thread = kind(component::Kind::Stack(Stack {
        direction: Direction::Column.into(),
        children: vec![
            kind(component::Kind::Markdown(Markdown {
                content: Some(Content::Source(source_with(
                    THREAD,
                    json!({ "task": "{{row.id}}" }),
                ))),
            })),
            form(action(
                SAVE,
                json!({ "task": "{{row.id}}", "text": "{{form.title}}" }),
            )),
        ],
    }));
    check(&manifest(vec![view(
        "board",
        boards("task.column", thread),
    )]))
    .unwrap();

    let unnamed = refused(vec![view("board", boards("", table(vec![])))]);
    assert_eq!(unnamed.at, "root.list.detail.lanes");
    assert!(unnamed.reason.contains("lane_field"), "{unnamed}");

    let outside = refused(vec![view(
        "v",
        kind(component::Kind::Markdown(Markdown {
            content: Some(Content::Source(source_with(
                THREAD,
                json!({ "task": "{{row.id}}" }),
            ))),
        })),
    )]);
    assert!(outside.reason.contains("{{row.id}}"), "{outside}");
}

/// A row action that asks for fields takes `{{form.<field>}}` of them and
/// the row's; a form's submit asks none of its own; the fields are of the
/// settings subset, a group's by their path.
#[test]
fn an_action_that_asks_for_fields_takes_them() {
    let fields = json!({ "type": "object", "properties": {
        "task": build::group("Task", json!({ "assignee": { "type": "string" } }), &["assignee"])
    } });
    let asking = |request: Value, schema: &Value| Action {
        form: schema.as_object().cloned().map(json::object_to_wire),
        ..action(SAVE, request)
    };
    check(&manifest(vec![view(
        "v",
        table(vec![asking(
            json!({ "task": "{{row.id}}", "assignee": "{{form.task.assignee}}" }),
            &fields,
        )]),
    )]))
    .unwrap();
    let unknown = refused(vec![view(
        "v",
        table(vec![asking(json!({ "text": "{{form.body}}" }), &fields)]),
    )]);
    assert!(unknown.reason.contains("no field body"), "{unknown}");
    let outside = refused(vec![view(
        "v",
        table(vec![asking(
            json!({}),
            &json!({ "type": "object", "properties": { "a": { "type": "string", "format": "uri" } } }),
        )]),
    )]);
    assert!(outside.reason.contains("format"), "{outside}");
    let nested = refused(vec![view(
        "v",
        form(asking(json!({ "text": "{{form.title}}" }), &fields)),
    )]);
    assert!(nested.reason.contains("asks no fields"), "{nested}");
}

#[test]
fn a_settings_action_has_a_label_only_on_a_module_with_settings() {
    let open = build::settings("Settings");
    let bar = || {
        kind(component::Kind::Stack(Stack {
            direction: Direction::Row.into(),
            children: vec![table(vec![build::settings("Settings")])],
        }))
    };
    let configured = Manifest {
        config_type: "example.Config".to_string(),
        ..manifest(vec![view("v", bar())])
    };
    check(&configured).unwrap();
    // A module without settings has none to open.
    let error = refused(vec![view("v", bar())]);
    assert!(error.reason.contains("has none"), "{error}");
    // A label only: no RPC, and never a form's submit.
    let with_rpc = Manifest {
        views: vec![view(
            "v",
            table(vec![Action {
                rpc: ROWS.into(),
                ..open.clone()
            }]),
        )],
        ..configured.clone()
    };
    assert!(check(&with_rpc).is_err());
    let submit = Manifest {
        views: vec![view("v", form(open))],
        ..configured
    };
    assert!(check(&submit).is_err());
}

#[test]
fn a_fields_component_reads_an_object_and_its_actions_fit() {
    let shown = build::fields(source(ROWS, "/tasks/0"), vec![action(ROWS, json!({}))]);
    check(&manifest(vec![view("v", shown)])).unwrap();
    let elsewhere = build::fields(source("/pcx.board.v1.Boards/Other", ""), Vec::new());
    assert!(refused(vec![view("v", elsewhere)]).reason.contains("Other"));
    let row = build::fields(
        source(ROWS, ""),
        vec![action(ROWS, json!({ "board": "{{row.id}}" }))],
    );
    assert!(
        check(&manifest(vec![view("v", row)])).is_err(),
        "no row outside a detail"
    );
}

/// Lanes moved by a drop: the move takes `{{drop.*}}` and the dropped row,
/// and needs a card with an id; groups need their source; images take
/// `{{images}}` in their attach only.
#[test]
fn a_move_takes_the_drop_and_groups_their_source() {
    let lanes = |r#move: Option<Action>, card: Option<Card>, group_field: &str| {
        kind(component::Kind::Lanes(Box::new(Lanes {
            source: Some(source(ROWS, "/tasks")),
            lane_field: "task.column".to_string(),
            lanes: Some(source(ROWS, "/tasks")),
            title_field: "task.title".to_string(),
            card,
            group_field: group_field.to_string(),
            groups: (!group_field.is_empty()).then(|| source(ROWS, "/tasks")),
            r#move,
            ..Lanes::default()
        })))
    };
    let card = Card {
        id_field: "id".to_string(),
        waits_field: "task.depends_on".to_string(),
        ..Card::default()
    };
    let drop = action(
        MOVE,
        json!({ "task": "{{row.id}}", "column": "{{drop.lane}}",
                "before": "{{drop.before}}", "parent": "{{drop.group}}" }),
    );
    check(&manifest(vec![view(
        "v",
        lanes(Some(drop.clone()), Some(card.clone()), "task.parent"),
    )]))
    .unwrap();
    let no_id = refused(vec![view("v", lanes(Some(drop.clone()), None, ""))]);
    assert!(no_id.reason.contains("id_field"), "{no_id}");
    let unknown = refused(vec![view(
        "v",
        lanes(
            Some(action(MOVE, json!({ "column": "{{drop.place}}" }))),
            Some(card.clone()),
            "",
        ),
    )]);
    assert!(unknown.reason.contains("{{drop.place}}"), "{unknown}");
    let outside = refused(vec![view(
        "v",
        table(vec![action(SAVE, json!({ "title": "{{drop.lane}}" }))]),
    )]);
    assert!(outside.reason.contains("nothing to stand for"), "{outside}");
    let waits_without_id = refused(vec![view(
        "v",
        lanes(
            None,
            Some(Card {
                id_field: String::new(),
                ..card
            }),
            "",
        ),
    )]);
    assert!(
        waits_without_id.reason.contains("id_field"),
        "{waits_without_id}"
    );

    let images = |attach: Value| {
        kind(component::Kind::Images(Images {
            source: Some(source(THREAD, "/task/task/attachments")),
            attach: Some(action(SAVE, attach)),
        }))
    };
    check(&manifest(vec![view(
        "v",
        images(json!({ "attachments": { "attachments": "{{images}}" } })),
    )]))
    .unwrap();
    let images_elsewhere = refused(vec![view(
        "v",
        table(vec![action(SAVE, json!({ "title": "{{images}}" }))]),
    )]);
    assert!(
        images_elsewhere.reason.contains("{{images}}"),
        "{images_elsewhere}"
    );
}
