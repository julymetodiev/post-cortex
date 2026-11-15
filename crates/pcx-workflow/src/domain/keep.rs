// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! What a block's check compiles and keeps for its runs (docs/modules/
//! workflow.md, data blocks: each expression of a version is compiled
//! once, when it is checked): its expressions and texts by field.

use pcx_expr::{Expression, Inputs, Template, Type};

use crate::domain::Refusal;
use crate::domain::blocks::Checked;
use crate::domain::flow::{Found, Out};

/// What a block's check compiles, kept for its runs: its expressions and
/// texts by field, and what else it found.
pub struct Keep<'a> {
    /// The block's id.
    pub id: &'a str,
    /// Its variables, as its expressions are checked against.
    pub inputs: Inputs,
    /// What it found.
    pub found: Found,
}

impl Keep<'_> {
    /// A refusal of `field`.
    pub fn refuse(&self, field: &str, message: impl Into<String>) -> Refusal {
        Refusal::block(self.id, field, message)
    }

    /// Text `field` (with `{{ }}` parts) compiled and kept; empty refused
    /// when `required`.
    ///
    /// # Errors
    /// It is empty but required, or does not compile.
    pub fn text(&mut self, field: &str, source: &str, required: bool) -> Result<(), Refusal> {
        if required && source.trim().is_empty() {
            return Err(self.refuse(field, "is empty"));
        }
        let template = Template::compile(source, &self.inputs)
            .map_err(|e| self.refuse(field, e.to_string()))?;
        self.found
            .texts
            .insert((self.id.to_string(), field.to_string()), template);
        Ok(())
    }

    /// Expression `field` compiled over `inputs` and kept: its type.
    ///
    /// # Errors
    /// It does not check.
    pub fn expr_in(&mut self, field: &str, source: &str, inputs: &Inputs) -> Result<Type, Refusal> {
        let expr =
            Expression::check(source, inputs).map_err(|e| self.refuse(field, e.to_string()))?;
        let ty = expr.ty().clone();
        self.found
            .exprs
            .insert((self.id.to_string(), field.to_string()), expr);
        Ok(ty)
    }

    /// Expression `field` compiled over the block's variables and kept.
    ///
    /// # Errors
    /// It does not check.
    pub fn expr(&mut self, field: &str, source: &str) -> Result<Type, Refusal> {
        let inputs = self.inputs.clone();
        self.expr_in(field, source, &inputs)
    }

    /// Condition `field` over `inputs` compiled and kept: a bool.
    ///
    /// # Errors
    /// It does not check, or gives no bool.
    pub fn condition_in(
        &mut self,
        field: &str,
        source: &str,
        inputs: &Inputs,
    ) -> Result<(), Refusal> {
        let ty = self.expr_in(field, source, inputs)?;
        if Type::Bool.accepts(&ty) {
            Ok(())
        } else {
            Err(self.refuse(field, format!("gives a {ty}, not a bool")))
        }
    }

    /// Condition `field` over the block's variables.
    ///
    /// # Errors
    /// It does not check, or gives no bool.
    pub fn condition(&mut self, field: &str, source: &str) -> Result<(), Refusal> {
        let inputs = self.inputs.clone();
        self.condition_in(field, source, &inputs)
    }

    /// The block gives `out`, with what it kept.
    pub fn gives(self, out: Out) -> Checked {
        Checked {
            out,
            found: self.found,
        }
    }
}
