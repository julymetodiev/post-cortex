// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Values between the engine and a run: written as a workflow keeps them,
//! and a run's input read (docs/modules/workflow.md, expressions).

use std::collections::BTreeMap;

use pcx_expr::{Value as Expr, Values};
use pcx_module_api::{ModuleError, ModuleErrorKind};
use post_cortex_proto::workflow::Run;

use crate::domain::{self, Catalog};

/// `value` as a workflow keeps it.
///
/// # Errors
/// An item of a list or a map that is no value here: an internal failure.
pub fn written(value: &Expr) -> Result<post_cortex_proto::workflow::Value, ModuleError> {
    domain::write_value(value).map_err(|why| ModuleError::new(ModuleErrorKind::Internal, why))
}

/// `values`, named, as a workflow keeps them.
///
/// # Errors
/// As [`written`]'s.
pub fn written_map(
    values: &BTreeMap<String, Expr>,
) -> Result<post_cortex_proto::workflow::Map, ModuleError> {
    domain::write_map(values).map_err(|why| ModuleError::new(ModuleErrorKind::Internal, why))
}

/// A run's input as its expressions read it: `input.<field>`.
///
/// # Errors
/// A field that does not read: an internal failure.
pub fn run_input(run: &Run, catalog: &Catalog) -> Result<Values, ModuleError> {
    let mut values = Values::new();
    for (name, value) in run.input.as_ref().map(|m| &m.entries).into_iter().flatten() {
        let value = domain::read_value(value, catalog.messages()).map_err(|why| {
            ModuleError::new(
                ModuleErrorKind::Internal,
                format!("the run's input {name}: {why}"),
            )
        })?;
        values.insert(format!("input.{name}"), value);
    }
    Ok(values)
}
