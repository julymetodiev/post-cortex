// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use super::*;

#[test]
fn kinds_round_trip_and_refuse_malformed_names() {
    let kind: DocumentKind = "board/task".parse().unwrap();
    assert_eq!((kind.module(), kind.name()), ("board", "task"));
    assert_eq!(kind.to_string(), "board/task");
    let dotted: DocumentKind = "post-cortex.memory/note_2".parse().unwrap();
    assert_eq!(dotted.module(), "post-cortex.memory");

    for bad in ["task", "/task", "board/", "a/b/c", "board/task:x"] {
        assert!(bad.parse::<DocumentKind>().is_err(), "{bad} was accepted");
    }
}

#[test]
fn kinds_serialize_as_their_name() {
    let kind: DocumentKind = "board/task".parse().unwrap();
    assert_eq!(serde_json::to_string(&kind).unwrap(), "\"board/task\"");
    assert!(serde_json::from_str::<DocumentKind>("\"board\"").is_err());
}

#[test]
fn keys_round_trip_and_refuse_malformed_text() {
    let mut digest = [0u8; 16];
    digest[0] = 0xab;
    digest[15] = 0x01;
    let key = DocumentKey::new(7, digest);
    let text = key.to_string();
    assert_eq!(text, "7:ab000000000000000000000000000001");
    assert_eq!(text.parse::<DocumentKey>().unwrap(), key);
    assert_eq!(serde_json::to_string(&key).unwrap(), format!("\"{text}\""));

    for bad in [
        "7",
        "07:ab000000000000000000000000000001",
        "+7:ab000000000000000000000000000001",
        "7:AB000000000000000000000000000001",
        "7:ab00000000000000000000000000000",
        "7:ab0000000000000000000000000000011",
        "7:zz000000000000000000000000000001",
    ] {
        assert!(bad.parse::<DocumentKey>().is_err(), "{bad} was accepted");
    }
}

#[test]
fn data_is_stored_as_its_type_and_encoding_and_old_json_reads_as_json() {
    let data = DocumentData::Message {
        type_name: "pcx.board.v1.Task".into(),
        bytes: vec![10, 2, b'h', b'i'],
    };
    let stored = serde_json::to_value(&data).unwrap();
    assert_eq!(
        stored,
        serde_json::json!({"@type": "pcx.board.v1.Task", "bytes": "CgJoaQ=="})
    );
    assert_eq!(
        serde_json::from_value::<DocumentData>(stored).unwrap(),
        data
    );

    let old = serde_json::json!({"title": "a task", "column": "todo"});
    let Value::Object(map) = old.clone() else {
        unreachable!()
    };
    assert_eq!(
        serde_json::from_value::<DocumentData>(old).unwrap(),
        DocumentData::Json(map)
    );
    let mixed = serde_json::json!({"@type": "x", "bytes": "", "title": "t"});
    assert!(serde_json::from_value::<DocumentData>(mixed).is_err());
}
