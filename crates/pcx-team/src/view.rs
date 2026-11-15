// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The team's view (ARCHITECTURE section 8): buttons to create a team and
//! add a member; the workspace's teams with the trust each has, to pause
//! or go on; a team's detail: buttons to add or remove a member and change
//! its trust (each a dialog), its members (each with an edit dialog of its
//! role, authority, skills and model, and a chat with it, W25), what they
//! work on now (W25), and its trust. Every change is a
//! person's: the view's RPCs are the person-only ones.

use pcx_module_api::view::build::{
    action, asking, bar, dialog, fields, group, lines, node, source, stack, text,
};
use pcx_module_api::view::{Body, Chat, Column, Component, Format, List, Table, View, component};
use serde_json::{Value, json as value};

use crate::autonomy::{Class, Mode};
use crate::domain::{Authority, MODULE_ID};
use crate::specs::path;

fn column(field: &str, label: &str) -> Column {
    shown(field, label, Format::Text)
}

fn shown(field: &str, label: &str, format: Format) -> Column {
    Column {
        field: field.to_string(),
        label: label.to_string(),
        format: format.into(),
    }
}

/// The values of an enum in a form, as the canonical proto JSON names
/// them.
fn names<T: Copy>(values: &[T], name: fn(&T) -> &'static str) -> Vec<&'static str> {
    values.iter().map(name).collect()
}

fn authority() -> Value {
    value!({ "type": "string", "title": "Authority", "enum": names(&[Authority::Discuss, Authority::Decide, Authority::Close], Authority::as_str_name),
             "description": "discuss: comments only; decide: records decisions; close: moves a task to a done or canceled column" })
}

/// What a person sets of a member, as an `AgentRecord` nests it; `new`:
/// its name and whether it is the harness's too.
fn member_fields(new: bool) -> Value {
    let mut agent = value!({
        "role": text("Role"),
        "authority": authority(),
        "skills": lines("Skills"),
        "model": text("Model (empty: the agent's)")
    });
    let mut required = vec!["role", "authority"];
    if new {
        agent["name"] = text("Name (the author it acts as; @name mentions it)");
        agent["harness"] = value!({ "type": "boolean", "title": "An agent of the harness (else Claude Code, Codex or a person)" });
        required.push("name");
    }
    value!({ "agent": group("Member", agent, &required) })
}

/// The members of the team in `{{row.*}}` (its `Roster`), each with its
/// edit dialog.
fn members() -> Component {
    let edit = asking(
        action(
            "Edit",
            &path("UpdateAgent"),
            value!({ "name": "{{row.agent.name}}", "version": "{{row.version}}", "role": "{{form.agent.role}}",
                     "authority": "{{form.agent.authority}}", "skills": { "names": "{{form.agent.skills}}" },
                     "model": "{{form.agent.model}}" }),
            None,
        ),
        member_fields(false),
        &["agent"],
    );
    node(component::Kind::Table(Box::new(Table {
        source: Some(source(
            &path("ListTeams"),
            value!({ "team": "{{row.team.team.name}}" }),
            "/teams/0/agents",
        )),
        columns: vec![
            column("agent.name", "Member"),
            column("agent.role", "Role"),
            column("agent.authority", "Authority"),
            column("agent.harness", "Agent of the harness"),
        ],
        row_actions: vec![edit],
        empty: "No members yet.".to_string(),
        detail: Some(Box::new(talk())),
    })))
}

/// A person's chat with the member in `{{row.*}}` (W25), beside the view.
fn talk() -> Component {
    node(component::Kind::Chat(Chat {
        agent: MODULE_ID.to_string(),
        member: "{{row.agent.name}}".to_string(),
        open: "Talk".to_string(),
    }))
}

/// What the members of the team in `{{row.*}}` work on now (W25).
fn work() -> Component {
    node(component::Kind::Table(Box::new(Table {
        source: Some(source(
            &path("ListWork"),
            value!({ "team": "{{row.team.team.name}}" }),
            "/work",
        )),
        columns: vec![
            column("agent", "Member"),
            column("title", "Task"),
            column("column", "Column"),
            shown("busy", "Working now", Format::YesNo),
            shown("turns", "Turns", Format::Number),
            shown("updated_at", "Last turn", Format::Time),
        ],
        row_actions: vec![action(
            "Stop",
            &path("StopWork"),
            value!({ "task": "{{row.task}}", "member": "{{row.agent}}" }),
            Some("Stop this member's running turn on the task?"),
        )],
        empty: "No member has worked on a task yet.".to_string(),
        detail: None,
    })))
}

/// The dialog that changes the trust of the team in `{{row.*}}`, from the
/// trust it has.
fn change_trust() -> Component {
    dialog(
        "Change the trust",
        value!({
            "mode": { "type": "string", "title": "Trust", "enum": names(&[Mode::Supervised, Mode::Classes, Mode::Full], Mode::as_str_name),
                      "description": "supervised: a person approves every decision with a risk; classes: the classes below approve themselves within their limits; full: every class within its limits" },
            "classes": { "type": "array", "title": "Classes (for classes)", "items": { "type": "string", "enum": names(&[Class::Code, Class::Money, Class::Data], Class::as_str_name) } },
            "code": { "type": "object", "title": "Code", "properties": {
                "branches": lines("Branch prefixes it may change, e.g. feature/"),
                "protected": lines("Branches it never changes, e.g. main")
            } },
            "money": { "type": "object", "title": "Money", "properties": {
                "currency": text("Currency"),
                "per_decision": { "type": "integer", "title": "Most per decision" },
                "per_day": { "type": "integer", "title": "Most per day" }
            } },
            "data": { "type": "object", "title": "Data", "properties": {
                "deletions": { "type": "boolean", "title": "Deletions too (else reversible changes only)" }
            } }
        }),
        &["mode"],
        Some(source(
            &path("GetAutonomy"),
            value!({ "team": "{{row.team.team.name}}" }),
            "/autonomy/trust",
        )),
        action(
            "Set the trust",
            &path("SetAutonomy"),
            value!({
                "team": "{{row.team.team.name}}",
                "version": "{{row.autonomy.version}}",
                "trust": {
                    "mode": "{{form.mode}}",
                    "classes": "{{form.classes}}",
                    "code": "{{form.code}}",
                    "money": "{{form.money}}",
                    "data": "{{form.data}}"
                }
            }),
            Some("Give the team this trust? Its decisions inside it are approved at once."),
        ),
    )
}

/// A team's detail: `{{row.*}}` is its `Roster`.
fn team() -> Component {
    let member = |open: &str, label: &str, method: &str, confirm: Option<&str>| {
        dialog(
            open,
            value!({ "member": text("Member's name") }),
            &["member"],
            None,
            action(
                label,
                &path(method),
                value!({ "team": "{{row.team.team.name}}", "version": "{{row.team.version}}", "member": "{{form.member}}" }),
                confirm,
            ),
        )
    };
    let trust = fields(
        source(
            &path("ListTeams"),
            value!({ "team": "{{row.team.team.name}}" }),
            "/teams/0/autonomy/autonomy",
        ),
        Vec::new(),
    );
    stack(vec![
        bar(vec![
            member("Add a member", "Add to the team", "AddMember", None),
            member(
                "Remove a member",
                "Remove from the team",
                "RemoveMember",
                Some("Remove this member from the team?"),
            ),
            change_trust(),
        ]),
        members(),
        work(),
        trust,
    ])
}

/// The team's views.
pub fn views() -> Vec<View> {
    let pause = |label: &str, paused: bool, confirm: &str| {
        action(
            label,
            &path("PauseAutonomy"),
            value!({ "team": "{{row.team.team.name}}", "version": "{{row.autonomy.version}}", "paused": paused }),
            Some(confirm),
        )
    };
    let teams = node(component::Kind::List(Box::new(List {
        source: Some(source(&path("ListTeams"), value!({}), "/teams")),
        title_field: "team.team.name".to_string(),
        subtitle_field: "autonomy.autonomy.trust.mode".to_string(),
        row_actions: vec![
            pause(
                "Pause",
                true,
                "Pause the team's trust? Every decision with a risk waits for a person.",
            ),
            pause("Go on", false, "Give the team its trust again?"),
        ],
        empty: "No teams yet.".to_string(),
        detail: Some(Box::new(team())),
        markdown: false,
    })));
    let add_member = dialog(
        "New member",
        member_fields(true),
        &["agent"],
        None,
        action(
            "Add the member",
            &path("AddAgent"),
            value!({ "agent": { "name": "{{form.agent.name}}", "role": "{{form.agent.role}}",
                                "authority": "{{form.agent.authority}}", "harness": "{{form.agent.harness}}",
                                "skills": "{{form.agent.skills}}", "model": "{{form.agent.model}}" } }),
            None,
        ),
    );
    let add_team = dialog(
        "New team",
        value!({ "name": text("Team's name"), "members": lines("Members, by name") }),
        &["name"],
        None,
        action(
            "Create the team",
            &path("CreateTeam"),
            value!({ "name": "{{form.name}}", "members": "{{form.members}}" }),
            None,
        ),
    );
    vec![View {
        id: "team".to_string(),
        title: "Team".to_string(),
        description: "The workspace's teams, their members' roles and authority, and the trust each team has."
            .to_string(),
        body: Some(Body::Root(Box::new(stack(vec![
            bar(vec![add_team, add_member]),
            teams,
        ])))),
    }]
}

#[cfg(test)]
mod tests {
    use super::*;

    use pcx_module_api::Module;

    /// The view fits the module's RPCs, as the host checks it at load.
    #[test]
    fn the_view_fits_the_rpcs() {
        let module = crate::TeamModule::new();
        pcx_module_api::view::check(module.manifest()).unwrap();
        assert_eq!(views().len(), 1);
    }
}
