// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use post_cortex_proto::module::EmittedEvent;

use super::*;

fn board(emits: &[(&str, &str)]) -> Manifest {
    Manifest {
        id: "board".to_string(),
        descriptors: post_cortex_proto::BOARD_DESCRIPTOR_SET.to_vec(),
        emits: emits
            .iter()
            .map(|(name, ty)| EmittedEvent {
                name: (*name).to_string(),
                r#type: (*ty).to_string(),
                description: String::new(),
            })
            .collect(),
        ..Manifest::default()
    }
}

#[test]
fn an_event_is_its_modules_once_and_of_a_message_it_holds() {
    let task = "pcx.board.v1.Task";
    assert_eq!(check_emits(&board(&[("board.task_moved", task)])), Ok(()));
    assert_eq!(check_emits(&board(&[("board.task.moved_2", task)])), Ok(()));
    for name in [
        "team.task_moved",
        "board.",
        "board.Task",
        "board..x",
        "boardtask",
    ] {
        assert!(check_emits(&board(&[(name, task)])).is_err(), "{name}");
    }
    assert!(check_emits(&board(&[("board.a", task), ("board.a", task)])).is_err());
    assert!(check_emits(&board(&[("board.a", "pcx.board.v1.Nothing")])).is_err());
}

#[test]
fn a_payload_is_the_type_its_url_names() {
    let any = |url: &str| Any {
        type_url: url.to_string(),
        value: Vec::new().into(),
    };
    assert!(payload_is(
        &any("type.googleapis.com/pcx.board.v1.Task"),
        "pcx.board.v1.Task"
    ));
    assert!(payload_is(&any("x/pcx.board.v1.Task"), "pcx.board.v1.Task"));
    assert!(!payload_is(&any("pcx.board.v1.Task"), "pcx.board.v1.Task"));
    assert!(!payload_is(
        &any("x/pcx.board.v1.Board"),
        "pcx.board.v1.Task"
    ));
}
