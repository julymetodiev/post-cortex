// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! What each block's expressions may read: the variables on every path to
//! it (docs/modules/workflow.md, the check). A block's variables are those
//! every edge into it carries with one type; an edge carries its block's
//! variables and the block's output (`steps.<id>`, its fields when it has
//! a schema; the failure on `error`; `trigger` from a trigger).

use std::collections::BTreeMap;
use std::sync::Arc;

use pcx_expr::{Expression, Inputs, Template, Type};
use post_cortex_proto::workflow::Failure;
use post_cortex_proto::workflow::block::Kind;

use crate::domain::blocks::{self, Context};
use crate::domain::data::Data;
use crate::domain::graph::{ERROR, Shape, ports};
use crate::domain::retry::failure;
use crate::domain::slots::Texts;
use crate::domain::{Catalog, Refusal, Tool};
use pcx_expr::schema::Field;

/// The message a trigger gives as `trigger`.
const FIRED: &str = "pcx.workflow.v1.Fired";

/// The message a block's `error` edge gives as its step.
const FAILURE: &str = "pcx.workflow.v1.Failure";

/// What a block gives: one value, or fields (a schema's, a `set`'s).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Out {
    /// One value of a type.
    One(Type),
    /// Named values.
    Fields(Vec<(String, Type)>),
}

/// The variables an expression may read, by name (`input.limit`,
/// `steps.fetch`, `steps.sum.total`, `trigger`, `item`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Vars(BTreeMap<String, Type>);

impl Vars {
    /// A run's input: `input.<field>`.
    pub fn input(fields: &[Field]) -> Self {
        Self(
            fields
                .iter()
                .map(|f| (format!("input.{}", f.name), f.ty.clone()))
                .collect(),
        )
    }

    /// These and variable `name` of `ty`, which hides one of that name.
    #[must_use]
    pub fn with(&self, name: &str, ty: Type) -> Self {
        let mut vars = self.clone();
        vars.0.insert(name.to_string(), ty);
        vars
    }

    /// These and block `id`'s output `out`.
    fn with_step(&self, id: &str, out: &Out) -> Self {
        let mut vars = self.clone();
        match out {
            Out::One(ty) => {
                vars.0.insert(format!("steps.{id}"), ty.clone());
            }
            Out::Fields(fields) => {
                for (name, ty) in fields {
                    vars.0.insert(format!("steps.{id}.{name}"), ty.clone());
                }
            }
        }
        vars
    }

    /// The variables both have with one type.
    fn meet(&self, other: &Self) -> Self {
        Self(
            self.0
                .iter()
                .filter(|(name, ty)| other.0.get(*name) == Some(ty))
                .map(|(name, ty)| (name.clone(), ty.clone()))
                .collect(),
        )
    }

    /// The variables as expressions are checked against, the catalog's
    /// messages known.
    ///
    /// # Errors
    /// A variable that does not declare (a message the catalog does not
    /// hold); why.
    pub fn inputs(&self, catalog: &Catalog) -> Result<Inputs, String> {
        let mut inputs = Inputs::with_messages(catalog.messages().clone());
        for (name, ty) in &self.0 {
            inputs
                .declare(name, ty.clone())
                .map_err(|e| e.to_string())?;
        }
        Ok(inputs)
    }
}

/// The variables every one of `all` has with one type; none when there
/// are none.
fn meet<'a>(all: impl Iterator<Item = &'a Vars>) -> Vars {
    all.fold(None, |met: Option<Vars>, v| {
        Some(met.map_or_else(|| v.clone(), |m| m.meet(v)))
    })
    .unwrap_or_default()
}

/// What the check found that a run reads, by block id, its bodies' and
/// branches' included.
#[derive(Debug, Clone, Default)]
pub struct Found {
    /// Each `tool` block's tool.
    pub tools: BTreeMap<String, Tool>,
    /// Each block's variables.
    pub vars: BTreeMap<String, Vars>,
    /// The variables where each body or branch ends, by its key: its
    /// `for_each`'s id, its `parallel`'s id and `#<n>`.
    pub ends: BTreeMap<String, Vars>,
    /// Each `call` block's called workflow's input.
    pub calls: BTreeMap<String, Vec<Field>>,
    /// Each block's expressions, compiled once, by its id and field.
    pub exprs: BTreeMap<(String, String), Expression>,
    /// Each block's texts with `{{ }}` parts, compiled once.
    pub texts: BTreeMap<(String, String), Template>,
    /// Each block's templates (a tool's request and fixture, a call's
    /// input), their texts compiled once.
    pub templates: BTreeMap<(String, String), Texts>,
    /// Each graph's output expression, by its end's key.
    pub outputs: BTreeMap<String, Expression>,
    /// Each data block (`filter`, `transform`, `validate`), compiled.
    pub data: BTreeMap<String, Arc<Data>>,
}

impl Found {
    /// These and `other`'s.
    pub fn extend(&mut self, other: Self) {
        self.tools.extend(other.tools);
        self.vars.extend(other.vars);
        self.ends.extend(other.ends);
        self.calls.extend(other.calls);
        self.exprs.extend(other.exprs);
        self.texts.extend(other.texts);
        self.templates.extend(other.templates);
        self.outputs.extend(other.outputs);
        self.data.extend(other.data);
    }

    /// Block `block`'s expression `field`, compiled when it was checked.
    ///
    /// # Errors
    /// It was not checked: an internal failure.
    pub fn expr(&self, block: &str, field: &str) -> Result<&Expression, Failure> {
        self.exprs
            .get(&(block.to_string(), field.to_string()))
            .ok_or_else(|| unchecked(block, field))
    }

    /// Block `block`'s template `field`'s texts, compiled when it was
    /// checked; none for a template it has not.
    pub fn template(&self, block: &str, field: &str) -> Option<&Texts> {
        self.templates.get(&(block.to_string(), field.to_string()))
    }

    /// Block `block`'s text `field`, compiled when it was checked.
    ///
    /// # Errors
    /// It was not checked: an internal failure.
    pub fn text(&self, block: &str, field: &str) -> Result<&Template, Failure> {
        self.texts
            .get(&(block.to_string(), field.to_string()))
            .ok_or_else(|| unchecked(block, field))
    }
}

/// A field a run reads that its check did not compile.
fn unchecked(block: &str, field: &str) -> Failure {
    failure(
        "internal",
        &format!("block {block}'s {field} was not checked"),
    )
}

/// What a graph's check found.
pub struct Flowed {
    /// What its blocks need at run time.
    pub found: Found,
    /// The variables where a run of the graph ends: those of every end.
    pub ends: Vars,
}

/// Every block of a graph of shape `shape` checked in order, its starts
/// with `entry`.
///
/// # Errors
/// The first block at fault.
pub fn check(context: &Context<'_>, shape: &Shape<'_>, entry: Vars) -> Result<Flowed, Refusal> {
    let mut leaving: BTreeMap<(String, String), Vars> = BTreeMap::new();
    let mut found = Found::default();
    for block in &shape.order {
        let vars = match shape.incoming.get(block.id.as_str()) {
            None => entry.clone(),
            Some(edges) => meet(
                edges
                    .iter()
                    .map(|(from, port)| &leaving[&((*from).to_string(), (*port).to_string())]),
            ),
        };
        let kind = block
            .kind
            .as_ref()
            .ok_or_else(|| Refusal::block(&block.id, "kind", "a block has a kind"))?;
        let checked = blocks::check(context, block, kind, &vars)?;
        found.extend(checked.found);
        found.vars.insert(block.id.clone(), vars.clone());
        for port in ports(kind) {
            let carried = if port == ERROR {
                vars.with_step(&block.id, &Out::One(Type::Message(FAILURE.to_string())))
            } else if matches!(kind, Kind::Trigger(_)) {
                vars.with("trigger", Type::Message(FIRED.to_string()))
            } else {
                vars.with_step(&block.id, &checked.out)
            };
            leaving.insert((block.id.clone(), port), carried);
        }
    }
    // A graph that ends only by failing has no variables at its end.
    let ends = meet(
        shape
            .ends
            .iter()
            .map(|(id, port)| &leaving[&((*id).to_string(), port.clone())]),
    );
    Ok(Flowed { found, ends })
}

/// `expression`, a graph's output, compiled over the variables where it
/// ends.
///
/// # Errors
/// The expression does not check; why.
pub fn output(catalog: &Catalog, expression: &str, ends: &Vars) -> Result<Expression, String> {
    Expression::check(expression, &ends.inputs(catalog)?).map_err(|e| e.to_string())
}
