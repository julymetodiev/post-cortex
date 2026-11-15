// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use serde_json::json;

use super::*;

#[test]
fn values_read_by_their_type() {
    let at = Value::read(&Type::Timestamp, &json!("2026-10-31T12:00:00+02:00")).unwrap();
    assert_eq!(
        at,
        Value::Timestamp(
            DateTime::parse_from_rfc3339("2026-10-31T10:00:00Z")
                .unwrap()
                .with_timezone(&Utc)
        )
    );
    assert_eq!(
        Value::read(&Type::Double, &json!(3)).unwrap(),
        Value::Double(3.0)
    );
    let rows = Type::parse("list<map<string, int>>").unwrap();
    assert_eq!(
        Value::read(&rows, &json!([{"a": 1}])).unwrap(),
        Value::list(vec![Value::map(BTreeMap::from([(
            "a".to_string(),
            Value::Int(1)
        )]))])
    );
    assert!(Value::read(&Type::Int, &json!(1.5)).is_err());
    assert_eq!(
        Value::read(&Type::Int, &json!(3.0)).unwrap(),
        Value::Int(3),
        "a whole number through a protobuf Value"
    );
    assert!(Value::read(&Type::Int, &json!(1e30)).is_err());
    assert!(Value::read(&Type::String, &json!(1)).is_err());
    assert!(Value::read(&Type::Timestamp, &json!("yesterday")).is_err());
    assert!(Value::read(&Type::List(Box::new(Type::Int)), &json!([1, "2"])).is_err());
    assert_eq!(
        Value::read(
            &Type::Dyn,
            &json!({"host": "db-1", "n": 2, "up": 0.5, "x": null, "tags": [true]})
        )
        .unwrap(),
        Value::map(BTreeMap::from([
            ("host".to_string(), Value::String("db-1".into())),
            ("n".to_string(), Value::Int(2)),
            ("up".to_string(), Value::Double(0.5)),
            ("x".to_string(), Value::Null),
            ("tags".to_string(), Value::list(vec![Value::Bool(true)])),
        ])),
        "any JSON as dyn"
    );
}

#[test]
fn well_known_messages_read_as_their_types_say() {
    let pool = prost_reflect::DescriptorPool::global();
    let of = |name: &str, json: Json| {
        let descriptor = pool.get_message_by_name(name).unwrap();
        let m = DynamicMessage::deserialize(descriptor.clone(), json).unwrap();
        Value::of_proto(
            &prost_reflect::Kind::Message(descriptor),
            &prost_reflect::Value::Message(m),
        )
        .unwrap()
    };
    let unset = DynamicMessage::new(pool.get_message_by_name("google.protobuf.Value").unwrap());
    assert_eq!(
        Value::of_proto(
            &prost_reflect::Kind::Message(unset.descriptor()),
            &prost_reflect::Value::Message(unset),
        )
        .unwrap(),
        Value::Null,
        "an unset JSON value, as a request's body that is not json: null, never a message without its kind"
    );
    assert_eq!(
        of("google.protobuf.Struct", json!({"a": 1, "b": [true, null]})),
        Value::map(BTreeMap::from([
            ("a".to_string(), Value::Int(1)),
            (
                "b".to_string(),
                Value::list(vec![Value::Bool(true), Value::Null])
            ),
        ]))
    );
    assert_eq!(
        of("google.protobuf.ListValue", json!(["x", 2.5])),
        Value::list(vec![Value::String("x".into()), Value::Double(2.5)])
    );
    assert_eq!(of("google.protobuf.Int64Value", json!("5")), Value::Int(5));
    assert_eq!(
        of("google.protobuf.StringValue", json!("hi")),
        Value::String("hi".into())
    );
    assert_eq!(
        Value::of_proto(
            &prost_reflect::Kind::Message(
                pool.get_message_by_name("google.protobuf.BoolValue")
                    .unwrap()
            ),
            &prost_reflect::Value::Message(DynamicMessage::new(
                pool.get_message_by_name("google.protobuf.BoolValue")
                    .unwrap()
            )),
        )
        .unwrap(),
        Value::Bool(false),
        "an unset wrapper is its zero value, as its type is no null"
    );
}
