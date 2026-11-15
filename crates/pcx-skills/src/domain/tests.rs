// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use super::*;

#[test]
fn a_skill_has_a_name_of_its_charset_and_text_in_its_fields() {
    let (skill, instructions) = skill(
        "review-code",
        " Review a change ",
        "Read the diff.\n",
        vec![" fs_read ".into(), "git_diff".into(), "fs_read".into()],
    )
    .unwrap();
    assert_eq!(skill.description, "Review a change");
    assert_eq!(skill.tools, ["fs_read", "git_diff"]);
    assert_eq!(instructions, "Read the diff.");

    for bad in ["", "Review", "a b", "ревю", &"a".repeat(NAME_CHARS + 1)] {
        assert_eq!(name(bad), Err(Broken::Name(bad.to_string())), "{bad}");
    }
    assert!(name(&"a".repeat(NAME_CHARS)).is_ok());
    assert_eq!(
        super::skill("x", "", "do", Vec::new()).unwrap_err(),
        Broken::Empty("description")
    );
    assert_eq!(
        super::skill("x", "d", " ", Vec::new()).unwrap_err(),
        Broken::Empty("instructions")
    );
}

#[test]
fn a_revision_keeps_the_name_and_what_it_does_not_change() {
    let (current, text) = skill("plan", "Plan", "Think first.", vec!["bash".into()]).unwrap();
    let (revised, instructions) = revise(
        &current,
        &text,
        Revision {
            tools: Some(Vec::new()),
            ..Revision::default()
        },
    )
    .unwrap();
    assert_eq!(
        (revised.name.as_str(), revised.description.as_str()),
        ("plan", "Plan")
    );
    assert!(revised.tools.is_empty());
    assert_eq!(instructions, "Think first.");
    let empty = revise(
        &current,
        &text,
        Revision {
            instructions: Some(String::new()),
            ..Revision::default()
        },
    );
    assert_eq!(empty.unwrap_err(), Broken::Empty("instructions"));
}

#[test]
fn its_prompt_is_its_instructions_then_its_tools() {
    let (with, text) = skill("plan", "Plan", "Think first.", vec!["bash".into()]).unwrap();
    assert_eq!(
        prompt(&with, &text),
        "Think first.\n\nTools this skill needs: bash."
    );
    let (without, text) = skill("plan", "Plan", "Think first.", Vec::new()).unwrap();
    assert_eq!(prompt(&without, &text), "Think first.");
}

#[test]
fn a_built_in_skill_is_a_front_matter_then_its_instructions() {
    let (skill, instructions) =
        shipped("---\nname: plan\ndescription: Plan it\ntools: a, b\n---\nThink.\n").unwrap();
    assert_eq!(skill.name, "plan");
    assert_eq!(skill.tools, ["a", "b"]);
    assert_eq!(instructions, "Think.");
    assert!(matches!(
        shipped("no front matter"),
        Err(Broken::Shipped(_))
    ));
    assert!(matches!(
        shipped("---\nname: plan\ncolour: red\n---\nThink."),
        Err(Broken::Shipped(_))
    ));
    for file in LIBRARY {
        shipped(file).unwrap();
    }
}
