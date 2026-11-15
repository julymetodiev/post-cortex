use super::*;

use serde_json::json;

fn back(reply: wire::TurnReply) -> Reply {
    Reply::from_wire(reply).unwrap()
}

#[test]
fn a_request_crosses_the_wire_whole() {
    let request = TurnRequest {
        instructions: "You review code.".into(),
        model: Some("glm-5.3".into()),
        modules: vec!["board".into()],
        tools: vec!["comment".into(), "decide".into()],
        withheld: vec!["comment".into()],
        ..TurnRequest::new("s1", "Review the task")
    };
    assert_eq!(TurnRequest::from_wire(request.to_wire(None)), request);
    let plain = TurnRequest::new("s1", "Hi");
    assert_eq!(TurnRequest::from_wire(plain.to_wire(None)), plain);
}

#[test]
fn every_event_crosses_the_wire_whole() {
    let events = [
        TurnEvent::Text("hi".into()),
        TurnEvent::Thinking("hmm".into()),
        TurnEvent::ToolCall {
            id: "c1".into(),
            name: "manage_workspace".into(),
            input: json!({ "action": "list" }),
        },
        TurnEvent::ToolResult {
            id: "c1".into(),
            content: "none".into(),
            is_error: false,
        },
        TurnEvent::Retrying {
            failure: ModelFailure::new(crate::model::ModelFailureKind::RateLimited, "slow down"),
            after: Duration::from_millis(1500),
        },
    ];
    for event in events {
        match back(event.clone().to_wire()) {
            Reply::Event(read) => assert_eq!(read, event),
            _ => panic!("{event:?} came back as another reply"),
        }
    }
}

#[test]
fn every_end_crosses_the_wire_whole() {
    let usage = Usage {
        input: 10,
        output: 5,
        cache_read: 2,
        cache_write: 1,
    };
    let stops = [
        TurnStop::Answered(StopReason::EndTurn),
        TurnStop::Answered(StopReason::MaxTokens),
        TurnStop::Answered(StopReason::ToolUse),
        TurnStop::Blocked("a hook said no".into()),
        TurnStop::StepLimit,
        TurnStop::Cancelled,
        TurnStop::Failed(ModelFailure::new(
            crate::model::ModelFailureKind::QuotaExhausted,
            "the plan is used up",
        )),
    ];
    for stop in stops {
        let end = TurnEnd {
            stop,
            steps: 3,
            usage,
            answer: "done".into(),
        };
        match back(end.clone().to_wire()) {
            Reply::End(read) => assert_eq!(read, end),
            _ => panic!("{end:?} came back as another reply"),
        }
    }
}

#[test]
fn a_turn_starts_with_its_id_and_a_broken_end_is_refused() {
    assert!(matches!(back(started("t1".into())), Reply::Started(id) if id == "t1"));
    let failed_without_failure = wire::TurnReply {
        event: Some(wire::turn_reply::Event::End(wire::TurnEnd {
            stop: "failed".into(),
            usage: Some(wire::TurnUsage::default()),
            ..wire::TurnEnd::default()
        })),
    };
    assert!(Reply::from_wire(failed_without_failure).is_err());
    let unknown = wire::TurnReply {
        event: Some(wire::turn_reply::Event::End(wire::TurnEnd {
            stop: "exploded".into(),
            usage: Some(wire::TurnUsage::default()),
            ..wire::TurnEnd::default()
        })),
    };
    assert!(Reply::from_wire(unknown).is_err());
    assert!(Reply::from_wire(wire::TurnReply { event: None }).is_err());
}
