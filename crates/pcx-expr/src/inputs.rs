// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! What expressions are checked against: variables, each a name and a
//! type (a template's inputs; a workflow's `input.<field>`,
//! `steps.<id>`, `trigger`), and the proto messages their types name.

use std::collections::BTreeMap;
use std::sync::Arc;

use cel_core::{Env, MapActivation};
use cel_core_proto::ProstProtoRegistry;
use prost_reflect::DescriptorPool;
use serde_json::{Map, Value as Json};

use crate::error::{ExprError, ExprErrorKind};
use crate::types::Type;
use crate::value::{Value, Values};

/// Words CEL reserves, which no variable may be named.
const RESERVED: [&str; 21] = [
    "as",
    "break",
    "const",
    "continue",
    "else",
    "false",
    "for",
    "function",
    "if",
    "import",
    "in",
    "let",
    "loop",
    "package",
    "namespace",
    "null",
    "return",
    "true",
    "var",
    "void",
    "while",
];

/// Characters of a name (or of each part of a qualified one) at most.
const MAX_NAME: usize = 64;

/// The variables expressions are checked against, and the proto messages
/// their types name.
#[derive(Debug, Clone)]
pub struct Inputs {
    vars: BTreeMap<String, Type>,
    messages: Option<DescriptorPool>,
}

impl Inputs {
    /// `declared`, each name a CEL identifier (a lowercase letter or `_`,
    /// then letters, digits and `_`, at most 64), unique and not reserved;
    /// no message types.
    ///
    /// # Errors
    /// `Input`, naming the first input at fault.
    pub fn new(declared: impl IntoIterator<Item = (String, Type)>) -> Result<Self, ExprError> {
        let mut inputs = Self {
            vars: BTreeMap::new(),
            messages: None,
        };
        for (name, ty) in declared {
            if !identifier(&name) {
                return Err(ExprError::named(
                    ExprErrorKind::Input,
                    &name,
                    "is no input name: a lowercase letter or _, then letters, digits and _, not a word CEL reserves",
                ));
            }
            inputs.insert(name, ty)?;
        }
        Ok(inputs)
    }

    /// No variables, the messages of `pool` known: a workflow's.
    #[must_use]
    pub fn with_messages(pool: DescriptorPool) -> Self {
        Self {
            vars: BTreeMap::new(),
            messages: Some(pool),
        }
    }

    /// Variable `name` of type `ty` declared too: an identifier, or
    /// identifiers joined by `.` (`steps.fetch`), which an expression reads
    /// as one name.
    ///
    /// # Errors
    /// `Input` for a name that is none, one declared already, or a message
    /// type the descriptors do not hold.
    pub fn declare(&mut self, name: &str, ty: Type) -> Result<(), ExprError> {
        if !name.split('.').all(identifier) {
            return Err(ExprError::named(
                ExprErrorKind::Input,
                name,
                "is no variable name: identifiers joined by .",
            ));
        }
        self.insert(name.to_string(), ty)
    }

    fn insert(&mut self, name: String, ty: Type) -> Result<(), ExprError> {
        if let Some(missing) = self.unknown_message(&ty) {
            return Err(ExprError::named(
                ExprErrorKind::Input,
                &name,
                format!("is of {missing}, which the descriptors do not hold"),
            ));
        }
        if self.vars.contains_key(&name) {
            return Err(ExprError::named(
                ExprErrorKind::Input,
                &name,
                "is declared twice",
            ));
        }
        self.vars.insert(name, ty);
        Ok(())
    }

    /// The first message `ty` names that is not known.
    fn unknown_message(&self, ty: &Type) -> Option<String> {
        match ty {
            Type::Message(name) => self
                .messages
                .as_ref()
                .is_none_or(|pool| pool.get_message_by_name(name).is_none())
                .then(|| name.clone()),
            Type::List(item) | Type::Map(item) => self.unknown_message(item),
            _ => None,
        }
    }

    /// The type of variable `name`, when it is declared.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&Type> {
        self.vars.get(name)
    }

    /// The messages known; none for inputs without them.
    #[must_use]
    pub fn messages(&self) -> Option<&DescriptorPool> {
        self.messages.as_ref()
    }

    /// `given`, read by the inputs' types: every input has a value of its
    /// type, and nothing else is given.
    ///
    /// # Errors
    /// `Value`, naming the first input missing, of another type, or not
    /// declared.
    pub fn read(&self, given: &Map<String, Json>) -> Result<Values, ExprError> {
        if let Some(extra) = given.keys().find(|k| !self.vars.contains_key(*k)) {
            return Err(ExprError::named(
                ExprErrorKind::Value,
                extra,
                "is no input of the template",
            ));
        }
        self.vars
            .iter()
            .map(|(name, ty)| {
                let json = given.get(name).ok_or_else(|| {
                    ExprError::named(ExprErrorKind::Value, name, format!("is missing: a {ty}"))
                })?;
                let value = Value::read(ty, json)
                    .map_err(|why| ExprError::named(ExprErrorKind::Value, name, why))?;
                Ok((name.clone(), value))
            })
            .collect()
    }

    /// The environment their expressions are checked in: CEL's standard
    /// library and its extensions, ours (lists and times), each variable,
    /// and the messages.
    pub fn env(&self) -> Env {
        let mut env = Env::with_standard_library()
            .with_all_extensions()
            .with_extension(crate::functions::all());
        if let Some(pool) = &self.messages {
            env = env.with_proto_registry(Arc::new(ProstProtoRegistry::from_pool(pool.clone())));
        }
        for (name, ty) in &self.vars {
            env.add_variable(name.clone(), ty.cel());
        }
        env
    }
}

/// `values` as CEL reads them.
pub fn activation(values: &Values) -> MapActivation {
    let mut activation = MapActivation::new();
    for (name, value) in values {
        activation.insert(name.clone(), value.cel());
    }
    activation
}

/// Whether `name` is a CEL identifier that is not reserved: a lowercase
/// letter or `_`, then letters, digits and `_`, at most 64.
fn identifier(name: &str) -> bool {
    let mut chars = name.chars();
    name.len() <= MAX_NAME
        && chars
            .next()
            .is_some_and(|c| c.is_ascii_lowercase() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
        && !RESERVED.contains(&name)
}

#[cfg(test)]
mod tests;
