// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use serde_json::json;

use super::*;

fn read(events: &[Value]) -> Result<Vec<ModelEvent>, ModelFailure> {
    let mut answer = Answer::new();
    let mut out = Vec::new();
    for event in events {
        out.extend(answer.read(&event.to_string())?);
    }
    out.extend(answer.finish()?);
    Ok(out)
}

#[test]
fn reasoning_text_and_a_tool_call_in_their_order() {
    let events = [
        json!({"type": "message_start", "message": {"usage": {
            "input_tokens": 10, "output_tokens": 1,
            "cache_read_input_tokens": 100, "cache_creation_input_tokens": 5}}}),
        json!({"type": "content_block_start", "index": 0, "content_block": {"type": "thinking", "thinking": ""}}),
        json!({"type": "content_block_delta", "index": 0, "delta": {"type": "thinking_delta", "thinking": "Read it."}}),
        json!({"type": "content_block_delta", "index": 0, "delta": {"type": "signature_delta", "signature": "sig"}}),
        json!({"type": "content_block_stop", "index": 0}),
        json!({"type": "content_block_start", "index": 1, "content_block": {"type": "text", "text": ""}}),
        json!({"type": "content_block_delta", "index": 1, "delta": {"type": "text_delta", "text": "Reading."}}),
        json!({"type": "content_block_start", "index": 2, "content_block": {"type": "tool_use", "id": "toolu_1", "name": "fs_read", "input": {}}}),
        json!({"type": "content_block_delta", "index": 2, "delta": {"type": "input_json_delta", "partial_json": "{\"path\":"}}),
        json!({"type": "content_block_delta", "index": 2, "delta": {"type": "input_json_delta", "partial_json": "\"a.rs\"}"}}),
        json!({"type": "ping"}),
        json!({"type": "message_delta", "delta": {"stop_reason": "tool_use"}, "usage": {"output_tokens": 30}}),
        json!({"type": "message_stop"}),
    ];
    let out = read(&events).unwrap();
    assert_eq!(out[0], ModelEvent::ThinkingDelta("Read it.".into()));
    assert_eq!(out[1], ModelEvent::TextDelta("Reading.".into()));
    let input = json!({"path": "a.rs"});
    assert_eq!(
        out[2],
        ModelEvent::ToolUse {
            id: "toolu_1".into(),
            name: "fs_read".into(),
            input: input.clone()
        }
    );
    assert_eq!(
        out[3],
        ModelEvent::Done {
            blocks: vec![
                Block::Thinking {
                    text: "Read it.".into(),
                    signature: Some("sig".into())
                },
                Block::Text {
                    text: "Reading.".into()
                },
                Block::ToolUse {
                    id: "toolu_1".into(),
                    name: "fs_read".into(),
                    input
                },
            ],
            usage: Usage {
                input: 10,
                output: 30,
                cache_read: 100,
                cache_write: 5
            },
            stop: StopReason::ToolUse,
        }
    );
}

#[test]
fn an_error_in_the_stream_a_refusal_and_a_full_window() {
    let overloaded =
        [json!({"type": "error", "error": {"type": "overloaded_error", "message": "Overloaded"}})];
    assert_eq!(
        read(&overloaded).unwrap_err().kind,
        ModelFailureKind::Overloaded
    );
    let refused = [json!({"type": "message_delta", "delta": {"stop_reason": "refusal"}})];
    assert_eq!(read(&refused).unwrap_err().kind, ModelFailureKind::Invalid);
    let full = [
        json!({"type": "message_delta", "delta": {"stop_reason": "model_context_window_exceeded"}}),
    ];
    assert_eq!(
        read(&full).unwrap_err().kind,
        ModelFailureKind::ContextTooLong
    );
    // A stream that ends before its stop reason.
    let cut = [json!({"type": "message_start", "message": {"usage": {}}})];
    assert_eq!(read(&cut).unwrap_err().kind, ModelFailureKind::Unavailable);
}

#[test]
fn a_tool_call_the_most_tokens_cut_is_dropped_and_the_stop_says_so() {
    let mut answer = Answer::default();
    for event in [
        json!({"type": "content_block_start", "index": 0, "content_block": {"type": "tool_use", "id": "c1", "name": "write"}}),
        json!({"type": "content_block_delta", "index": 0, "delta": {"type": "input_json_delta", "partial_json": "{\"text\":\"half"}}),
        json!({"type": "message_delta", "delta": {"stop_reason": "max_tokens"}, "usage": {"output_tokens": 9}}),
    ] {
        answer.read(&event.to_string()).unwrap();
    }
    let events = answer.finish().unwrap();
    assert_eq!(events.len(), 1);
    assert!(matches!(
        &events[0],
        ModelEvent::Done { blocks, stop: StopReason::MaxTokens, .. } if blocks.is_empty()
    ));
}
