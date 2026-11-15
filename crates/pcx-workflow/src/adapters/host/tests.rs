// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use pcx_module_api::model::{ModelFailure, ModelFailureKind, StopReason, Usage};

use super::*;

fn ended(stop: TurnStop, answer: &str) -> TurnEnd {
    TurnEnd {
        stop,
        steps: 1,
        usage: Usage {
            input: 10,
            output: 5,
            ..Usage::default()
        },
        answer: answer.to_string(),
    }
}

#[test]
fn a_members_turn_is_asked_only_of_a_harness_agent_of_a_team() {
    use post_cortex_proto::team::{Agent, AgentRecord};
    let member = |name: &str, harness: bool| AgentRecord {
        agent: Some(Agent {
            name: name.into(),
            harness,
            ..Default::default()
        }),
        ..Default::default()
    };
    let teams = [
        Roster::default(),
        Roster {
            agents: vec![member("community", true), member("ana", false)],
            ..Default::default()
        },
    ];
    in_a_team(&teams, "community").unwrap();
    // Prod 2026-11-14: the template named a member no team had.
    let none = in_a_team(&[Roster::default()], "community").unwrap_err();
    assert!(none.starts_with("no member community in the workspace's teams"));
    assert!(none.contains("Teams"), "it says where a person adds it");
    let person = in_a_team(&teams, "ana").unwrap_err();
    assert!(person.contains("no agent of the harness"), "{person}");
}

#[test]
fn a_turn_the_model_answered_is_its_answer() {
    let said = said(ended(
        TurnStop::Answered(StopReason::EndTurn),
        "{\"kind\":\"abuse\"}",
    ))
    .unwrap();
    assert_eq!(said.answer, "{\"kind\":\"abuse\"}");
    assert_eq!(said.tokens, 15);
}

#[test]
fn a_turn_whose_model_failed_is_that_failure_not_an_empty_answer() {
    // As Z.AI refused a key with no plan (2026-11-14): code 1113.
    let quota = ModelFailure::new(
        ModelFailureKind::QuotaExhausted,
        "Insufficient balance or no resource package. Please recharge. (code 1113)",
    );
    let refused = said(ended(TurnStop::Failed(quota), "")).unwrap_err();
    assert_eq!(refused.kind, ModuleErrorKind::ResourceExhausted);
    assert!(refused.message.contains("code 1113"), "{}", refused.message);

    let blocked = said(ended(TurnStop::Blocked("a hook".into()), "")).unwrap_err();
    assert_eq!(blocked.kind, ModuleErrorKind::Denied);
    let steps = said(ended(TurnStop::StepLimit, "half")).unwrap_err();
    assert_eq!(steps.kind, ModuleErrorKind::ResourceExhausted);
    let cancelled = said(ended(TurnStop::Cancelled, "")).unwrap_err();
    assert_eq!(cancelled.kind, ModuleErrorKind::DeadlineExceeded);
}
