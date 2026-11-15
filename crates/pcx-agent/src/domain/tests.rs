// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use super::*;

use serde_json::json;

fn text(turn_id: &str, role: Role, parent: Option<&str>, said: &str) -> Turn {
    Turn::new(
        turn_id.into(),
        role,
        parent.map(str::to_string),
        vec![Block::Text { text: said.into() }],
    )
}

#[test]
fn the_conversation_follows_the_chain_back_from_the_newest_turn() {
    // b2 forks from a1, so b1 is not in its chain.
    let turns = vec![
        text("a1", Role::User, None, "first"),
        text("b1", Role::Assistant, None, "an answer left behind"),
        text("b2", Role::Assistant, Some("a1"), "the answer kept"),
        Turn {
            turn_id: "c1".into(),
            role: Role::User,
            text: "captured as text".into(),
            parent: None,
            blocks: Vec::new(),
            author: None,
        },
    ];
    let sent = conversation(&turns);
    let said: Vec<String> = sent.iter().map(|m| text_of(&m.blocks)).collect();
    assert_eq!(said, ["first", "the answer kept", "captured as text"]);
    assert!(conversation(&[]).is_empty());
}

#[test]
fn an_answer_with_tool_calls_runs_them_and_any_other_ends_the_turn() {
    let call = Block::ToolUse {
        id: "c1".into(),
        name: "fs_read".into(),
        input: json!({ "path": "a.rs" }),
    };
    let blocks = vec![
        Block::Text {
            text: "Reading".into(),
        },
        call,
    ];
    assert_eq!(
        next(&blocks, StopReason::ToolUse),
        Next::Tools(vec![Call {
            id: "c1".into(),
            name: "fs_read".into(),
            input: json!({ "path": "a.rs" })
        }])
    );
    assert_eq!(
        next(&blocks[..1], StopReason::EndTurn),
        Next::End(StopReason::EndTurn)
    );
    assert_eq!(
        next(&blocks[..1], StopReason::MaxTokens),
        Next::End(StopReason::MaxTokens)
    );
    let turn = Turn::new("a".into(), Role::Assistant, None, blocks);
    assert_eq!(
        (turn.text.as_str(), turn.tools()),
        ("Reading", vec!["fs_read".to_string()])
    );
}

#[test]
fn failures_are_retried_compacted_or_stop_by_their_kind() {
    let failed = |kind| ModelFailure::new(kind, "x");
    let mut recovery = Recovery::default();
    let waits: Vec<Recover> = (0..4)
        .map(|_| recovery.after(&failed(ModelFailureKind::Overloaded)))
        .collect();
    assert_eq!(
        waits,
        [
            Recover::Retry(Duration::from_secs(1)),
            Recover::Retry(Duration::from_secs(2)),
            Recover::Retry(Duration::from_secs(4)),
            Recover::Stop
        ]
    );

    let mut recovery = Recovery::default();
    let mut limited = failed(ModelFailureKind::RateLimited);
    limited.retry_after = Some(Duration::from_secs(90));
    assert_eq!(recovery.after(&limited), Recover::Retry(MAX_BACKOFF));

    let mut recovery = Recovery::default();
    let long = failed(ModelFailureKind::ContextTooLong);
    assert_eq!(recovery.after(&long), Recover::Compact);
    assert_eq!(recovery.after(&long), Recover::Stop);

    for kind in [
        ModelFailureKind::Unauthorized,
        ModelFailureKind::QuotaExhausted,
        ModelFailureKind::Invalid,
    ] {
        assert_eq!(Recovery::default().after(&failed(kind)), Recover::Stop);
    }
}

#[test]
fn the_conversation_starts_at_the_newest_summary() {
    let summary = Turn::new(
        "s1".into(),
        Role::User,
        Some("b1".into()),
        vec![Block::Summary {
            text: "they greeted".into(),
        }],
    );
    let turns = vec![
        text("a1", Role::User, None, "hi"),
        text("b1", Role::Assistant, None, "hello"),
        summary,
        text("a2", Role::User, Some("s1"), "go on"),
    ];
    let sent = conversation(&turns);
    let said: Vec<String> = sent.iter().map(|m| text_of(&m.blocks)).collect();
    assert_eq!(
        said,
        [format!("{SUMMARY_HEAD}\nthey greeted"), "go on".to_string()]
    );
}

fn failed(content: &str) -> Block {
    Block::ToolResult {
        tool_use_id: "1".to_string(),
        content: content.to_string(),
        is_error: true,
    }
}

#[test]
fn a_turn_stops_at_the_third_same_failure_in_a_row_and_a_success_does_not_reset_the_count() {
    let mut failures = Failures::default();
    assert_eq!(failures.see(&[failed("a")]), None);
    assert_eq!(failures.see(&[failed("a")]), None);
    assert_eq!(
        failures.see(&[failed("b")]),
        None,
        "another failure starts again"
    );
    assert_eq!(failures.see(&[failed("b")]), None);
    let ok = Block::ToolResult {
        tool_use_id: "2".to_string(),
        content: "fine".to_string(),
        is_error: false,
    };
    assert_eq!(failures.see(&[ok]), None);
    assert_eq!(failures.see(&[failed("b")]), Some("b".to_string()));
}

#[test]
fn a_turn_stops_at_the_most_failures_in_all() {
    let mut failures = Failures::default();
    for n in 0..ALL_FAILURES - 1 {
        assert_eq!(failures.see(&[failed(&format!("failure {n}"))]), None);
    }
    assert_eq!(failures.see(&[failed("last")]), Some("last".to_string()));
}

fn call(id: &str) -> Block {
    Block::ToolUse {
        id: id.to_string(),
        name: "bash".to_string(),
        input: serde_json::json!({}),
    }
}

fn blocks_turn(id: &str, role: Role, blocks: Vec<Block>) -> Turn {
    Turn {
        turn_id: id.to_string(),
        role,
        text: String::new(),
        parent: None,
        blocks,
        author: None,
    }
}

#[test]
fn a_call_the_system_stopped_under_is_answered_before_what_follows() {
    let turns = [
        blocks_turn("a", Role::Assistant, vec![call("1")]),
        text("b", Role::User, None, "go on"),
    ];
    let sent = conversation(&turns);
    assert_eq!(sent.len(), 2);
    assert!(matches!(
        &sent[1].blocks[0],
        Block::ToolResult { tool_use_id, is_error: true, .. } if tool_use_id == "1"
    ));
    assert_eq!(text_of(&sent[1].blocks[1..]), "go on");
}

#[test]
fn a_call_with_its_result_is_left_as_it_is() {
    let turns = [
        blocks_turn("a", Role::Assistant, vec![call("1")]),
        blocks_turn("b", Role::User, vec![failed("no")]),
    ];
    let sent = conversation(&turns);
    assert_eq!(sent.len(), 2);
    assert_eq!(sent[1].blocks.len(), 1);
}
