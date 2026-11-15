// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use prost_reflect::DescriptorPool;
use prost_reflect::prost_types::field_descriptor_proto::{Label, Type};
use prost_reflect::prost_types::{
    DescriptorProto, FieldDescriptorProto, FileDescriptorProto, FileDescriptorSet,
};

use super::*;

fn field(
    name: &str,
    number: i32,
    ty: Type,
    message: Option<&str>,
    label: Label,
) -> FieldDescriptorProto {
    FieldDescriptorProto {
        name: Some(name.into()),
        number: Some(number),
        r#type: Some(ty as i32),
        type_name: message.map(|m| format!(".t.{m}")),
        label: Some(label as i32),
        ..FieldDescriptorProto::default()
    }
}

fn message(name: &str, fields: Vec<FieldDescriptorProto>) -> DescriptorProto {
    DescriptorProto {
        name: Some(name.into()),
        field: fields,
        ..DescriptorProto::default()
    }
}

/// `Root` holds `Shared` twice, `Once` once and the tree `Node`, which
/// holds itself.
fn root() -> MessageDescriptor {
    let one = Label::Optional;
    let file = FileDescriptorProto {
        name: Some("t.proto".into()),
        package: Some("t".into()),
        syntax: Some("proto3".into()),
        message_type: vec![
            message(
                "Root",
                vec![
                    field("a", 1, Type::Message, Some("Shared"), one),
                    field("b", 2, Type::Message, Some("Shared"), Label::Repeated),
                    field("once", 3, Type::Message, Some("Once"), one),
                    field("tree", 4, Type::Message, Some("Node"), one),
                ],
            ),
            message("Shared", vec![field("x", 1, Type::String, None, one)]),
            message("Once", vec![field("y", 1, Type::Int32, None, one)]),
            message(
                "Node",
                vec![field(
                    "children",
                    1,
                    Type::Message,
                    Some("Node"),
                    Label::Repeated,
                )],
            ),
        ],
        ..FileDescriptorProto::default()
    };
    let pool =
        DescriptorPool::from_file_descriptor_set(FileDescriptorSet { file: vec![file] }).unwrap();
    pool.get_message_by_name("t.Root").unwrap()
}

#[test]
fn a_shared_or_recursive_message_is_written_once_and_one_held_once_in_place() {
    let schema = Value::Object(object(&root()));
    let shared = json!({ "type": "object", "$ref": "#/$defs/t.Shared" });
    assert_eq!(schema["properties"]["a"], shared);
    assert_eq!(schema["properties"]["b"]["items"], shared);
    assert_eq!(
        schema["$defs"]["t.Shared"],
        json!({ "type": "object", "properties": { "x": { "type": "string" } } })
    );
    assert_eq!(
        schema["properties"]["once"],
        json!({ "type": "object", "properties": { "y": { "type": "integer" } } })
    );
    let node = json!({ "type": "object", "$ref": "#/$defs/t.Node" });
    assert_eq!(schema["properties"]["tree"], node);
    assert_eq!(
        schema["$defs"]["t.Node"]["properties"]["children"]["items"],
        node
    );
    assert!(schema["$defs"].get("t.Once").is_none());
}
