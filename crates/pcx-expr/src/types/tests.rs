// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use super::*;

#[test]
fn types_read_as_cel_writes_them() {
    for text in [
        "string",
        "int",
        "uint",
        "double",
        "bool",
        "bytes",
        "timestamp",
        "duration",
        "list<string>",
        "map<string, list<int>>",
    ] {
        assert_eq!(Type::parse(text).unwrap().to_string(), text);
    }
    assert_eq!(
        Type::parse(" map<string,double> ").unwrap(),
        Type::Map(Box::new(Type::Double))
    );
    for bad in [
        "str",
        "list<>",
        "map<int, string>",
        "list<string",
        "dyn",
        "null",
    ] {
        assert!(Type::parse(bad).is_err(), "{bad}");
    }
}

#[test]
fn a_type_accepts_its_own_dyn_and_null_for_a_message() {
    let task = Type::Message("t.Task".into());
    assert!(task.accepts(&task));
    assert!(task.accepts(&Type::Null));
    assert!(task.accepts(&Type::Dyn));
    assert!(Type::String.accepts(&Type::Dyn));
    assert!(!Type::String.accepts(&Type::Int));
    assert!(!Type::Int.accepts(&Type::Null));
    let strings = Type::List(Box::new(Type::String));
    assert!(strings.accepts(&Type::List(Box::new(Type::Dyn))));
    assert!(!strings.accepts(&Type::List(Box::new(Type::Int))));
    assert!(!Type::Message("t.Other".into()).accepts(&task));
}

#[test]
fn the_well_known_messages_are_cel_types() {
    assert_eq!(
        Type::of_message("google.protobuf.Timestamp"),
        Type::Timestamp
    );
    assert_eq!(Type::of_message("google.protobuf.Duration"), Type::Duration);
    assert_eq!(Type::of_message("google.protobuf.Any"), Type::Dyn);
    assert_eq!(
        Type::of_message("google.protobuf.StringValue"),
        Type::String
    );
    assert_eq!(Type::of_message("t.Task"), Type::Message("t.Task".into()));
    let cel = Type::Map(Box::new(Type::List(Box::new(Type::Message(
        "t.Task".into(),
    )))))
    .cel();
    assert_eq!(
        Type::of_cel(&cel),
        Some(Type::Map(Box::new(Type::List(Box::new(Type::Message(
            "t.Task".into()
        ))))))
    );
}
