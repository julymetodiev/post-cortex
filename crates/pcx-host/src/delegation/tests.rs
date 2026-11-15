// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! What a module's own call reaches: the tools an agent may call, and a
//! tool only a person calls when its author is a person its settings name
//! (W29).

use super::*;

#[test]
fn a_modules_call_reaches_a_person_tool_only_as_a_person_it_acts_for() {
    let persons = ["julius".to_string()];
    let as_person = |author: Option<&str>| Reach::Own {
        persons: &persons,
        author: author.map(str::to_string),
    };
    assert!(as_person(Some("julius")).allows("board", "approve_decision", false));
    assert!(!as_person(Some("eve")).allows("board", "approve_decision", false));
    assert!(!as_person(None).allows("board", "approve_decision", false));
    assert!(
        as_person(Some("eve")).allows("board", "comment", true),
        "an agent's tool"
    );
    let refused = as_person(Some("eve"))
        .admit("board", "approve_decision", false)
        .unwrap_err();
    assert_eq!(refused.kind, ModuleErrorKind::Denied);
    let nobody = Reach::Own {
        persons: &[],
        author: Some("julius".to_string()),
    };
    assert!(
        !nobody.allows("board", "approve_decision", false),
        "no grant, no person"
    );
}
