// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use prost::Message;
use prost_reflect::DynamicMessage;
use prost_types::field_descriptor_proto::{Label, Type as Kind};
use prost_types::{DescriptorProto, FieldDescriptorProto, FileDescriptorProto, FileDescriptorSet};

use super::*;
use crate::expression::Expression;
use crate::template::Template;

/// A pool with `t.Task { string title = 1; repeated string tags = 2;
/// int64 points = 3; }`.
fn pool() -> DescriptorPool {
    let field = |name: &str, number, kind: Kind, label: Label| FieldDescriptorProto {
        name: Some(name.into()),
        number: Some(number),
        r#type: Some(kind.into()),
        label: Some(label.into()),
        ..Default::default()
    };
    DescriptorPool::from_file_descriptor_set(FileDescriptorSet {
        file: vec![FileDescriptorProto {
            name: Some("t.proto".into()),
            package: Some("t".into()),
            syntax: Some("proto3".into()),
            message_type: vec![DescriptorProto {
                name: Some("Task".into()),
                field: vec![
                    field("title", 1, Kind::String, Label::Optional),
                    field("tags", 2, Kind::String, Label::Repeated),
                    field("points", 3, Kind::Int64, Label::Optional),
                ],
                ..Default::default()
            }],
            ..Default::default()
        }],
    })
    .unwrap()
}

fn task(pool: &DescriptorPool) -> DynamicMessage {
    let mut task = DynamicMessage::new(pool.get_message_by_name("t.Task").unwrap());
    task.set_field_by_name("title", prost_reflect::Value::String("Ship it".into()));
    task.set_field_by_name(
        "tags",
        prost_reflect::Value::List(vec![
            prost_reflect::Value::String("a".into()),
            prost_reflect::Value::String("b".into()),
        ]),
    );
    task.set_field_by_name("points", prost_reflect::Value::I64(5));
    task
}

fn inputs() -> Inputs {
    let mut inputs = Inputs::with_messages(pool());
    inputs
        .declare("steps.fetch", Type::Message("t.Task".into()))
        .unwrap();
    inputs.declare("input.limit", Type::Int).unwrap();
    inputs
}

fn values() -> Values {
    Values::from([
        ("steps.fetch".to_string(), Value::Message(task(&pool()))),
        ("input.limit".to_string(), Value::Int(3)),
    ])
}

#[test]
fn a_qualified_message_variable_is_checked_and_read_by_its_fields() {
    let points = Expression::check("steps.fetch.points + input.limit", &inputs()).unwrap();
    assert_eq!(points.ty(), &Type::Int);
    assert_eq!(points.value(&values()).unwrap(), Value::Int(8));
    let tags = Expression::check("steps.fetch.tags.filter(t, t != 'a')", &inputs()).unwrap();
    assert_eq!(tags.ty(), &Type::List(Box::new(Type::String)));
    assert_eq!(
        tags.value(&values()).unwrap(),
        Value::list(vec![Value::String("b".into())])
    );
    let whole = Expression::check("steps.fetch", &inputs()).unwrap();
    assert_eq!(whole.ty(), &Type::Message("t.Task".into()));
    let Value::Message(read) = whole.value(&values()).unwrap() else {
        panic!("not a message");
    };
    assert_eq!(read.encode_to_vec(), task(&pool()).encode_to_vec());
}

#[test]
fn an_expression_that_names_what_is_not_declared_or_a_missing_field_fails_its_check() {
    for bad in [
        "steps.other.points",
        "steps.fetch.nope",
        "input.limit + 'a'",
    ] {
        let err = Expression::check(bad, &inputs()).err().unwrap();
        assert_eq!(err.kind, ExprErrorKind::Type, "{bad}");
    }
    let deep = format!("{}1{}", "(".repeat(40), ")".repeat(40));
    assert_eq!(
        Expression::check(&deep, &inputs()).err().unwrap().kind,
        ExprErrorKind::Limit
    );
}

#[test]
fn a_slot_of_one_whole_expression_gives_its_type_and_text_around_gives_a_string() {
    let whole = Template::slot("{{ steps.fetch.tags }}", &inputs()).unwrap();
    assert_eq!(whole.ty(), Type::List(Box::new(Type::String)));
    assert_eq!(
        whole.value(&values()).unwrap(),
        Value::list(vec![Value::String("a".into()), Value::String("b".into())])
    );
    let text = Template::slot("Done: {{ steps.fetch.title }}", &inputs()).unwrap();
    assert_eq!(text.ty(), Type::String);
    assert_eq!(
        text.value(&values()).unwrap(),
        Value::String("Done: Ship it".into())
    );
    assert!(Template::slot("Tags {{ steps.fetch.tags }}", &inputs()).is_err());
    assert!(Template::compile("{{ steps.fetch.tags }}", &inputs()).is_err());
}

#[test]
fn a_declaration_is_a_qualified_name_once_of_a_known_message() {
    let mut inputs = inputs();
    assert!(inputs.declare("steps.fetch", Type::Int).is_err());
    assert!(inputs.declare("steps.Bad", Type::Int).is_err());
    assert!(inputs.declare("steps..x", Type::Int).is_err());
    assert!(
        inputs
            .declare("steps.x", Type::Message("t.Nope".into()))
            .is_err()
    );
    assert!(Inputs::new([("steps.x".to_string(), Type::Int)]).is_err());
    inputs.declare("item", Type::Dyn).unwrap();
    assert_eq!(inputs.get("item"), Some(&Type::Dyn));
}

#[test]
fn durations_read_as_proto_json_writes_them_and_show_as_cel_does() {
    let ninety = Value::read(&Type::Duration, &serde_json::json!("90.5s")).unwrap();
    assert_eq!(
        ninety,
        Value::Duration(chrono::TimeDelta::new(90, 500_000_000).unwrap())
    );
    assert!(Value::read(&Type::Duration, &serde_json::json!("90")).is_err());
    let inputs = Inputs::new([("wait".to_string(), Type::Duration)]).unwrap();
    let shown = Template::compile("{{ wait + duration('30s') }}", &inputs).unwrap();
    let values = Values::from([("wait".to_string(), ninety)]);
    assert_eq!(
        shown.fill(&values, crate::template::Escape::None).unwrap(),
        "120.5s"
    );
}
