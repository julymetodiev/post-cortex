// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! A `Template` checked against what it builds (docs/modules/workflow.md,
//! templates): a request message by its fields' proto names, nested as it
//! nests, or a called workflow's input by its schema's fields. Each slot is
//! a constant of the field's type, a text whose type is the field's, a
//! message for a message field or a list for a repeated one.

use std::collections::BTreeMap;

use pcx_expr::{Inputs, Template as Text, Type};
use post_cortex_proto::workflow::{Slot, Template, slot};
use prost_reflect::{FieldDescriptor, Kind, MessageDescriptor};

use crate::domain::value::type_of;
use pcx_expr::schema::Field;

/// A template's texts, compiled when it is checked, by the path of the
/// field each fills (`task.title`, `tags[1]`): a run fills them without
/// compiling them again.
#[derive(Debug, Clone, Default)]
pub struct Texts(BTreeMap<String, Text>);

impl Texts {
    /// The text that fills the field at `path`.
    pub fn get(&self, path: &str) -> Option<&Text> {
        self.0.get(path)
    }
}

/// What a template builds.
pub enum Target<'a> {
    /// A proto message.
    Message(MessageDescriptor),
    /// A schema's fields (a called workflow's input).
    Fields(&'a [Field]),
}

/// What a field takes.
enum Shape {
    /// A value of a type (a scalar, a well-known message, a map).
    Value(Type),
    /// A message, by its fields.
    Message(MessageDescriptor),
    /// A repeated field: its items' shape and its type.
    List(Box<Shape>, Type),
}

impl Shape {
    /// The shape of proto field `field`.
    fn of_field(field: &FieldDescriptor) -> Result<Self, String> {
        let ty = Type::of_field(field)?;
        if field.is_map() {
            return Ok(Self::Value(ty));
        }
        let item = match field.kind() {
            Kind::Message(m) if matches!(Type::of_message(m.full_name()), Type::Message(_)) => {
                Self::Message(m)
            }
            kind => Self::Value(Type::of_kind(&kind)),
        };
        Ok(if field.is_list() {
            Self::List(Box::new(item), ty)
        } else {
            item
        })
    }

    /// The shape of a schema's field of type `ty`.
    fn of_type(ty: &Type) -> Self {
        match ty {
            Type::List(item) => Self::List(Box::new(Self::of_type(item)), ty.clone()),
            ty => Self::Value(ty.clone()),
        }
    }

    /// The type of a value that fills it.
    fn ty(&self) -> Type {
        match self {
            Self::Value(ty) | Self::List(_, ty) => ty.clone(),
            Self::Message(m) => Type::Message(m.full_name().to_string()),
        }
    }
}

/// `template` checked against `target` with `inputs` its expressions'
/// variables.
///
/// # Errors
/// The field at fault (`task.title`, `tags[1]`) and why.
pub fn check(
    template: &Template,
    target: &Target<'_>,
    inputs: &Inputs,
) -> Result<Texts, (String, String)> {
    let mut texts = Texts::default();
    match target {
        Target::Message(message) => fields(template, message, inputs, "", &mut texts)?,
        Target::Fields(schema) => {
            for (name, slot) in sorted(template) {
                let field = schema.iter().find(|f| f.name == *name).ok_or_else(|| {
                    (
                        name.clone(),
                        "is no field of the workflow's input".to_string(),
                    )
                })?;
                one(slot, &Shape::of_type(&field.ty), inputs, name, &mut texts)?;
            }
            if let Some(missing) = schema
                .iter()
                .find(|f| f.required && !template.fields.contains_key(&f.name))
            {
                return Err((missing.name.clone(), "is required".to_string()));
            }
        }
    }
    Ok(texts)
}

/// The slots of `template` by name, in order.
fn sorted(template: &Template) -> BTreeMap<&String, &Slot> {
    template.fields.iter().collect()
}

/// `template`'s fields checked as fields of `message`, `at` the path to
/// it.
fn fields(
    template: &Template,
    message: &MessageDescriptor,
    inputs: &Inputs,
    at: &str,
    texts: &mut Texts,
) -> Result<(), (String, String)> {
    let mut oneofs: BTreeMap<String, &str> = BTreeMap::new();
    for (name, slot) in sorted(template) {
        let path = if at.is_empty() {
            name.clone()
        } else {
            format!("{at}.{name}")
        };
        let field = message.get_field_by_name(name).ok_or_else(|| {
            (
                path.clone(),
                format!("is no field of {}", message.full_name()),
            )
        })?;
        if let Some(oneof) = field.containing_oneof().filter(|o| !o.is_synthetic())
            && let Some(other) = oneofs.insert(oneof.name().to_string(), name)
        {
            return Err((
                path,
                format!(
                    "{other} is set too, and only one of {} may be",
                    oneof.name()
                ),
            ));
        }
        let shape = Shape::of_field(&field).map_err(|why| (path.clone(), why))?;
        one(slot, &shape, inputs, &path, texts)?;
    }
    Ok(())
}

/// `slot` checked as filling `shape`, `at` its path.
fn one(
    slot: &Slot,
    shape: &Shape,
    inputs: &Inputs,
    at: &str,
    texts: &mut Texts,
) -> Result<(), (String, String)> {
    let refuse = |why: String| (at.to_string(), why);
    let want = shape.ty();
    match slot
        .slot
        .as_ref()
        .ok_or_else(|| refuse("a slot is a constant, a text, a message or a list".to_string()))?
    {
        slot::Slot::Constant(value) => {
            let messages = inputs
                .messages()
                .ok_or_else(|| refuse("a message is read without descriptors".to_string()))?;
            let given = type_of(value, messages).map_err(refuse)?;
            if !want.accepts(&given) {
                return Err(refuse(format!("is a {given}, not a {want}")));
            }
        }
        slot::Slot::Text(source) => {
            let text = Text::slot(source, inputs).map_err(|e| refuse(e.to_string()))?;
            let given = text.ty();
            if !want.accepts(&given) {
                return Err(refuse(format!("gives a {given}, not a {want}")));
            }
            texts.0.insert(at.to_string(), text);
        }
        slot::Slot::Message(nested) => match shape {
            Shape::Message(message) => fields(nested, message, inputs, at, texts)?,
            _ => {
                return Err(refuse(format!(
                    "is a {want}, not a message to build by its fields"
                )));
            }
        },
        slot::Slot::List(list) => match shape {
            Shape::List(item, _) => {
                for (i, slot) in list.items.iter().enumerate() {
                    one(slot, item, inputs, &format!("{at}[{i}]"), texts)?;
                }
            }
            _ => return Err(refuse(format!("is a {want}, not a list"))),
        },
    }
    Ok(())
}
