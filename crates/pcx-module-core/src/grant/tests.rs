// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use super::*;

/// An agent may call it.
const AGENTS: bool = true;
/// Only a person calls it.
const PERSON: bool = false;

#[test]
fn no_grant_gives_the_agents_tools_only() {
    let grant = TurnGrant::default();
    assert!(grant.allows("comment", AGENTS));
    assert!(grant.allows("search", AGENTS));
    assert!(!grant.allows("team_create", PERSON));
}

#[test]
fn a_person_only_tool_passes_by_name_and_a_withheld_one_never() {
    let grant = TurnGrant::from(wire::TurnGrant {
        person_tools: vec!["team_create".into(), "revoke_approval".into()],
        withheld: vec!["revoke_approval".into(), "comment".into()],
    });
    assert!(grant.allows("team_create", PERSON));
    assert!(!grant.allows("autonomy_set", PERSON));
    assert!(!grant.allows("revoke_approval", PERSON));
    assert!(!grant.allows("comment", AGENTS));
}
