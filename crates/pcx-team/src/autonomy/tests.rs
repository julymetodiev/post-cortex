// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use super::*;

use crate::domain::Authority;

fn approves(policy: &Autonomy, member: &Agent, class: Class, stakes: &Stakes, spent: u64) -> bool {
    verdict(policy, member, class, stakes, spent).is_ok()
}

fn member(name: &str, role: &str) -> Agent {
    Agent {
        name: name.into(),
        role: role.into(),
        authority: Authority::Decide.into(),
        harness: true,
        skills: Vec::new(),
        model: None,
        modules: Vec::new(),
        tools: Vec::new(),
        reach: Vec::new(),
    }
}

fn full() -> Trust {
    Trust {
        mode: Mode::Full.into(),
        classes: Vec::new(),
        code: Some(CodeLimit {
            branches: vec!["feature/".into()],
            protected: vec!["feature/release".into()],
        }),
        money: Some(MoneyLimit {
            currency: "EUR".into(),
            per_decision: 50,
            per_day: 100,
        }),
        data: Some(DataLimit { deletions: false }),
        operations: None,
    }
}

fn spend(amount: u64, currency: &str) -> Stakes {
    Stakes {
        amount: Some(amount),
        currency: Some(currency.into()),
        ..Stakes::default()
    }
}

#[test]
fn a_supervised_or_paused_team_waits_for_a_person() {
    let dev = member("dev", "builder");
    let mut policy = supervised("core");
    assert!(!approves(&policy, &dev, Class::Money, &spend(1, "EUR"), 0));
    policy.trust = Some(full());
    assert!(approves(&policy, &dev, Class::Money, &spend(1, "EUR"), 0));
    policy.paused = true;
    assert!(!approves(&policy, &dev, Class::Money, &spend(1, "EUR"), 0));
}

#[test]
fn each_class_approves_within_its_limits() {
    let dev = member("dev", "builder");
    let policy = Autonomy {
        trust: Some(full()),
        ..supervised("core")
    };
    let branch = |b: &str| Stakes {
        branch: Some(b.into()),
        ..Stakes::default()
    };
    assert!(approves(
        &policy,
        &dev,
        Class::Code,
        &branch("feature/x"),
        0
    ));
    assert!(!approves(&policy, &dev, Class::Code, &branch("main"), 0));
    assert!(!approves(
        &policy,
        &dev,
        Class::Code,
        &branch("feature/release"),
        0
    ));
    assert!(!approves(&policy, &dev, Class::Code, &Stakes::default(), 0));

    assert!(approves(&policy, &dev, Class::Money, &spend(50, "EUR"), 50));
    assert!(
        !approves(&policy, &dev, Class::Money, &spend(51, "EUR"), 0),
        "over one decision's cap"
    );
    assert!(
        !approves(&policy, &dev, Class::Money, &spend(10, "EUR"), 95),
        "over the day's"
    );
    assert!(
        !approves(&policy, &dev, Class::Money, &spend(10, "USD"), 0),
        "another currency"
    );

    let data = |reversible| Stakes {
        reversible,
        ..Stakes::default()
    };
    assert!(approves(&policy, &dev, Class::Data, &data(Some(true)), 0));
    assert!(!approves(&policy, &dev, Class::Data, &data(Some(false)), 0));
    assert!(!approves(&policy, &dev, Class::Data, &data(None), 0));
}

#[test]
fn a_member_or_role_exception_comes_first() {
    let policy = Autonomy {
        trust: Some(full()),
        exceptions: vec![
            Exception {
                of: "intern".into(),
                trust: Some(Trust {
                    mode: Mode::Supervised.into(),
                    ..Trust::default()
                }),
            },
            Exception {
                of: "ops".into(),
                trust: Some(Trust {
                    mode: Mode::Classes.into(),
                    classes: vec![Class::Data.into()],
                    ..full()
                }),
            },
        ],
        ..supervised("core")
    };
    let stakes = Stakes {
        reversible: Some(true),
        ..spend(1, "EUR")
    };
    assert!(!approves(
        &policy,
        &member("ana", "intern"),
        Class::Data,
        &stakes,
        0
    ));
    let ops = member("ops", "builder");
    assert!(approves(&policy, &ops, Class::Data, &stakes, 0));
    assert!(
        !approves(&policy, &ops, Class::Money, &stakes, 0),
        "not a class it lists"
    );
}

#[test]
fn a_refusal_says_why() {
    let dev = member("dev", "builder");
    let mut policy = supervised("core");
    assert_eq!(
        verdict(&policy, &dev, Class::Money, &spend(1, "EUR"), 0),
        Err(Refusal::Supervised)
    );
    policy.trust = Some(full());
    let over = verdict(&policy, &dev, Class::Money, &spend(51, "EUR"), 0).unwrap_err();
    assert_eq!(over.to_string(), "over the money limit: 50 EUR a decision");
    let day = verdict(&policy, &dev, Class::Money, &spend(10, "EUR"), 95).unwrap_err();
    assert_eq!(
        day.to_string(),
        "over the money limit: 100 EUR a day, 95 approved today"
    );
    let branch = Stakes {
        branch: Some("main".into()),
        ..Stakes::default()
    };
    assert_eq!(
        verdict(&policy, &dev, Class::Code, &branch, 0)
            .unwrap_err()
            .to_string(),
        "code approves itself only on feature/; this changes main"
    );
    let data = Stakes {
        reversible: Some(false),
        ..Stakes::default()
    };
    assert_eq!(
        verdict(&policy, &dev, Class::Data, &data, 0),
        Err(Refusal::Deletion)
    );
}

#[test]
fn operations_approve_themselves_only_for_the_actions_and_environments_trusted() {
    let medic = member("eq-medic", "responder");
    let act = |action: &str, environment: &str| Stakes {
        action: Some(action.into()),
        environment: Some(environment.into()),
        target: Some("equal-rust/svc-music".into()),
        ..Stakes::default()
    };
    let mut policy = supervised("ops");
    policy.trust = Some(Trust {
        mode: Mode::Classes.into(),
        classes: vec![Class::Operations.into()],
        ..full()
    });
    assert_eq!(
        verdict(
            &policy,
            &medic,
            Class::Operations,
            &act("restart", "prod"),
            0
        ),
        Err(Refusal::NoLimit(Class::Operations)),
        "no limits set: operations wait"
    );
    policy.trust.as_mut().unwrap().operations = Some(post_cortex_proto::team::OperationsLimit {
        actions: vec!["restart".into(), "delete_pod".into()],
        environments: vec!["staging".into(), "prod".into()],
    });
    assert!(approves(
        &policy,
        &medic,
        Class::Operations,
        &act("restart", "prod"),
        0
    ));
    let scale = verdict(&policy, &medic, Class::Operations, &act("scale", "prod"), 0);
    assert!(matches!(scale, Err(Refusal::Action { .. })));
    let elsewhere = verdict(
        &policy,
        &medic,
        Class::Operations,
        &act("restart", "dev"),
        0,
    );
    assert!(matches!(elsewhere, Err(Refusal::Environment { .. })));
    assert_eq!(
        verdict(&policy, &medic, Class::Operations, &Stakes::default(), 0),
        Err(Refusal::NoAction)
    );
    assert_eq!(
        scale.unwrap_err().to_string(),
        "operations approve themselves only for restart, delete_pod; this is scale"
    );
}

#[test]
fn a_decision_only_a_person_approves_waits_whatever_the_trust() {
    let medic = member("pcx-medic", "responder");
    let mut policy = supervised("ops");
    policy.trust = Some(Trust {
        operations: Some(post_cortex_proto::team::OperationsLimit {
            actions: vec!["revert_jobs".into()],
            environments: vec!["pcx".into()],
        }),
        ..full()
    });
    let revert = Stakes {
        action: Some("revert_jobs".into()),
        environment: Some("pcx".into()),
        target: Some("pcx-app, pcx-cloud".into()),
        ..Stakes::default()
    };
    assert!(
        approves(&policy, &medic, Class::Operations, &revert, 0),
        "the trust names it"
    );
    let by_person = Stakes {
        person: Some(true),
        ..revert
    };
    assert_eq!(
        verdict(&policy, &medic, Class::Operations, &by_person, 0),
        Err(Refusal::Person)
    );
}
