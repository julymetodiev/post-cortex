// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! A response read and judged (docs/modules/http.md, requests and
//! failures): its body by its media type, its headers without cookies, a
//! status outside `ok` as the failure a workflow's retry reads, a JSON body
//! held to `expect`, and the secrets of its connection never shown.

use pcx_expr::schema::read_body;
use pcx_module_api::{ModuleErrorKind, json};
use post_cortex_proto::http::http_response::Body;
use serde_json::Value as Json;

use crate::domain::Broken;
use crate::domain::request::Expect;

/// Characters of a failed response's body its failure shows at most.
const FAILURE_BODY: usize = 512;

/// Whether `status` answers: one of `ok`, else a 2xx.
#[must_use]
pub fn answers(status: u16, ok: &[u16]) -> bool {
    if ok.is_empty() {
        (200..300).contains(&status)
    } else {
        ok.contains(&status)
    }
}

/// How a call fails on `status`, outside the statuses that answer.
#[must_use]
pub fn failure_kind(status: u16) -> ModuleErrorKind {
    match status {
        401 | 403 => ModuleErrorKind::Denied,
        404 => ModuleErrorKind::NotFound,
        408 | 429 | 500..=599 => ModuleErrorKind::Unavailable,
        _ => ModuleErrorKind::FailedPrecondition,
    }
}

/// The failure's message for `status` (with its `reason`) and the head of
/// its `body`, the `secrets` replaced.
#[must_use]
pub fn failure_message(status: u16, reason: &str, body: &[u8], secrets: &[String]) -> String {
    let text = String::from_utf8_lossy(body);
    let head: String = text.chars().take(FAILURE_BODY).collect();
    let head = redact(head.trim(), secrets);
    if head.is_empty() {
        format!("HTTP {status} {reason}")
    } else {
        format!("HTTP {status} {reason}: {head}")
    }
}

/// `text` with each of `secrets` replaced by `***`.
#[must_use]
pub fn redact(text: &str, secrets: &[String]) -> String {
    secrets
        .iter()
        .filter(|s| !s.is_empty())
        .fold(text.to_string(), |text, secret| {
            text.replace(secret.as_str(), "***")
        })
}

/// The headers a response shows: names in lower case, no cookies.
#[must_use]
pub fn shown_headers(headers: Vec<(String, String)>) -> Vec<(String, String)> {
    headers
        .into_iter()
        .map(|(name, value)| (name.to_ascii_lowercase(), value))
        .filter(|(name, _)| name != "set-cookie" && name != "set-cookie2")
        .collect()
}

/// A media type's essence: before its parameters, in lower case.
fn essence(content_type: &str) -> String {
    content_type
        .split_once(';')
        .map_or(content_type, |(essence, _)| essence)
        .trim()
        .to_ascii_lowercase()
}

/// Whether media type `essence` is JSON.
fn json_type(essence: &str) -> bool {
    essence == "application/json" || essence.ends_with("+json")
}

/// Whether media type `essence` is text.
fn text_type(essence: &str) -> bool {
    essence.starts_with("text/")
        || essence.ends_with("+xml")
        || matches!(
            essence,
            "application/xml" | "application/javascript" | "application/x-www-form-urlencoded"
        )
}

/// The body of `bytes` of `content_type`, with its JSON when it has
/// some: JSON when its media type says so and it parses, else text when
/// its media type is textual (or none, or JSON that is not) and it is
/// UTF-8, else bytes. A body that says JSON and is not is shown as the
/// text it is, and `expect` refuses it.
#[must_use]
pub fn read(content_type: &str, bytes: Vec<u8>) -> (Body, Option<Json>) {
    let essence = essence(content_type);
    if json_type(&essence)
        && let Ok(json) = serde_json::from_slice::<Json>(&bytes)
    {
        return (Body::Json(json::to_wire(json.clone())), Some(json));
    }
    let body = if essence.is_empty() || text_type(&essence) || json_type(&essence) {
        match String::from_utf8(bytes) {
            Ok(text) => Body::Text(text),
            Err(e) => Body::Raw(e.into_bytes()),
        }
    } else {
        Body::Raw(bytes)
    };
    (body, None)
}

/// Check `json`, a response's body, against `expect`.
///
/// # Errors
/// `FailedPrecondition` naming what does not fit: no JSON, not an object
/// (or a list of them), a field missing or of another type.
pub fn fits(expect: &Expect, json: Option<&Json>) -> Result<(), Broken> {
    let refused = |field: &str, why: String| Broken {
        kind: ModuleErrorKind::FailedPrecondition,
        field: field.to_string(),
        reason: why,
    };
    let Some(json) = json else {
        return Err(refused(
            "body",
            "is not the JSON `expect` describes".to_string(),
        ));
    };
    if expect.list {
        let Json::Array(items) = json else {
            return Err(refused("body", "is not a list".to_string()));
        };
        for (i, item) in items.iter().enumerate() {
            read_body(&expect.fields, item).map_err(|why| refused(&format!("body[{i}]"), why))?;
        }
        return Ok(());
    }
    read_body(&expect.fields, json)
        .map(|_| ())
        .map_err(|why| refused("body", why))
}

#[cfg(test)]
mod tests;
