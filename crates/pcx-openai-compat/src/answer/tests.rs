// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use super::*;

use serde_json::json;

fn codes() -> Codes {
    Codes {
        context_too_long: vec!["1261".into()],
        rate_limited: vec!["1302".into()],
        overloaded: vec!["1305".into()],
        quota_exhausted: vec!["1308".into()],
    }
}

fn read(answer: &mut Answer<'_>, event: Value) -> Vec<ModelEvent> {
    answer.read(&event.to_string()).unwrap()
}

#[test]
fn text_reasoning_and_tool_calls_build_the_answer() {
    let codes = codes();
    let mut answer = Answer::new(&codes);
    let first = read(
        &mut answer,
        json!({ "choices": [{ "delta": { "reasoning_content": "Look", "content": "I'll read" } }] }),
    );
    assert_eq!(
        first,
        [
            ModelEvent::ThinkingDelta("Look".into()),
            ModelEvent::TextDelta("I'll read".into())
        ]
    );
    read(
        &mut answer,
        json!({ "choices": [{ "delta": { "tool_calls": [
            { "index": 0, "id": "c1", "function": { "name": "fs_read", "arguments": "{\"pa" } }
        ] } }] }),
    );
    read(
        &mut answer,
        json!({ "choices": [{ "delta": { "tool_calls": [
            { "index": 0, "function": { "arguments": "th\":\"a.rs\"}" } }
        ] }, "finish_reason": "tool_calls" }],
          "usage": { "prompt_tokens": 120, "completion_tokens": 30,
                     "prompt_tokens_details": { "cached_tokens": 100 } } }),
    );
    let end = answer.finish().unwrap();
    let call = Block::ToolUse {
        id: "c1".into(),
        name: "fs_read".into(),
        input: json!({ "path": "a.rs" }),
    };
    assert_eq!(
        end,
        [
            ModelEvent::ToolUse {
                id: "c1".into(),
                name: "fs_read".into(),
                input: json!({ "path": "a.rs" })
            },
            ModelEvent::Done {
                blocks: vec![
                    Block::Thinking {
                        text: "Look".into(),
                        signature: None
                    },
                    Block::Text {
                        text: "I'll read".into()
                    },
                    call
                ],
                usage: Usage {
                    input: 20,
                    output: 30,
                    cache_read: 100,
                    cache_write: 0
                },
                stop: StopReason::ToolUse,
            }
        ]
    );
}

#[test]
fn failures_are_typed() {
    let codes = codes();
    let mut answer = Answer::new(&codes);
    let error = answer
        .read(&json!({ "error": { "code": "1261", "message": "Prompt too long" } }).to_string())
        .unwrap_err();
    assert_eq!(error.kind, ModelFailureKind::ContextTooLong);

    let mut filtered = Answer::new(&codes);
    let stopped = filtered
        .read(&json!({ "choices": [{ "delta": {}, "finish_reason": "sensitive" }] }).to_string())
        .unwrap_err();
    assert_eq!(stopped.kind, ModelFailureKind::Invalid);

    let mut cut = Answer::new(&codes);
    read(
        &mut cut,
        json!({ "choices": [{ "delta": { "content": "Hal" } }] }),
    );
    assert_eq!(
        cut.finish().unwrap_err().kind,
        ModelFailureKind::Unavailable
    );

    let mut broken = Answer::new(&codes);
    read(
        &mut broken,
        json!({ "choices": [{ "delta": { "tool_calls": [
            { "index": 0, "id": "c1", "function": { "name": "x", "arguments": "{oops" } }
        ] }, "finish_reason": "tool_calls" }] }),
    );
    assert_eq!(broken.finish().unwrap_err().kind, ModelFailureKind::Invalid);

    // By status when the code is not one the provider names.
    let refused = codes.failure(Some(401), &json!({ "message": "bad key" }));
    assert_eq!(refused.kind, ModelFailureKind::Unauthorized);
    let busy = codes.failure(Some(429), &json!({ "code": "1305", "message": "busy" }));
    assert_eq!(busy.kind, ModelFailureKind::Overloaded);
    assert_eq!(busy.message, "busy (code 1305)");
    // A used-up plan answers 429 too, and is not retried.
    let spent = codes.failure(Some(429), &json!({ "code": 1308, "message": "limit" }));
    assert_eq!(spent.kind, ModelFailureKind::QuotaExhausted);
}

#[test]
fn a_tool_call_the_most_tokens_cut_is_dropped_and_the_stop_says_so() {
    let codes = codes();
    let mut cut = Answer::new(&codes);
    read(
        &mut cut,
        json!({ "choices": [{ "delta": { "tool_calls": [
            { "index": 0, "id": "c1", "function": { "name": "write", "arguments": "{\"text\":\"half" } }
        ] }, "finish_reason": "length" }] }),
    );
    let events = cut.finish().unwrap();
    assert_eq!(events.len(), 1);
    assert!(matches!(
        &events[0],
        ModelEvent::Done { blocks, stop: StopReason::MaxTokens, .. } if blocks.is_empty()
    ));
}
