// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use super::*;

use crate::domain::Authority;

fn member(name: &str, harness: bool) -> Agent {
    Agent {
        name: name.into(),
        role: "reviews code".into(),
        authority: Authority::Decide.into(),
        harness,
        skills: Vec::new(),
        model: None,
        modules: Vec::new(),
        tools: Vec::new(),
        reach: Vec::new(),
    }
}

fn members() -> Vec<Agent> {
    vec![
        member("dev", true),
        member("rev", true),
        member("julius", false),
    ]
}

#[test]
fn mentions_start_a_word_and_drop_a_full_stop() {
    assert_eq!(
        mentions("@dev please look, cc @rev. not mail@host @dev"),
        ["dev", "rev"]
    );
    assert!(mentions("no one").is_empty());
}

#[test]
fn an_assignment_wakes_the_new_assignee_once() {
    let assign = |was: Option<&str>, author: &str| Change::Task {
        task: "t1".into(),
        author: Some(author.into()),
        assignee: Some("dev".into()),
        was: was.map(str::to_string),
    };
    let woken = woken(&assign(None, "julius"), &members());
    assert_eq!(
        woken,
        [Wake {
            member: "dev".into(),
            task: "t1".into(),
            answers: None,
            why: Why::Assigned,
            reach: None,
        }]
    );
    // The same assignee again (the task moved), or its own change: nothing.
    assert!(super::woken(&assign(Some("dev"), "julius"), &members()).is_empty());
    assert!(super::woken(&assign(None, "dev"), &members()).is_empty());
}

#[test]
fn a_comment_wakes_the_members_it_names_or_answers_but_not_its_author_or_people() {
    let comment = Change::Comment {
        task: "t1".into(),
        comment: "c2".into(),
        author: Some("rev".into()),
        text: "@dev @rev @julius what now?".into(),
        answers: Some("dev".into()),
    };
    let woken: Vec<(String, Why)> = woken(&comment, &members())
        .into_iter()
        .map(|w| (w.member, w.why))
        .collect();
    assert_eq!(woken, [("dev".to_string(), Why::Mentioned)]);
}

#[test]
fn a_turn_is_told_its_role_skills_and_thread() {
    let told = instructions(
        &member("dev", true),
        &[("review".into(), "Read the diff.".into())],
    );
    assert!(told.contains("You are dev") && told.ends_with("## Skill: review\nRead the diff."));
    let asked = prompt(Why::Assigned, "t1", "ws-1", "Ship [todo]");
    assert!(asked.ends_with("Ship [todo]"));
    assert!(asked.contains("it is ws-1"), "{asked}");
    // A member is told what its authority leaves to others.
    let decide = Agent {
        authority: Authority::Decide.into(),
        ..member("dev", true)
    };
    assert!(instructions(&decide, &[]).contains(
        "closing a task (moving it into a done or canceled column) is a member's who may close"
    ));
    let close = Agent {
        authority: Authority::Close.into(),
        ..member("dev", true)
    };
    assert!(instructions(&close, &[]).contains("and close a task"));
    let told = instructions(&close, &[]);
    assert!(told.contains("only when you need them to act"), "{told}");
    assert!(told.contains("in the language the task"), "{told}");
    assert!(told.contains("(pcx:record/<entry_id>)"), "{told}");
    // One held to modules without the board is told nothing of it.
    let outside = Agent {
        modules: vec!["post-cortex.memory".into()],
        ..member("community", true)
    };
    assert_eq!(
        instructions(&outside, &[]),
        "You are community, a member of the team: reviews code."
    );
    let on_board = Agent {
        modules: vec!["board".into()],
        ..member("dev", true)
    };
    assert!(instructions(&on_board, &[]).contains("On the board you may"));
}

#[test]
fn a_persons_answer_wakes_the_member_it_answers_and_a_members_does_not() {
    let answer = |author: &str| Change::Comment {
        task: "t1".into(),
        comment: "c3".into(),
        author: Some(author.into()),
        text: "Looks right to me.".into(),
        answers: Some("dev".into()),
    };
    let by_person: Vec<(String, Why)> = woken(&answer("julius"), &members())
        .into_iter()
        .map(|w| (w.member, w.why))
        .collect();
    assert_eq!(by_person, [("dev".to_string(), Why::Answered)]);
    assert!(
        woken(&answer("rev"), &members()).is_empty(),
        "two members do not keep waking each other"
    );
}
