// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! REST requests: the route of the table they match, its path parameters
//! (named after the scope field they fill), the query and the JSON body as
//! one JSON object.

use post_cortex_memory::tools::operations::{OPERATIONS, Operation};
use serde_json::{Map, Value};

use post_cortex_memory::tools::operations::classify::of_json;

use crate::classify::{Classified, ClassifyError};

/// The operation of `method path` and the parameters of its route.
fn route(method: &str, path: &str) -> Option<(&'static Operation, Map<String, Value>)> {
    let segments: Vec<&str> = path.trim_matches('/').split('/').collect();
    OPERATIONS.iter().find_map(|op| {
        op.rest.iter().find_map(|route| {
            let (route_method, template) = route.split_once(' ')?;
            if !route_method.eq_ignore_ascii_case(method) {
                return None;
            }
            let parts: Vec<&str> = template.trim_matches('/').split('/').collect();
            if parts.len() != segments.len() {
                return None;
            }
            let mut params = Map::new();
            for (part, segment) in parts.iter().zip(&segments) {
                match part.strip_prefix('{').and_then(|p| p.strip_suffix('}')) {
                    Some(name) => {
                        let value =
                            percent_encoding::percent_decode_str(segment).decode_utf8_lossy();
                        params.insert(name.to_string(), Value::from(value.into_owned()));
                    }
                    None if part == segment => {}
                    None => return None,
                }
            }
            Some((op, params))
        })
    })
}

/// The REST request `method path?query` with JSON `body` (`Value::Null`
/// when it has none).
///
/// # Errors
/// An unknown route, a body that is not an object, or a field that names
/// no id.
pub fn rest(
    method: &str,
    path: &str,
    query: Option<&str>,
    body: &Value,
) -> Result<Classified, ClassifyError> {
    let (operation, mut fields) = route(method, path)
        .ok_or_else(|| ClassifyError::UnknownRoute(format!("{method} {path}")))?;
    for (key, value) in form_urlencoded::parse(query.unwrap_or_default().as_bytes()) {
        fields
            .entry(key.into_owned())
            .or_insert_with(|| Value::from(value.into_owned()));
    }
    match body {
        Value::Null => {}
        Value::Object(object) => {
            for (key, value) in object {
                fields.entry(key.clone()).or_insert_with(|| value.clone());
            }
        }
        _ => {
            return Err(ClassifyError::Malformed(
                "the body is not a JSON object".into(),
            ));
        }
    }
    of_json(operation, &Value::Object(fields))
}
