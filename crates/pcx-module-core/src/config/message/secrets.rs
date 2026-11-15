// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Where a configuration's secrets lie (docs/host.md, settings;
//! docs/modules/http.md, the host's change): a way of fields from the root,
//! a list on it naming its elements by their key field, and the places a
//! way has in one configuration, each with its path (`token`,
//! `connections[github].secret`), read, set and cleared there.

use pbjson_types::Any;
use prost_reflect::{DynamicMessage, FieldDescriptor, Value as Field};

use crate::config::ConfigError;
use crate::config::message::{ConfigType, refused};

/// A field on a secret's way.
#[derive(Debug, Clone)]
pub struct Step {
    /// The field.
    pub field: FieldDescriptor,
    /// For a list of messages: the field of its element that names it.
    pub key: Option<FieldDescriptor>,
}

/// One place of a secret in a configuration.
#[derive(Debug, Clone)]
pub struct Place {
    /// Its path: the field names joined by `.`, a list's element named by
    /// its key in brackets.
    pub path: String,
    /// The fields to it from the root, a list's with the element's index.
    hops: Vec<(FieldDescriptor, Option<usize>)>,
}

impl ConfigType {
    /// The places of every secret in `message`, in field order.
    fn places(&self, message: &DynamicMessage) -> Vec<Place> {
        self.secrets
            .iter()
            .flat_map(|way| places(message, way))
            .collect()
    }

    /// `config` without its secrets, and the path of each secret it sets
    /// (`token`, `slack.token`, `connections[github].secret`), in field
    /// order.
    ///
    /// # Errors
    /// It is not a message of this type.
    pub fn redact(&self, config: &Any) -> Result<(Any, Vec<String>), ConfigError> {
        let mut message = self.decode(config)?;
        let mut set = Vec::new();
        for place in self.places(&message) {
            if get(&message, &place).is_some() {
                put(&mut message, &place, None);
                set.push(place.path);
            }
        }
        Ok((self.encode(&message), set))
    }

    /// The secrets `config` sets: each one's path (as [`ConfigType::redact`]
    /// names it) and value, in field order.
    ///
    /// # Errors
    /// It is not a message of this type.
    pub fn secrets(&self, config: &Any) -> Result<Vec<(String, String)>, ConfigError> {
        let message = self.decode(config)?;
        Ok(self
            .places(&message)
            .into_iter()
            .filter_map(|place| get(&message, &place).map(|value| (place.path, value)))
            .collect())
    }

    /// `config` with each of `values` (a secret's path and its value) put
    /// back, as a store that keeps them apart opens them.
    ///
    /// # Errors
    /// It is not a message of this type, or a path names no secret of it.
    pub fn with_secrets(
        &self,
        config: &Any,
        values: &[(String, String)],
    ) -> Result<Any, ConfigError> {
        let mut message = self.decode(config)?;
        let places = self.places(&message);
        for (path, value) in values {
            let place = places
                .iter()
                .find(|place| place.path == *path)
                .ok_or_else(|| refused(path.clone(), "is no secret of the configuration"))?;
            put(&mut message, place, Some(value.clone()));
        }
        Ok(self.encode(&message))
    }

    /// `change` as the configuration to apply over `current`: a secret
    /// `change` leaves unset keeps its current value (in a list, the value
    /// of the element with the same key), one `clear` names is removed.
    /// Check the result, not `change`.
    ///
    /// # Errors
    /// Either is not a message of this type, or `clear` names a setting
    /// that is no secret of either.
    pub fn keep_secrets(
        &self,
        change: &Any,
        clear: &[String],
        current: &Any,
    ) -> Result<Any, ConfigError> {
        let mut message = self.decode(change)?;
        let current = self.decode(current)?;
        let places = self.places(&message);
        let kept = self.places(&current);
        if let Some(other) = clear
            .iter()
            .find(|c| !places.iter().chain(&kept).any(|place| place.path == **c))
        {
            return Err(refused(other.clone(), "is no secret to clear"));
        }
        for place in &places {
            if clear.contains(&place.path) {
                put(&mut message, place, None);
            } else if get(&message, place).is_none()
                && let Some(value) = kept
                    .iter()
                    .find(|k| k.path == place.path)
                    .and_then(|k| get(&current, k))
            {
                put(&mut message, place, Some(value));
            }
        }
        Ok(self.encode(&message))
    }
}

/// The field names of `way` joined by `.`, as an error names a field.
pub fn names(way: &[Step]) -> String {
    way.iter()
        .map(|step| step.field.name())
        .collect::<Vec<_>>()
        .join(".")
}

/// The places `way` has in `message`: one, or one for each element of the
/// list on it.
pub fn places(message: &DynamicMessage, way: &[Step]) -> Vec<Place> {
    let mut found = Vec::new();
    walk(message, way, String::new(), Vec::new(), &mut found);
    found
}

fn walk(
    message: &DynamicMessage,
    way: &[Step],
    prefix: String,
    hops: Vec<(FieldDescriptor, Option<usize>)>,
    found: &mut Vec<Place>,
) {
    let Some((step, rest)) = way.split_first() else {
        return;
    };
    let named = if prefix.is_empty() {
        step.field.name().to_string()
    } else {
        format!("{prefix}.{}", step.field.name())
    };
    if rest.is_empty() {
        let mut hops = hops;
        hops.push((step.field.clone(), None));
        found.push(Place { path: named, hops });
        return;
    }
    let value = message.get_field(&step.field);
    match (value.as_ref(), &step.key) {
        (Field::List(items), Some(key)) => {
            for (index, item) in items.iter().enumerate() {
                let Field::Message(element) = item else {
                    continue;
                };
                let name = element.get_field(key);
                let Some(name) = name.as_str() else {
                    continue;
                };
                let mut hops = hops.clone();
                hops.push((step.field.clone(), Some(index)));
                walk(element, rest, format!("{named}[{name}]"), hops, found);
            }
        }
        (Field::Message(inner), None) => {
            let mut hops = hops;
            hops.push((step.field.clone(), None));
            walk(inner, rest, named, hops, found);
        }
        _ => {}
    }
}

/// The secret at `place` of `message`, when it is set.
pub fn get(message: &DynamicMessage, place: &Place) -> Option<String> {
    let (last, parents) = place.hops.split_last()?;
    let mut at = message.clone();
    for (field, index) in parents {
        let inner = match (at.get_field(field).into_owned(), index) {
            (Field::List(items), Some(i)) => items.into_iter().nth(*i)?,
            (value, None) => value,
            _ => return None,
        };
        let Field::Message(inner) = inner else {
            return None;
        };
        at = inner;
    }
    match at.get_field(&last.0).as_ref() {
        Field::String(value) if !value.is_empty() => Some(value.clone()),
        _ => None,
    }
}

/// Set the secret at `place` of `message` to `value`, or clear it.
pub fn put(message: &mut DynamicMessage, place: &Place, value: Option<String>) {
    let Some(((last, _), parents)) = place.hops.split_last() else {
        return;
    };
    let mut at = message;
    for (field, index) in parents {
        let inner = match (at.get_field_mut(field), index) {
            (Field::List(items), Some(i)) => match items.get_mut(*i) {
                Some(item) => item,
                None => return,
            },
            (value, None) => value,
            _ => return,
        };
        let Field::Message(inner) = inner else {
            return;
        };
        at = inner;
    }
    match value {
        Some(value) => at.set_field(last, Field::String(value)),
        None => at.clear_field(last),
    }
}
