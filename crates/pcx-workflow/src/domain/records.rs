// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! A schema's record type (docs/modules/workflow.md, data blocks): from a
//! `transform`'s output schema a proto message of its own,
//! `pcx.workflow.record.w_<workflow>.<block>` (and `<block>_group` for its
//! groups), made in the catalog of the version checked and never stored,
//! so a list of records is typed field by field.

use pcx_expr::Type;
use post_cortex_proto::workflow::{Block, Graph, Schema, Workflow, block::Kind};
use prost_reflect::DescriptorPool;
use prost_types::field_descriptor_proto::{Label, Type as Wire};
use prost_types::{DescriptorProto, FieldDescriptorProto, FileDescriptorProto, MessageOptions};

use crate::domain::{Catalog, Refusal};

/// The package of every workflow's records.
const PACKAGE: &str = "pcx.workflow.record";

/// The well-known messages a record's fields may be, by their files.
const WELL_KNOWN: [(&str, &str); 3] = [
    (
        "google.protobuf.Timestamp",
        "google/protobuf/timestamp.proto",
    ),
    ("google.protobuf.Duration", "google/protobuf/duration.proto"),
    ("google.protobuf.Value", "google/protobuf/struct.proto"),
];

/// The package of `workflow`'s records: its name, `-` as `_` (a name has
/// no `_`, so two names never meet).
fn package(workflow: &str) -> String {
    format!("{PACKAGE}.w_{}", workflow.replace('-', "_"))
}

/// The record type of block `block` of workflow `workflow`.
pub fn record(workflow: &str, block: &str) -> String {
    format!("{}.{block}", package(workflow))
}

/// The group type of block `block` of workflow `workflow`: its `key` and
/// its `items`, records.
pub fn group(workflow: &str, block: &str) -> String {
    format!("{}.{block}_group", package(workflow))
}

impl Catalog {
    /// The catalog with `workflow`'s records: one for each `transform` of
    /// it, of its bodies and branches too.
    ///
    /// # Errors
    /// A field of a schema no record holds (a list of lists, `dyn`
    /// without the well-known `Value`, a message the catalog does not
    /// hold), naming the block and the field.
    pub fn with_records(mut self, workflow: &Workflow) -> Result<Self, Refusal> {
        let mut file = FileDescriptorProto {
            name: Some(format!(
                "{}.proto",
                package(&workflow.name).replace('.', "/")
            )),
            package: Some(package(&workflow.name)),
            syntax: Some("proto3".into()),
            ..FileDescriptorProto::default()
        };
        let package = package(&workflow.name);
        if let Some(graph) = &workflow.graph {
            records_of(graph, &package, &self.messages, &mut file)?;
        }
        if file.message_type.is_empty() {
            return Ok(self);
        }
        self.messages.add_file_descriptor_proto(file).map_err(|e| {
            Refusal::workflow("graph", format!("its records do not read together: {e}"))
        })?;
        Ok(self)
    }
}

/// The records of `graph`'s `transform` blocks, of its bodies and branches
/// too, into `file`.
fn records_of(
    graph: &Graph,
    package: &str,
    pool: &DescriptorPool,
    file: &mut FileDescriptorProto,
) -> Result<(), Refusal> {
    for block in &graph.blocks {
        match &block.kind {
            Some(Kind::Transform(t)) => {
                let schema = t.output.clone().unwrap_or_default();
                add(block, &schema, package, pool, file)?;
            }
            Some(Kind::ForEach(each)) => {
                if let Some(body) = &each.body {
                    records_of(body, package, pool, file)?;
                }
            }
            Some(Kind::Parallel(p)) => {
                for branch in &p.branches {
                    records_of(branch, package, pool, file)?;
                }
            }
            _ => {}
        }
    }
    Ok(())
}

/// Block `block`'s record of `schema`, and its group, into `file`.
fn add(
    block: &Block,
    schema: &Schema,
    package: &str,
    pool: &DescriptorPool,
    file: &mut FileDescriptorProto,
) -> Result<(), Refusal> {
    let mut record = DescriptorProto {
        name: Some(block.id.clone()),
        ..DescriptorProto::default()
    };
    for (i, field) in schema.fields.iter().enumerate() {
        let at = format!("transform.output.fields[{i}].type");
        let ty = Type::parse(&field.r#type).map_err(|why| Refusal::block(&block.id, &at, why))?;
        let number = i32::try_from(i + 1)
            .map_err(|_| Refusal::block(&block.id, &at, "a record has too many fields"))?;
        let (proto, entry) = proto_field(&field.name, number, &ty, pool, file)
            .map_err(|why| Refusal::block(&block.id, &at, why))?;
        record.field.push(proto);
        record.nested_type.extend(entry);
    }
    let group = DescriptorProto {
        name: Some(format!("{}_group", block.id)),
        field: vec![
            scalar("key", 1, Wire::String, Label::Optional),
            FieldDescriptorProto {
                type_name: Some(format!(".{package}.{}", block.id)),
                ..scalar("items", 2, Wire::Message, Label::Repeated)
            },
        ],
        ..DescriptorProto::default()
    };
    file.message_type.push(record);
    file.message_type.push(group);
    Ok(())
}

/// A field of one wire type.
fn scalar(name: &str, number: i32, wire: Wire, label: Label) -> FieldDescriptorProto {
    FieldDescriptorProto {
        name: Some(name.to_string()),
        number: Some(number),
        label: Some(label as i32),
        r#type: Some(wire as i32),
        json_name: Some(name.to_string()),
        ..FieldDescriptorProto::default()
    }
}

/// Field `name` of type `ty` as a record holds it, and its map entry when
/// it is a map; the files it needs added to `file`'s dependencies.
///
/// # Errors
/// A type no record holds; why.
fn proto_field(
    name: &str,
    number: i32,
    ty: &Type,
    pool: &DescriptorPool,
    file: &mut FileDescriptorProto,
) -> Result<(FieldDescriptorProto, Option<DescriptorProto>), String> {
    match ty {
        Type::List(item) => {
            let (wire, message) = one(item, pool, file)?;
            Ok((
                FieldDescriptorProto {
                    type_name: message,
                    ..scalar(name, number, wire, Label::Repeated)
                },
                None,
            ))
        }
        Type::Map(value) => {
            let (wire, message) = one(value, pool, file)?;
            let entry_name = format!("{}Entry", camel(name));
            let entry = DescriptorProto {
                name: Some(entry_name.clone()),
                field: vec![
                    scalar("key", 1, Wire::String, Label::Optional),
                    FieldDescriptorProto {
                        type_name: message,
                        ..scalar("value", 2, wire, Label::Optional)
                    },
                ],
                options: Some(MessageOptions {
                    map_entry: Some(true),
                    ..MessageOptions::default()
                }),
                ..DescriptorProto::default()
            };
            Ok((
                FieldDescriptorProto {
                    // Relative to the record, which nests its entry.
                    type_name: Some(entry_name),
                    ..scalar(name, number, Wire::Message, Label::Repeated)
                },
                Some(entry),
            ))
        }
        other => {
            let (wire, message) = one(other, pool, file)?;
            Ok((
                FieldDescriptorProto {
                    type_name: message,
                    ..scalar(name, number, wire, Label::Optional)
                },
                None,
            ))
        }
    }
}

/// The wire type of one value of `ty`, with its message's name (a
/// message), its file a dependency of `file`.
///
/// # Errors
/// A list or a map inside a list or a map, null, or a message the pool
/// does not hold; why.
fn one(
    ty: &Type,
    pool: &DescriptorPool,
    file: &mut FileDescriptorProto,
) -> Result<(Wire, Option<String>), String> {
    let mut message = |full: &str| -> Result<(Wire, Option<String>), String> {
        let found = pool
            .get_message_by_name(full)
            .ok_or_else(|| format!("{full} is no message the workflow reaches"))?;
        let dependency = WELL_KNOWN
            .iter()
            .find(|(name, _)| *name == full)
            .map_or_else(
                || found.parent_file().name().to_string(),
                |(_, f)| (*f).to_string(),
            );
        if !file.dependency.contains(&dependency) {
            file.dependency.push(dependency);
        }
        Ok((Wire::Message, Some(format!(".{full}"))))
    };
    Ok(match ty {
        Type::String => (Wire::String, None),
        Type::Int => (Wire::Int64, None),
        Type::Uint => (Wire::Uint64, None),
        Type::Double => (Wire::Double, None),
        Type::Bool => (Wire::Bool, None),
        Type::Bytes => (Wire::Bytes, None),
        Type::Timestamp => message("google.protobuf.Timestamp")?,
        Type::Duration => message("google.protobuf.Duration")?,
        Type::Dyn => message("google.protobuf.Value")?,
        Type::Message(full) => message(full)?,
        Type::List(_) | Type::Map(_) => {
            return Err(format!(
                "a {ty} inside a list or a map: a record holds one level"
            ));
        }
        Type::Null => return Err("a record field is not null".to_string()),
    })
}

/// `name` as a message names a map's entry: `due_at` as `DueAt`.
fn camel(name: &str) -> String {
    name.split('_')
        .map(|part| {
            let mut chars = part.chars();
            chars.next().map_or_else(String::new, |c| {
                c.to_ascii_uppercase().to_string() + chars.as_str()
            })
        })
        .collect()
}
