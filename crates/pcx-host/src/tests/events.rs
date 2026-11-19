// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Events: fail-open failures skipped, fail-closed ones blocking the turn,
//! any module's block answer blocking,
//! context merged in load order, and no event to a module that did not
//! subscribe.

use super::*;

#[tokio::test]
async fn fail_open_failures_are_skipped_and_context_merges_in_load_order() {
    let host = host(vec![
        fake("a", ModuleKind::Tool, Act::Answer, FailurePolicy::FailOpen),
        fake("b", ModuleKind::Tool, Act::Hang, FailurePolicy::FailOpen),
        fake(
            "c",
            ModuleKind::Tool,
            Act::Answer,
            FailurePolicy::Unspecified,
        ),
    ])
    .await
    .unwrap();
    let dispatched = host
        .dispatch(
            &scope(Duration::from_secs(5)),
            EventKind::TurnStart,
            &prompt(),
        )
        .await;
    assert_eq!(dispatched.outcome.decision, "");
    let added = dispatched.outcome.hook_specific_output.unwrap();
    assert_eq!(added.additional_context, "from a\n\nfrom c");
    assert_eq!(added.hook_event_name, "UserPromptSubmit");
    assert_eq!(dispatched.failures.len(), 1);
    assert_eq!(dispatched.failures[0].module, "b");
    assert!(!dispatched.failures[0].blocked);
}

#[tokio::test]
async fn a_fail_closed_failure_blocks_the_turn() {
    let host = host(vec![fake(
        "guard",
        ModuleKind::Tool,
        Act::Fail,
        FailurePolicy::FailClosed,
    )])
    .await
    .unwrap();
    let dispatched = host
        .dispatch(
            &scope(Duration::from_secs(5)),
            EventKind::TurnStart,
            &prompt(),
        )
        .await;
    assert_eq!(dispatched.outcome.decision, "block");
    assert!(dispatched.outcome.reason.contains("guard"));
    assert!(dispatched.failures[0].blocked);
}

#[tokio::test]
async fn any_module_may_block_by_its_answer() {
    let host = host(vec![
        fake(
            "loud",
            ModuleKind::Tool,
            Act::Block,
            FailurePolicy::FailOpen,
        ),
        fake(
            "guard",
            ModuleKind::Tool,
            Act::Block,
            FailurePolicy::FailClosed,
        ),
    ])
    .await
    .unwrap();
    let dispatched = host
        .dispatch(
            &scope(Duration::from_secs(5)),
            EventKind::TurnStart,
            &prompt(),
        )
        .await;
    assert_eq!(dispatched.outcome.decision, "block");
    let reasons: Vec<&str> = dispatched.outcome.reason.lines().collect();
    assert_eq!(reasons.len(), 2, "{reasons:?}");
    assert!(reasons[0].contains("loud") && reasons[1].contains("guard"));

    // The failure policy is about failures: a fail-open module's answer
    // blocks as well (memory holds a file's first edit once).
    let open_only = host_with_one_open_blocker().await;
    let dispatched = open_only
        .dispatch(
            &scope(Duration::from_secs(5)),
            EventKind::TurnStart,
            &prompt(),
        )
        .await;
    assert_eq!(dispatched.outcome.decision, "block");
    assert!(dispatched.failures.is_empty());
}

async fn host_with_one_open_blocker() -> Host {
    host(vec![fake(
        "loud",
        ModuleKind::Tool,
        Act::Block,
        FailurePolicy::FailOpen,
    )])
    .await
    .unwrap()
}

#[tokio::test]
async fn unsubscribed_modules_get_no_event() {
    let host = host(vec![tool(Act::Panic)]).await.unwrap();
    let dispatched = host
        .dispatch(
            &scope(Duration::from_secs(5)),
            EventKind::TurnEnd,
            &prompt(),
        )
        .await;
    assert_eq!(dispatched.outcome, Outcome::default());
    assert!(dispatched.failures.is_empty());
    assert_eq!(host.health()[0].calls, 0);
}
