// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use serde_json::json;

use super::*;

fn frame(value: Value) -> Frame {
    serde_json::from_value(value).unwrap()
}

fn hello() -> Frame {
    frame(json!({ "op": 10, "d": { "heartbeat_interval": 41250 }, "s": null, "t": null }))
}

fn ready() -> Frame {
    frame(json!({
        "op": 0, "s": 1, "t": "READY",
        "d": {
            "v": 10,
            "session_id": "sess-1",
            "resume_gateway_url": "wss://resume.discord.gg",
            "user": { "id": "9", "username": "pcx", "global_name": null, "bot": true },
        },
    }))
}

fn begun(next: Next) -> (Sent, u32) {
    match next {
        Next::Begin { send, every_ms } => (send, every_ms),
        other => panic!("not a beginning: {other:?}"),
    }
}

#[test]
fn a_hello_is_answered_with_an_identify_and_heartbeats_begin() {
    let mut session = Session::default();
    assert_eq!(session.resumed_at(), None);
    let (send, every_ms) = begun(session.on("tok", hello()).unwrap());
    assert_eq!(every_ms, 41250);
    assert_eq!(send.op, 2);
    assert_eq!(send.d["token"], "tok");
    assert_eq!(send.d["intents"], INTENTS);
    assert_eq!(send.d["properties"]["browser"], "pcx");

    assert_eq!(session.on("tok", ready()).unwrap(), Next::Nothing);
    assert_eq!(session.resumed_at(), Some("wss://resume.discord.gg"));
}

#[test]
fn an_event_is_handed_on_and_its_number_goes_in_the_next_heartbeat() {
    let mut session = Session::default();
    session.on("tok", hello()).unwrap();
    session.on("tok", ready()).unwrap();
    let created = frame(json!({
        "op": 0, "s": 7, "t": "MESSAGE_CREATE", "d": { "id": "5", "content": "hi" },
    }));
    assert_eq!(
        session.on("tok", created).unwrap(),
        Next::Event(
            "MESSAGE_CREATE".to_string(),
            json!({ "id": "5", "content": "hi" })
        )
    );
    let beat = Sent { op: 1, d: json!(7) };
    assert_eq!(session.beat(), Next::Send(beat.clone()));
    // The Gateway may ask for one at once.
    assert_eq!(
        session.on("tok", frame(json!({ "op": 1 }))).unwrap(),
        Next::Send(beat)
    );
}

#[test]
fn a_heartbeat_that_is_not_acknowledged_ends_the_connection() {
    let mut session = Session::default();
    session.on("tok", hello()).unwrap();
    assert_eq!(
        session.beat(),
        Next::Send(Sent {
            op: 1,
            d: Value::Null
        })
    );
    assert_eq!(
        session.on("tok", frame(json!({ "op": 11 }))).unwrap(),
        Next::Nothing
    );
    assert!(matches!(session.beat(), Next::Send(_)));
    assert_eq!(session.beat(), Next::Reconnect);
}

#[test]
fn after_a_cut_the_session_is_resumed_from_its_last_event() {
    let mut session = Session::default();
    session.on("tok", hello()).unwrap();
    session.on("tok", ready()).unwrap();
    session
        .on(
            "tok",
            frame(json!({ "op": 0, "s": 12, "t": "TYPING_START", "d": {} })),
        )
        .unwrap();
    assert_eq!(
        session
            .on("tok", frame(json!({ "op": 7, "d": null })))
            .unwrap(),
        Next::Reconnect
    );
    // The new connection's hello.
    let (send, _) = begun(session.on("tok", hello()).unwrap());
    assert_eq!(send.op, 6);
    assert_eq!(
        send.d,
        json!({ "token": "tok", "session_id": "sess-1", "seq": 12 })
    );
    assert_eq!(
        session
            .on(
                "tok",
                frame(json!({ "op": 0, "s": 13, "t": "RESUMED", "d": {} }))
            )
            .unwrap(),
        Next::Nothing
    );
}

#[test]
fn an_invalid_session_that_cannot_be_resumed_identifies_again() {
    let mut session = Session::default();
    session.on("tok", hello()).unwrap();
    session.on("tok", ready()).unwrap();
    // One that may be resumed keeps the session.
    assert_eq!(
        session
            .on("tok", frame(json!({ "op": 9, "d": true })))
            .unwrap(),
        Next::Reconnect
    );
    assert!(session.resumed_at().is_some());
    assert_eq!(
        session
            .on("tok", frame(json!({ "op": 9, "d": false })))
            .unwrap(),
        Next::Reconnect
    );
    assert_eq!(session.resumed_at(), None);
    let (send, _) = begun(session.on("tok", hello()).unwrap());
    assert_eq!(send.op, 2);
}

#[test]
fn a_frame_that_is_not_the_gateways_is_refused_and_an_unknown_one_ignored() {
    let mut session = Session::default();
    assert!(
        session
            .on("tok", frame(json!({ "op": 10, "d": {} })))
            .is_err()
    );
    assert!(
        session
            .on("tok", frame(json!({ "op": 0, "s": 1, "d": {} })))
            .is_err()
    );
    assert_eq!(
        session
            .on("tok", frame(json!({ "op": 42, "d": "?" })))
            .unwrap(),
        Next::Nothing
    );
}
