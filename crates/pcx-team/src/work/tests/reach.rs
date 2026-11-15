// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! A member's reach (docs/connect.md section 4a): the workspaces its turn
//! reaches are those it and who woke it both reach.

use super::*;

#[test]
fn a_members_turn_reaches_what_it_and_who_woke_it_both_reach() {
    let names = |list: &[&str]| list.iter().map(|w| (*w).to_string()).collect::<Vec<_>>();
    let member = |reach: &[&str]| Agent {
        name: "guide".into(),
        reach: names(reach),
        ..Agent::default()
    };
    // A member that names none reaches what who woke it does.
    assert_eq!(held(&scope(), &member(&[])).unwrap().reach, None);
    let woken = Scope {
        reach: Some(names(&["ws-1", "ws-2"])),
        ..scope()
    };
    assert_eq!(held(&woken, &member(&[])).unwrap().reach, woken.reach);
    // Its own, when who woke it reaches every one; else what both name.
    assert_eq!(
        held(&scope(), &member(&["ws-1"])).unwrap().reach,
        Some(names(&["ws-1"]))
    );
    assert_eq!(
        held(&woken, &member(&["ws-1", "ws-3"])).unwrap().reach,
        Some(names(&["ws-1"]))
    );
    // Its turn runs in a workspace it reaches (the scope's is ws-1).
    for reach in [&["ws-9"][..], &["ws-2"]] {
        let refused = held(&woken, &member(reach)).unwrap_err();
        assert_eq!(refused.kind, ModuleErrorKind::FailedPrecondition);
        assert!(refused.message.contains("guide"), "{}", refused.message);
    }
}
