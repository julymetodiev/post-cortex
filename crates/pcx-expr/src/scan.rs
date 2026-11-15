// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! A template cut into its parts: text, and `{{ <CEL> }}` expressions. An
//! expression ends at the first `}}` outside its strings and brackets, so
//! a map literal or a string with `}}` in it stays whole; its brackets
//! nest at most 32 deep, which also bounds the parser's recursion.

use crate::error::{ExprError, ExprErrorKind};

/// Brackets an expression may nest.
const MAX_NESTING: usize = 32;

/// A part of a template.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Piece<'a> {
    /// Text, as it is.
    Text(&'a str),
    /// An expression's source and its byte offset in the template.
    Expr(&'a str, usize),
}

/// The parts of `template`, in order.
///
/// # Errors
/// `Syntax` for a `{{` with no `}}` or with nothing in it; `Limit` for
/// brackets nested deeper than 32.
pub fn pieces(template: &str) -> Result<Vec<Piece<'_>>, ExprError> {
    let mut parts = Vec::new();
    let mut rest = 0;
    while let Some(found) = template[rest..].find("{{") {
        let open = rest + found;
        if open > rest {
            parts.push(Piece::Text(&template[rest..open]));
        }
        let start = open + 2;
        let end = close(template, start, open)?;
        let source = &template[start..end];
        if source.trim().is_empty() {
            return Err(ExprError::at(
                ExprErrorKind::Syntax,
                open,
                "{{ }} has no expression",
            ));
        }
        parts.push(Piece::Expr(source, start));
        rest = end + 2;
    }
    if rest < template.len() {
        parts.push(Piece::Text(&template[rest..]));
    }
    Ok(parts)
}

/// The offset of the `}}` that closes the expression starting at
/// `start` (its `{{` at `open`).
fn close(template: &str, start: usize, open: usize) -> Result<usize, ExprError> {
    walk(template, start, open, true)?
        .ok_or_else(|| ExprError::at(ExprErrorKind::Syntax, open, "{{ is not closed with }}"))
}

/// That `expression`, a CEL expression alone, nests its brackets at most
/// 32 deep and closes its strings.
///
/// # Errors
/// `Limit` for brackets nested deeper; `Syntax` for a string not closed.
pub fn nesting(expression: &str) -> Result<(), ExprError> {
    walk(expression, 0, 0, false).map(|_| ())
}

/// `source` walked from `start` over its strings and brackets (the
/// expression's `{{` at `open`): the offset of the first `}}` outside them
/// when `to_close`, else none at its end.
fn walk(
    source: &str,
    start: usize,
    open: usize,
    to_close: bool,
) -> Result<Option<usize>, ExprError> {
    let bytes = source.as_bytes();
    let mut depth = 0usize;
    let mut i = start;
    while i < bytes.len() {
        match bytes[i] {
            b'"' | b'\'' => i = string_end(bytes, i, open)?,
            b'(' | b'[' | b'{' => {
                depth += 1;
                if depth > MAX_NESTING {
                    return Err(ExprError::at(
                        ExprErrorKind::Limit,
                        i,
                        format!("brackets nest more than {MAX_NESTING} deep"),
                    ));
                }
                i += 1;
            }
            b'}' if to_close && depth == 0 && bytes.get(i + 1) == Some(&b'}') => {
                return Ok(Some(i));
            }
            b')' | b']' | b'}' => {
                depth = depth.saturating_sub(1);
                i += 1;
            }
            _ => i += 1,
        }
    }
    Ok(None)
}

/// The offset just past the string literal whose quote is at `at`: one
/// quote or three, raw (`r` before it) or with `\` escapes.
fn string_end(bytes: &[u8], at: usize, open: usize) -> Result<usize, ExprError> {
    let quote = bytes[at];
    let raw = at > 0 && matches!(bytes[at - 1], b'r' | b'R');
    let triple = bytes.get(at + 1) == Some(&quote) && bytes.get(at + 2) == Some(&quote);
    let delimiter = if triple { 3 } else { 1 };
    let mut i = at + delimiter;
    while i < bytes.len() {
        if !raw && bytes[i] == b'\\' {
            i += 2;
            continue;
        }
        let closes = (0..delimiter).all(|k| bytes.get(i + k) == Some(&quote));
        if closes {
            return Ok(i + delimiter);
        }
        if !triple && bytes[i] == b'\n' {
            break;
        }
        i += 1;
    }
    Err(ExprError::at(
        ExprErrorKind::Syntax,
        open,
        "a string in {{ }} is not closed",
    ))
}

#[cfg(test)]
mod tests;
