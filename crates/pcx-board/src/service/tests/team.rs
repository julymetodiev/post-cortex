// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The board with a team: who decides and closes, and the team's policy
//! approving within its limits.

use super::*;

#[tokio::test]
async fn with_a_team_only_its_members_decide_and_close_within_their_authority() {
    let (docs, memory) = (documents(), Memory::default());
    let ana = Service::new(&docs, &memory, &Git, &memory, Some("ana"), None);
    let created = ana
        .create_board("Ops", columns(), vec![], None)
        .await
        .unwrap();
    let task = ana
        .add_task(AddTaskRequest {
            board: created.document.id.clone(),
            title: "Ship".into(),
            ..Default::default()
        })
        .await
        .unwrap()
        .task
        .task;
    // Without a team anyone closes.
    let eve = Service::new(&docs, &memory, &Git, &memory, Some("eve"), None);
    let closed = eve
        .move_task(to(&task.document.id, task.document.version, "done"))
        .await
        .unwrap()
        .task;

    team(&docs).await;
    let bot = Service::new(&docs, &memory, &Git, &memory, Some("bot"), None);
    let refused = bot
        .decide(
            &task.document.id,
            "ship",
            "ready",
            Risk::None,
            Stakes::default(),
        )
        .await
        .unwrap_err();
    assert_eq!(refused.kind, ModuleErrorKind::Denied);
    assert!(refused.message.contains("discuss"), "{}", refused.message);
    let stranger = eve
        .move_task(to(&task.document.id, closed.document.version, "todo"))
        .await
        .unwrap()
        .task;
    let not_member = eve
        .move_task(to(&task.document.id, stranger.document.version, "done"))
        .await
        .unwrap_err();
    assert_eq!(not_member.kind, ModuleErrorKind::Denied);
    // Moving out of a done column is not closing; into it is.
    ana.decide(
        &task.document.id,
        "ship",
        "ready",
        Risk::None,
        Stakes::default(),
    )
    .await
    .unwrap();
    ana.move_task(to(&task.document.id, stranger.document.version, "done"))
        .await
        .unwrap();
}

#[tokio::test]
async fn the_teams_policy_approves_within_its_limits_and_a_person_revokes() {
    let (docs, memory) = (documents(), Memory::default());
    team(&docs).await;
    Typed::new(&docs, pcx_team::TeamKind::Autonomy.document())
        .create(
            &pcx_team::Autonomy {
                team: "core".into(),
                paused: false,
                trust: Some(pcx_team::Trust {
                    mode: pcx_team::Mode::Classes.into(),
                    classes: vec![pcx_team::Class::Money.into()],
                    code: None,
                    money: Some(pcx_team::MoneyLimit {
                        currency: "EUR".into(),
                        per_decision: 50,
                        per_day: 100,
                    }),
                    data: None,
                    operations: None,
                }),
                exceptions: Vec::new(),
            },
            "",
        )
        .await
        .unwrap();
    let ana = Service::new(&docs, &memory, &Git, &memory, Some("ana"), None);
    let created = ana
        .create_board("Ops", columns(), vec![], None)
        .await
        .unwrap();
    let task = ana
        .add_task(AddTaskRequest {
            board: created.document.id.clone(),
            title: "Buy".into(),
            ..Default::default()
        })
        .await
        .unwrap()
        .task
        .task;
    let spend = |amount| Stakes {
        amount: Some(amount),
        currency: Some("EUR".into()),
        ..Stakes::default()
    };
    let first = ana
        .decide(
            &task.document.id,
            "a domain",
            "needed",
            Risk::Money,
            spend(30),
        )
        .await
        .unwrap();
    assert_eq!(first.data.approved_by.as_deref(), Some("policy:1"));
    let second = ana
        .decide(
            &task.document.id,
            "hosting",
            "needed",
            Risk::Money,
            spend(40),
        )
        .await
        .unwrap();
    assert!(second.data.by_policy());
    // 110 in a day is past its limit: a person decides.
    let third = ana
        .decide(&task.document.id, "a logo", "nice", Risk::Money, spend(40))
        .await
        .unwrap();
    assert!(third.data.pending());
    assert_eq!(
        third.data.held_because.as_deref(),
        Some(
            "the policy would not approve it: over the money limit: 100 EUR a day, 70 approved today"
        )
    );
    // Data is not a class the team was given.
    let data = ana
        .decide(
            &task.document.id,
            "drop it",
            "old",
            Risk::Data,
            Stakes::default(),
        )
        .await
        .unwrap();
    assert!(data.data.pending());
    assert_eq!(
        data.data.held_because.as_deref(),
        Some("the policy would not approve it: the trust does not approve data by itself")
    );

    ana.approve(&third.document.id, third.document.version, None)
        .await
        .unwrap();
    ana.approve(&data.document.id, data.document.version, None)
        .await
        .unwrap();
    let moved = ana
        .move_task(to(&task.document.id, task.document.version, "doing"))
        .await
        .unwrap()
        .task;
    let revoked = ana
        .revoke(&first.document.id, first.document.version)
        .await
        .unwrap();
    assert!(revoked.data.pending());
    let held = ana
        .move_task(to(&task.document.id, moved.document.version, "done"))
        .await
        .unwrap_err();
    assert_eq!(held.kind, ModuleErrorKind::FailedPrecondition);
    let again = ana
        .revoke(&revoked.document.id, revoked.document.version)
        .await
        .unwrap_err();
    assert_eq!(again.kind, ModuleErrorKind::FailedPrecondition);
}

#[tokio::test]
async fn an_owner_of_the_organisation_closes_outside_every_team() {
    let (docs, memory) = (documents(), Memory::default());
    let ana = Service::new(&docs, &memory, &Git, &memory, Some("ana"), None);
    let created = ana
        .create_board("Ops", columns(), vec![], None)
        .await
        .unwrap();
    let task = ana
        .add_task(AddTaskRequest {
            board: created.document.id.clone(),
            title: "Ship".into(),
            ..Default::default()
        })
        .await
        .unwrap()
        .task
        .task;
    team(&docs).await;
    let member = Service::new(
        &docs,
        &memory,
        &Git,
        &memory,
        Some("julius"),
        Some(PersonRole::Member),
    );
    let refused = member
        .move_task(to(&task.document.id, task.document.version, "done"))
        .await
        .unwrap_err();
    assert_eq!(refused.kind, ModuleErrorKind::Denied);
    let owner = Service::new(
        &docs,
        &memory,
        &Git,
        &memory,
        Some("julius"),
        Some(PersonRole::Owner),
    );
    owner
        .decide(
            &task.document.id,
            "ship",
            "ready",
            Risk::None,
            Stakes::default(),
        )
        .await
        .unwrap();
    owner
        .move_task(to(&task.document.id, task.document.version, "done"))
        .await
        .unwrap();
}
