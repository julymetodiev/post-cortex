// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! A template of text and `{{ }}` parts: compiled when it is saved, every
//! expression checked against the inputs' types; filled when it is used,
//! each value put in as it is or HTML-escaped. A template that is one
//! whole `{{ }}` may give a value of any type (a workflow's field).

use cel_core::Activation;

use crate::bound::Local;
use crate::error::{ExprError, ExprErrorKind};
use crate::expression::{Expression, on_compile_stack};
use crate::inputs::{Inputs, activation};
use crate::scan::{Piece, pieces};
use crate::types::Type;
use crate::value::{Value, Values};

/// How a value is put into the text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Escape {
    /// As it is (a subject, a text body).
    None,
    /// HTML-escaped (an HTML body).
    Html,
}

/// A part of a compiled template.
#[derive(Debug, Clone)]
enum Part {
    Text(String),
    Expr(Expression),
}

/// A template, compiled.
#[derive(Debug, Clone)]
pub struct Template {
    parts: Vec<Part>,
    /// Whether it is one whole expression, of any type.
    whole: bool,
}

impl Template {
    /// `source` compiled against `inputs`: every `{{ }}` parsed, held to
    /// the limits and type-checked, each giving a value text shows.
    ///
    /// # Errors
    /// The first part at fault, with its byte offset.
    pub fn compile(source: &str, inputs: &Inputs) -> Result<Self, ExprError> {
        Self::build(source, inputs, false)
    }

    /// `source`, a field's template, compiled against `inputs`: as
    /// [`Template::compile`], but one whole `{{ }}` (nothing around it)
    /// gives a value of any type.
    ///
    /// # Errors
    /// The first part at fault, with its byte offset.
    pub fn slot(source: &str, inputs: &Inputs) -> Result<Self, ExprError> {
        Self::build(source, inputs, true)
    }

    fn build(source: &str, inputs: &Inputs, any: bool) -> Result<Self, ExprError> {
        let pieces = pieces(source)?;
        let whole = any && matches!(pieces.as_slice(), [Piece::Expr(..)]);
        on_compile_stack(|| {
            let env = inputs.env();
            let parts = pieces
                .into_iter()
                .map(|piece| match piece {
                    Piece::Text(text) => Ok(Part::Text(text.to_string())),
                    Piece::Expr(expr, at) => {
                        let expr = Expression::compile(&env, expr, at)?;
                        if !whole && !expr.ty().shown() {
                            return Err(ExprError::at(
                                ExprErrorKind::Type,
                                at,
                                format!(
                                    "gives a {}; text shows a string, a number, a bool, a time or a duration",
                                    expr.ty()
                                ),
                            ));
                        }
                        Ok(Part::Expr(expr))
                    }
                })
                .collect::<Result<_, _>>()?;
            Ok(Self { parts, whole })
        })
    }

    /// The type of its value: the expression's when it is one whole
    /// `{{ }}` of a slot, else text.
    #[must_use]
    pub fn ty(&self) -> Type {
        match (self.whole, self.parts.as_slice()) {
            (true, [Part::Expr(expr)]) => expr.ty().clone(),
            _ => Type::String,
        }
    }

    /// Its value with `values`: the expression's when it is one whole
    /// `{{ }}` of a slot, else its text.
    ///
    /// # Errors
    /// The first expression that fails, spends its budget or gives a
    /// value text cannot show.
    pub fn value(&self, values: &Values) -> Result<Value, ExprError> {
        match (self.whole, self.parts.as_slice()) {
            (true, [Part::Expr(expr)]) => expr.value(values),
            _ => self.fill(values, Escape::None).map(Value::string),
        }
    }

    /// The text of the template with `values`, each expression's value put
    /// in as `escape` says.
    ///
    /// # Errors
    /// The first expression that fails, spends its budget or gives a
    /// value text cannot show.
    pub fn fill(&self, values: &Values, escape: Escape) -> Result<String, ExprError> {
        self.write(&activation(values), escape)
    }

    /// The text of the template over `local` (a block's variables,
    /// converted once), as it is.
    ///
    /// # Errors
    /// As [`Template::fill`]'s.
    pub fn text_in(&self, local: &Local<'_, '_>) -> Result<String, ExprError> {
        self.write(local, Escape::None)
    }

    fn write(&self, activation: &dyn Activation, escape: Escape) -> Result<String, ExprError> {
        let mut text = String::new();
        for part in &self.parts {
            match part {
                Part::Text(t) => text.push_str(t),
                Part::Expr(expr) => {
                    let value = expr.text(activation)?;
                    match escape {
                        Escape::None => text.push_str(&value),
                        Escape::Html => html(&value, &mut text),
                    }
                }
            }
        }
        Ok(text)
    }
}

/// `value` HTML-escaped onto `out`.
fn html(value: &str, out: &mut String) {
    for c in value.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
}

#[cfg(test)]
mod tests;
