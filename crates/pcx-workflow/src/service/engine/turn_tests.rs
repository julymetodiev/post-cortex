// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

// An `agent` block's answer (docs/modules/workflow.md, blocks): read from
// its JSON object, asked for once more when it is not read, the block's
// failure when it is not read again.

use post_cortex_proto::workflow::{Map, StepStatus};

use super::*;
use crate::service::engine::test_engine::{Unused, board, docs, engine_with, kept, settled};
use crate::test_support::turn_of;

/// The run of `daily` whose agent answers `says`, turn after turn: the
/// run's status, the step of its `agent` block and what the agent was asked.
async fn ran(says: &[&str]) -> (i32, post_cortex_proto::workflow::Step, Vec<String>) {
    let docs = docs();
    let mut daily = turn_of("community", &[]);
    daily.grants.get_or_insert_default().members = vec!["community".into()];
    kept(&docs, &daily).await;
    let world = Arc::new(Unused::default());
    world
        .says
        .lock()
        .extend(says.iter().map(|s| (*s).to_string()));
    let engine = engine_with(&docs, board(0), world.clone());
    let run = engine
        .start(
            "daily",
            &Start::Manual,
            Fired::default(),
            &Input::Map(Map::default()),
            None,
        )
        .await
        .unwrap();
    let ended = settled(&engine, &run.id, false).await;
    let (_, steps) = engine.detail(&run.id).await.unwrap();
    let look = steps.into_iter().find(|s| s.block == "look").unwrap();
    let asked = world.asked.lock().clone();
    (ended.data.status, look, asked)
}

#[tokio::test]
async fn an_answer_with_its_json_object_is_read_at_once() {
    let (status, look, asked) = ran(&["Nothing fires.\n{\"report\": \"all quiet\"}"]).await;
    assert_eq!(status, i32::from(RunStatus::Succeeded));
    assert_eq!(look.status, i32::from(StepStatus::Succeeded));
    assert_eq!(asked.len(), 1);
    assert!(asked[0].starts_with("Is anything firing?"));
}

#[tokio::test]
async fn an_answer_not_read_is_asked_for_once_more_and_told_why() {
    // Prod 2026-11-14: the help forum's agent ended with plain text, then
    // with a line break inside a JSON string, and the person got no reply.
    for first in [
        "All is quiet.",
        "{\"report\": \"all\nquiet\"}",
        "{\"other\": 1}",
    ] {
        let (status, look, asked) = ran(&[first, "{\"report\": \"all quiet\"}"]).await;
        assert_eq!(status, i32::from(RunStatus::Succeeded), "{first}");
        assert_eq!(look.status, i32::from(StepStatus::Succeeded));
        assert_eq!(asked.len(), 2, "asked again, once");
        assert!(
            asked[1].starts_with("Your answer was not read: "),
            "{}",
            asked[1]
        );
        assert!(asked[1].contains("\"report\""), "the schema is told again");
    }
}

#[tokio::test]
async fn an_answer_not_read_twice_fails_the_block() {
    let (status, look, asked) =
        ran(&["All is quiet.", "Still quiet.", "{\"report\": \"x\"}"]).await;
    assert_eq!(status, i32::from(RunStatus::Failed));
    assert_eq!(asked.len(), 2, "never a third time");
    let failure = look.failure.unwrap();
    assert_eq!(failure.kind, "answer");
    assert!(
        failure
            .message
            .starts_with("the agent's answer has no JSON object, asked twice: \"Still quiet.\""),
        "{}",
        failure.message
    );
}
