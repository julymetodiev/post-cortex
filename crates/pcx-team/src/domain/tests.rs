// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use super::*;

use pcx_module_api::name::NAME_CHARS;

fn agent(name: &str, authority: Authority) -> Agent {
    Agent {
        name: name.into(),
        role: "dev".into(),
        authority: authority.into(),
        harness: true,
        skills: Vec::new(),
        model: None,
        modules: Vec::new(),
        tools: Vec::new(),
        reach: Vec::new(),
    }
}

#[test]
fn a_member_acts_within_its_authority_and_a_stranger_not_at_all() {
    let teams = vec![Team {
        name: "core".into(),
        members: vec!["ana".into(), "bot".into()],
    }];
    let agents = vec![
        agent("ana", Authority::Close),
        agent("bot", Authority::Discuss),
    ];
    assert_eq!(
        may(Some("ana"), None, None, Act::Close, &teams, &agents),
        Ok(())
    );
    assert_eq!(
        may(Some("bot"), None, None, Act::Decide, &teams, &agents),
        Err(Refused::Short {
            author: "bot".into(),
            has: Authority::Discuss,
            needs: Authority::Decide
        })
    );
    assert_eq!(
        may(Some("eve"), None, None, Act::Decide, &teams, &agents),
        Err(Refused::NotMember("eve".into()))
    );
    assert_eq!(
        may(None, None, None, Act::Decide, &teams, &agents),
        Err(Refused::NoAuthor)
    );
    // An agent document of someone outside every team is not membership.
    let outsider = vec![agent("eve", Authority::Close)];
    assert!(may(Some("eve"), None, None, Act::Close, &teams, &outsider).is_err());
    // Without a team, the board works as alone.
    assert_eq!(may(None, None, None, Act::Close, &[], &[]), Ok(()));
}

#[test]
fn an_owner_or_admin_of_the_organisation_always_decides_and_closes() {
    let teams = vec![Team {
        name: "core".into(),
        members: vec!["dev".into()],
    }];
    let agents = vec![agent("dev", Authority::Close)];
    for role in [PersonRole::Owner, PersonRole::Admin] {
        for act in [Act::Decide, Act::Close] {
            assert_eq!(
                may(Some("julius"), Some(role), None, act, &teams, &agents),
                Ok(())
            );
        }
    }
    for role in [PersonRole::Member, PersonRole::Viewer] {
        assert_eq!(
            may(
                Some("julius"),
                Some(role),
                None,
                Act::Close,
                &teams,
                &agents
            ),
            Err(Refused::NotMember("julius".into()))
        );
    }
}

#[test]
fn names_are_ones_a_mention_can_name() {
    for good in ["ana", "claude-code", "Bot_2.1"] {
        assert!(name(good).is_ok(), "{good}");
    }
    for bad in ["", "two words", "@ana", &"a".repeat(NAME_CHARS + 1)] {
        assert!(name(bad).is_err(), "{bad}");
    }
    assert!(Authority::Close > Authority::Decide && Authority::Decide > Authority::Discuss);
}

#[test]
fn a_module_asks_a_decision_in_its_own_name_but_closes_nothing() {
    let teams = vec![Team {
        name: "core".into(),
        members: vec!["ana".into()],
    }];
    let agents = Vec::new();
    let asked = may(
        Some("workflow:report"),
        None,
        Some("workflow"),
        Act::Decide,
        &teams,
        &agents,
    );
    assert_eq!(asked, Ok(()));
    assert!(
        may(
            Some("workflow:report"),
            None,
            Some("workflow"),
            Act::Close,
            &teams,
            &agents
        )
        .is_err()
    );
    assert!(
        may(
            Some("workflow:report"),
            None,
            Some("board"),
            Act::Decide,
            &teams,
            &agents
        )
        .is_err()
    );
    assert!(
        may(
            Some("workflow:report"),
            None,
            None,
            Act::Decide,
            &teams,
            &agents
        )
        .is_err()
    );
    assert!(
        may(
            Some("workflow:"),
            None,
            Some("workflow"),
            Act::Decide,
            &teams,
            &agents
        )
        .is_err()
    );
}
