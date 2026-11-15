// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use super::*;

use post_cortex_proto::module::Rpc;

use crate::Manifest;
use crate::view::{Body, Column, Component, Format, Table, View, check, component};

/// The team's members, a `RosterList`.
const ROSTERS: &str = "/pcx.team.v1.Teams/ListTeams";

fn chat(agent: &str, member: &str) -> Component {
    Component {
        kind: Some(component::Kind::Chat(Chat {
            agent: agent.to_string(),
            member: member.to_string(),
            open: "Talk".to_string(),
        })),
    }
}

/// The members with `detail` for the picked one.
fn members(detail: Component) -> Component {
    Component {
        kind: Some(component::Kind::Table(Box::new(Table {
            source: Some(crate::view::Source {
                rpc: ROSTERS.to_string(),
                request: None,
                pointer: "/teams".to_string(),
            }),
            columns: vec![Column {
                field: "team.team.name".to_string(),
                label: "Team".to_string(),
                format: Format::Text.into(),
            }],
            row_actions: Vec::new(),
            empty: String::new(),
            detail: Some(Box::new(detail)),
        }))),
    }
}

/// The team module's manifest with `root`; `turns`: it runs turns.
fn team(root: Component, turns: bool) -> Manifest {
    Manifest {
        id: "team".to_string(),
        views: vec![View {
            id: "v".to_string(),
            title: "A view".to_string(),
            description: String::new(),
            body: Some(Body::Root(Box::new(root))),
        }],
        descriptors: post_cortex_proto::TEAM_DESCRIPTOR_SET.to_vec(),
        rpcs: vec![Rpc {
            method: ROSTERS.to_string(),
            tool: String::new(),
            visibility: Vec::new(),
            effect: post_cortex_proto::module::Effect::Read.into(),
        }],
        turns,
        ..Manifest::default()
    }
}

#[test]
fn a_chat_is_with_the_hosts_agent_or_the_modules_own_turns() {
    check(&team(chat("", ""), false)).unwrap();
    check(&team(
        members(chat("team", "{{row.agents.0.agent.name}}")),
        true,
    ))
    .unwrap();
    check(&team(chat("team", "dev"), true)).unwrap();

    let other = check(&team(chat("chief", ""), true)).unwrap_err();
    assert!(other.reason.contains("chief"), "{other}");
    let no_turns = check(&team(chat("team", ""), false)).unwrap_err();
    assert!(no_turns.reason.contains("no turns"), "{no_turns}");
    let agents_member = check(&team(chat("", "dev"), true)).unwrap_err();
    assert!(agents_member.reason.contains("member"), "{agents_member}");
    let row_outside = check(&team(chat("team", "{{row.name}}"), true)).unwrap_err();
    assert!(
        row_outside.reason.contains("nothing to stand for"),
        "{row_outside}"
    );
}
