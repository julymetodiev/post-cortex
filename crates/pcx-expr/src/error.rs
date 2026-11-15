// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! What a template or its values break, and where.

/// What went wrong.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExprErrorKind {
    /// The template or an expression does not parse (an unclosed `{{`).
    Syntax,
    /// An expression does not type-check against the inputs, or gives a
    /// value text cannot show.
    Type,
    /// An expression is over a limit (its length, its AST).
    Limit,
    /// An input is declared wrong (its name, its type).
    Input,
    /// A value is missing, not declared, or not of its input's type.
    Value,
    /// An evaluation failed (a division by zero, a missing key).
    Eval,
    /// An evaluation spent its budget of steps.
    Budget,
}

/// A failure, with where it is: the byte offset in the template (or the
/// input's or value's name).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{at}: {message}")]
pub struct ExprError {
    /// What went wrong.
    pub kind: ExprErrorKind,
    /// Where: `offset <n>` in the template, or the name at fault.
    pub at: String,
    /// What is wrong, for a person.
    pub message: String,
}

impl ExprError {
    /// A failure of `kind` at byte `offset` of the template.
    pub fn at(kind: ExprErrorKind, offset: usize, message: impl Into<String>) -> Self {
        Self {
            kind,
            at: format!("offset {offset}"),
            message: message.into(),
        }
    }

    /// A failure of `kind` of input or value `name`.
    pub fn named(kind: ExprErrorKind, name: &str, message: impl Into<String>) -> Self {
        Self {
            kind,
            at: name.to_string(),
            message: message.into(),
        }
    }
}
