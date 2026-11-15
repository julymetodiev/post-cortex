// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use super::*;

use pcx_module_api::agent::TurnEnd;
use pcx_module_api::model::StopReason;

#[test]
fn an_answer_is_done_work_cut_short_halts_a_deadline_or_a_failure_is_tried_again_a_block_is_not() {
    let end = |stop: TurnStop, answer: &str| TurnEnd {
        stop,
        steps: 3,
        usage: pcx_module_api::model::Usage::default(),
        answer: answer.to_string(),
    };
    assert_eq!(
        ended(&end(TurnStop::Answered(StopReason::EndTurn), "done")),
        Ending::Done
    );
    assert_eq!(ended(&end(TurnStop::StepLimit, "")), Ending::Halt);
    // Its model's most tokens without text: unfinished, for a person.
    assert_eq!(
        ended(&end(TurnStop::Answered(StopReason::MaxTokens), " ")),
        Ending::Halt
    );
    // With text, what it wrote is its answer.
    assert_eq!(
        ended(&end(TurnStop::Answered(StopReason::MaxTokens), "half")),
        Ending::Done
    );
    assert!(matches!(
        ended(&end(TurnStop::Cancelled, "")),
        Ending::Retry { .. }
    ));
    assert!(matches!(
        ended(&end(TurnStop::Blocked("no".into()), "")),
        Ending::Stop(_)
    ));
    let busy = ModuleError::new(ModuleErrorKind::Unavailable, "rate limited");
    assert_eq!(
        failed(&busy),
        Ending::Retry {
            why: "rate limited".into(),
            after: Duration::from_secs(60)
        }
    );
    let refused = ModuleError::new(ModuleErrorKind::Denied, "no authority");
    assert_eq!(failed(&refused), Ending::Stop("no authority".into()));
}

#[test]
fn a_model_failure_that_passes_is_tried_again_after_a_pause_and_one_that_does_not_stops() {
    let end = |failure: ModelFailure| TurnEnd {
        stop: TurnStop::Failed(failure),
        steps: 1,
        usage: pcx_module_api::model::Usage::default(),
        answer: String::new(),
    };
    for kind in [
        ModelFailureKind::RateLimited,
        ModelFailureKind::Overloaded,
        ModelFailureKind::Unavailable,
    ] {
        assert!(
            matches!(
                ended(&end(ModelFailure::new(kind, "busy"))),
                Ending::Retry { after, .. } if after == Duration::from_secs(60)
            ),
            "{kind:?}"
        );
    }
    // A provider's longer wait is kept; a shorter one is the pause.
    let later = |secs| ModelFailure {
        retry_after: Some(Duration::from_secs(secs)),
        ..ModelFailure::new(ModelFailureKind::RateLimited, "slow down")
    };
    assert!(matches!(
        ended(&end(later(300))),
        Ending::Retry { after, .. } if after == Duration::from_secs(300)
    ));
    assert!(matches!(
        ended(&end(later(1))),
        Ending::Retry { after, .. } if after == Duration::from_secs(60)
    ));
    for kind in [
        ModelFailureKind::QuotaExhausted,
        ModelFailureKind::Unauthorized,
        ModelFailureKind::ContextTooLong,
        ModelFailureKind::Invalid,
    ] {
        assert_eq!(
            ended(&end(ModelFailure::new(kind, "code 1113"))),
            Ending::Stop(format!("its model failed: {}: code 1113", kind.name())),
        );
    }
}

#[test]
fn the_settings_give_a_deadline_and_tries() {
    assert_eq!(
        deadline(&TeamConfig::default()),
        Duration::from_secs(30 * 60)
    );
    let set = TeamConfig {
        work_minutes: 5,
        tries: 3,
    };
    assert_eq!(deadline(&set), Duration::from_secs(300));
    assert_eq!((tries(&TeamConfig::default()), tries(&set)), (2, 3));
}

#[test]
fn a_blocked_task_is_labelled_once_and_says_why() {
    let once = with_label(&["alert".to_string()], BLOCKED);
    assert_eq!(once, ["alert", "blocked"]);
    assert_eq!(with_label(&once, BLOCKED), once);
    assert!(
        said("eq-sentinel", 2, "it ran past its deadline")
            .starts_with("Blocked: eq-sentinel's work failed every time it was tried (2)")
    );
}

#[test]
fn an_offline_runner_is_waited_for_and_the_label_comes_off() {
    let offline = ModuleError::new(ModuleErrorKind::Offline, "the runner is offline");
    assert_eq!(
        failed(&offline),
        Ending::Wait("the runner is offline".into())
    );
    let labels = vec!["bug".to_string()];
    let labelled = with_label(&with_label(&labels, WAITING), WAITING);
    assert_eq!(labelled, vec!["bug".to_string(), WAITING.to_string()]);
    assert_eq!(without_labels(&labelled, &[WAITING]), labels);
    assert_eq!(without_labels(&labels, &[WAITING_PERSON]), labels);
    assert!(waits("dev", "offline").starts_with("Waiting: dev's work needs the runner"));
}
