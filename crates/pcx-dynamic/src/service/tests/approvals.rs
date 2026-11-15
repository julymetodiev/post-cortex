// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! What the board answers: approvals heard or read again, and the decisions
//! no one waits on withdrawn.

use post_cortex_proto::board::DecisionDeclined;

use super::*;

#[tokio::test]
async fn a_team_policy_approves_at_once_and_an_approval_missed_is_read_again() {
    service!(world, docs, settings, service);
    *world.policy.lock() = Some("policy v1".into());
    let installed = service.install(&scope(), request("a", &[])).await.unwrap();
    assert_eq!(
        installed.module.unwrap().approved.unwrap().approved_by,
        "policy v1"
    );
    assert_eq!(world.changed.lock().len(), 1);

    *world.policy.lock() = None;
    service.install(&scope(), request("b", &[])).await.unwrap();
    *world.thread.lock() = Thread {
        decisions: vec![DecisionRecord {
            id: "decision-2".into(),
            decision: Some(Decision {
                approved_by: Some("ana".into()),
                ..Default::default()
            }),
            ..Default::default()
        }],
        ..Default::default()
    };
    service.recheck().await.unwrap();
    let got = service.get("greet").await.unwrap().module.unwrap();
    assert_eq!(got.approved.unwrap().approved_by, "ana");
    assert_eq!(world.changed.lock().len(), 2);
}

#[tokio::test]
async fn a_version_asked_about_no_more_has_its_decision_withdrawn() {
    service!(world, docs, settings, service);
    service.install(&scope(), request("a", &[])).await.unwrap();
    // New files while the first wait: those are asked about instead.
    let b = service.install(&scope(), request("b", &[])).await.unwrap();
    assert_eq!(b.module.unwrap().asked.unwrap().decision, "decision-2");
    assert_eq!(world.withdrawn(), [by_greet("decision-1")]);
    // The same files again: the same decision waits.
    service.install(&scope(), request("b", &[])).await.unwrap();
    assert_eq!(world.questions.lock().len(), 2);
    assert_eq!(world.withdrawn().len(), 1);

    service
        .decided(&DecisionApproved {
            decision: "decision-2".into(),
            task: "task-1".into(),
            approved_by: "ana".into(),
        })
        .await
        .unwrap();
    service.install(&scope(), request("c", &[])).await.unwrap();
    assert_eq!(world.withdrawn().len(), 1);
    // Back to the approved files: nothing is asked, and "c" waits no more.
    let back = service.install(&scope(), request("b", &[])).await.unwrap();
    assert!(back.module.unwrap().asked.is_none());
    assert_eq!(world.questions.lock().len(), 3);
    assert_eq!(
        world.withdrawn(),
        [by_greet("decision-1"), by_greet("decision-3")]
    );
}

#[tokio::test]
async fn files_a_person_declined_do_not_run_and_are_asked_about_when_written_again() {
    service!(world, docs, settings, service);
    service.install(&scope(), request("a", &[])).await.unwrap();
    service
        .declined(&DecisionDeclined {
            decision: "decision-1".into(),
            task: "task-1".into(),
            declined_by: "ana".into(),
        })
        .await
        .unwrap();
    let got = service.get("greet").await.unwrap().module.unwrap();
    assert!(got.approved.is_none());
    assert_eq!(got.asked.unwrap().declined_by, "ana");
    assert!(service.approved().await.unwrap().modules.is_empty());
    // The same files written again: a person is asked again.
    let again = service.install(&scope(), request("a", &[])).await.unwrap();
    let asked = again.module.unwrap().asked.unwrap();
    assert_eq!(asked.decision, "decision-2");
    assert!(asked.declined_by.is_empty());
    assert_eq!(world.questions.lock().len(), 2);
}

#[tokio::test]
async fn at_start_a_tasks_older_decisions_that_wait_are_withdrawn() {
    service!(world, docs, settings, service);
    service.install(&scope(), request("a", &[])).await.unwrap();
    let decision = |id: &str, by: &str, pending: bool| DecisionRecord {
        id: id.into(),
        pending,
        decision: Some(Decision {
            decided_by: by.into(),
            ..Default::default()
        }),
        ..Default::default()
    };
    *world.thread.lock() = Thread {
        decisions: vec![
            decision("before", "dynamic:greet", true),
            decision("ended", "dynamic:greet", false),
            decision("a person's", "ana", true),
            decision("decision-1", "dynamic:greet", true),
        ],
        ..Default::default()
    };
    service.recheck().await.unwrap();
    assert_eq!(world.withdrawn(), [by_greet("before")]);
}
