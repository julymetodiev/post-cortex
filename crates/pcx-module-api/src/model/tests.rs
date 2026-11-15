// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use super::*;

use serde_json::json;

#[tokio::test]
async fn a_run_collects_its_answer_or_its_failure() {
    let (tx, rx) = mpsc::channel(4);
    tx.send(Ok(ModelEvent::TextDelta("Hi".into())))
        .await
        .unwrap();
    let blocks = vec![Block::Text { text: "Hi".into() }];
    tx.send(Ok(ModelEvent::Done {
        blocks: blocks.clone(),
        usage: Usage::default(),
        stop: StopReason::EndTurn,
    }))
    .await
    .unwrap();
    let (got, _, stop) = ModelRun::new(rx).collect().await.unwrap();
    assert_eq!((got, stop), (blocks, StopReason::EndTurn));

    let (tx, rx) = mpsc::channel(4);
    tx.send(Err(ModelFailure::new(ModelFailureKind::Overloaded, "busy")))
        .await
        .unwrap();
    let failed = ModelRun::new(rx).collect().await.unwrap_err();
    assert_eq!(failed.kind, ModelFailureKind::Overloaded);
    assert_eq!(failed.to_string(), "overloaded: busy");

    let (tx, rx) = mpsc::channel::<Result<ModelEvent, ModelFailure>>(1);
    drop(tx);
    let ended = ModelRun::new(rx).collect().await.unwrap_err();
    assert_eq!(ended.kind, ModelFailureKind::Unavailable);
}

#[test]
fn blocks_have_their_json_form() {
    let used = Block::ToolUse {
        id: "c1".into(),
        name: "fs_read".into(),
        input: json!({ "path": "a.rs" }),
    };
    assert_eq!(
        serde_json::to_value(&used).unwrap(),
        json!({ "type": "tool_use", "id": "c1", "name": "fs_read", "input": { "path": "a.rs" } })
    );
}
