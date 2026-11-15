// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! A request's path under a connection (docs/modules/http.md, requests):
//! its `{name}` places, each filled with one segment, percent-encoded, so a
//! value never adds a `/`, a `..` or a query.

use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};

use post_cortex_proto::http::Pair;

use crate::domain::Broken;

/// What a path segment's value keeps as it is: the unreserved characters.
const SEGMENT: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~');

/// A piece of a path: its text, or a `{name}` place.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Piece<'a> {
    /// Text, as it is.
    Text(&'a str),
    /// A place, by its name.
    Place(&'a str),
}

/// The pieces of `path`.
///
/// # Errors
/// A path that does not begin with `/`, has a query, a fragment or a `.`
/// or `..` segment, a brace without its pair, a place without a name.
pub fn pieces(path: &str) -> Result<Vec<Piece<'_>>, Broken> {
    if !path.starts_with('/') {
        return Err(Broken::new("path", "begins with /"));
    }
    if path.contains(['?', '#']) {
        return Err(Broken::new(
            "path",
            "has no query or fragment: the query is given apart",
        ));
    }
    if path.split('/').any(|s| s == "." || s == "..") {
        return Err(Broken::new("path", "has no . or .. segment"));
    }
    let mut pieces = Vec::new();
    let mut rest = path;
    while let Some(open) = rest.find('{') {
        let (text, after) = (&rest[..open], &rest[open + 1..]);
        if text.contains('}') {
            return Err(Broken::new("path", "has a } without its {"));
        }
        let close = after
            .find('}')
            .ok_or_else(|| Broken::new("path", "has a { without its }"))?;
        let name = &after[..close];
        if name.is_empty() || name.contains('{') {
            return Err(Broken::new("path", "names each {place}"));
        }
        if !text.is_empty() {
            pieces.push(Piece::Text(text));
        }
        pieces.push(Piece::Place(name));
        rest = &after[close + 1..];
    }
    if rest.contains('}') {
        return Err(Broken::new("path", "has a } without its {"));
    }
    if !rest.is_empty() {
        pieces.push(Piece::Text(rest));
    }
    Ok(pieces)
}

/// `path` with each `{name}` place filled from `params` (a name and a
/// value each), each value one segment.
///
/// # Errors
/// As [`pieces`]; a place without its value, a value without its place, a
/// place given twice, a value that is empty, `.` or `..`.
pub fn expand(path: &str, params: &[Pair]) -> Result<String, Broken> {
    for (i, p) in params.iter().enumerate() {
        if params[..i].iter().any(|q| q.name == p.name) {
            return Err(Broken::new(
                &format!("path_params[{i}].name"),
                format!("{} is given twice", p.name),
            ));
        }
    }
    let mut out = String::with_capacity(path.len());
    let mut used = Vec::new();
    for piece in pieces(path)? {
        match piece {
            Piece::Text(text) => out.push_str(text),
            Piece::Place(name) => {
                let field = format!("path_params.{name}");
                let value = &params
                    .iter()
                    .find(|p| p.name == name)
                    .ok_or_else(|| Broken::new(&field, "is required by the path"))?
                    .value;
                if value.is_empty() || value == "." || value == ".." {
                    return Err(Broken::new(&field, "is a segment: not empty, . or .."));
                }
                out.extend(utf8_percent_encode(value, SEGMENT));
                used.push(name);
            }
        }
    }
    if let Some(unused) = params.iter().find(|p| !used.contains(&p.name.as_str())) {
        return Err(Broken::new(
            &format!("path_params.{}", unused.name),
            "names no {place} of the path",
        ));
    }
    Ok(out)
}
