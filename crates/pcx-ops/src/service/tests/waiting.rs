// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Actions that wait for a person: declined, answered while the module
//! was down, and answered by no one.

use super::*;

/// The time this many minutes after [`At`].
struct Later(i64);

impl Clock for Later {
    fn now(&self) -> DateTime<Utc> {
        At.now() + chrono::Duration::minutes(self.0)
    }
}

/// How many actions a look at `minutes` after they were asked settles.
async fn look_after(
    minutes: i64,
    config: &OpsConfig,
    docs: &InMemoryDocuments,
    world: &World,
) -> usize {
    Service {
        clock: &Later(minutes),
        ..service(config, docs, world)
    }
    .look()
    .await
    .unwrap()
}

#[tokio::test]
async fn an_action_a_person_declines_is_given_up_and_its_target_is_free_again() {
    let (config, docs, world) = (config(), documents(), world(false));
    let svc = service(&config, &docs, &world);
    svc.act("eq-medic", restart(false)).await.unwrap();
    {
        let mut decisions = world.decisions.lock();
        let record = &mut decisions[0].2;
        record.pending = false;
        record.decision.as_mut().unwrap().declined_by = Some("julius".into());
    }
    let given_up = svc.decided("D-1").await.unwrap().unwrap().action.unwrap();
    assert_eq!(given_up.state(), State::Declined);
    assert_eq!(given_up.result, "declined by julius");
    assert!(world.patched.lock().is_empty(), "nothing ran");
    assert_eq!(
        world.comments.lock().clone(),
        ["Not done: restart deployment equal-rust/svc-music: declined by julius"]
    );
    assert_eq!(svc.decided("D-1").await.unwrap(), None, "given up once");
    // No brake holds its target: it may be asked again.
    let again = svc.act("eq-medic", restart(false)).await.unwrap();
    assert_eq!(again.action.unwrap().state(), State::Waiting);
}

#[tokio::test]
async fn actions_answered_while_the_module_was_down_are_settled_at_the_next_look() {
    let (config, docs, world) = (config(), documents(), world(false));
    let svc = service(&config, &docs, &world);
    svc.act("eq-medic", restart(false)).await.unwrap();
    let delete = Asked {
        wanted: Wanted::DeletePod {
            namespace: "equal-rust".into(),
            name: "svc-music-7d9".into(),
        },
        ..restart(false)
    };
    svc.act("eq-medic", delete).await.unwrap();
    world
        .objects
        .lock()
        .insert(paths::deployment("equal-rust", "svc-chat"), deployment(1));
    let scale = Asked {
        wanted: Wanted::Scale {
            namespace: "equal-rust".into(),
            name: "svc-chat".into(),
            replicas: 3,
        },
        ..restart(false)
    };
    svc.act("eq-medic", scale).await.unwrap();
    assert_eq!(svc.look().await.unwrap(), 0, "all three still wait");
    assert!(world.told.lock().is_empty(), "and none for long");
    {
        // No write of these was heard: one approved, one declined, one waits.
        let mut decisions = world.decisions.lock();
        decisions[0].2.pending = false;
        decisions[0].2.decision.as_mut().unwrap().approved_by = Some("julius".into());
        decisions[1].2.pending = false;
        decisions[1].2.decision.as_mut().unwrap().declined_by = Some("julius".into());
    }
    assert_eq!(svc.look().await.unwrap(), 2);
    assert_eq!(world.patched.lock().len(), 1, "the approved one ran");
    assert!(world.deleted.lock().is_empty(), "the declined one did not");
    let states: Vec<State> = svc
        .list(0)
        .await
        .unwrap()
        .actions
        .into_iter()
        .filter_map(|a| a.action.map(|a| a.state()))
        .collect();
    let count = |state: State| states.iter().filter(|s| **s == state).count();
    assert_eq!(
        (
            count(State::Done),
            count(State::Declined),
            count(State::Waiting)
        ),
        (1, 1, 1)
    );
    assert_eq!(svc.look().await.unwrap(), 0, "each is settled once");
}

#[tokio::test]
async fn an_action_no_one_answers_is_said_again_once_then_given_up() {
    let (config, docs, world) = (config(), documents(), world(false));
    service(&config, &docs, &world)
        .act("eq-medic", restart(false))
        .await
        .unwrap();
    assert_eq!(look_after(29, &config, &docs, &world).await, 0);
    assert!(world.told.lock().is_empty(), "not long yet");

    assert_eq!(look_after(31, &config, &docs, &world).await, 0);
    assert_eq!(look_after(90, &config, &docs, &world).await, 0);
    assert_eq!(
        world.told.lock().clone(),
        [
            "Still waits for a person, 31 minutes now: restart deployment equal-rust/svc-music. Approve or decline it; unanswered 240 minutes after it was asked, it is given up."
        ],
        "said once"
    );
    assert!(world.decisions.lock()[0].2.pending, "it still waits");

    // Four hours on its decision is withdrawn as who asked; that write, or
    // the next look, gives the action up.
    assert_eq!(look_after(240, &config, &docs, &world).await, 0);
    let record = world.decisions.lock()[0].2.clone();
    assert!(!record.pending);
    assert_eq!(
        record.decision.unwrap().withdrawn.as_deref(),
        Some("no one answered in 240 minutes")
    );
    assert_eq!(look_after(241, &config, &docs, &world).await, 1);
    assert_eq!(
        world.comments.lock().clone(),
        [
            "Not done: restart deployment equal-rust/svc-music: its decision was withdrawn (no one answered in 240 minutes)"
        ]
    );
    assert!(world.patched.lock().is_empty(), "nothing ran");
    let again = service(&config, &docs, &world)
        .act("eq-medic", restart(false))
        .await
        .unwrap();
    assert_eq!(
        again.action.unwrap().state(),
        State::Waiting,
        "its target is free"
    );
}

#[test]
fn settings_whose_reminder_is_not_before_giving_up_are_refused() {
    let late = OpsConfig {
        remind_minutes: 300,
        ..config()
    };
    let refused = checked(late).unwrap_err();
    assert_eq!(refused.kind, ModuleErrorKind::InvalidArgument);
    assert_eq!(refused.field.as_deref(), Some("give_up_minutes"));
    assert!(checked(config()).is_ok());
}
