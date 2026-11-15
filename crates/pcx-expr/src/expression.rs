// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! One CEL expression: parsed, held to the limits, checked against the
//! inputs, and evaluated within a budget of steps
//! (docs/modules/workflow.md, expressions).

use cel_core::{
    Activation, Ast, Budget, Env, EvalErrorKind, Expr, Program, SpannedExpr, Value as Cel,
};

use crate::bound::{Items, Local, Meter};
use crate::error::{ExprError, ExprErrorKind};
use crate::inputs::{Inputs, activation};
use crate::scan::nesting;
use crate::types::Type;
use crate::value::{Value, Values, text};

/// The stack expressions are compiled on.
const COMPILE_STACK: usize = 64 * 1024 * 1024;

/// Bytes of an expression at most.
const MAX_SOURCE: usize = 4 * 1024;

/// Nodes of an expression's AST at most, and how deep it nests.
const MAX_NODES: usize = 1_000;
const MAX_DEPTH: usize = 32;

/// Steps an evaluation may take: each node visited and each iteration of
/// a comprehension.
const STEPS: u64 = 100_000;

/// An expression, checked, ready to evaluate.
#[derive(Debug, Clone)]
pub struct Expression {
    program: Program,
    /// Its byte offset in its template; 0 alone.
    at: usize,
    /// The type of its value.
    ty: Type,
}

impl Expression {
    /// `source`, a CEL expression alone (a condition, a list), checked
    /// against `inputs`.
    ///
    /// # Errors
    /// `Limit` over 4 KiB, 1,000 nodes or 32 deep, its brackets over 32
    /// deep; `Syntax` when it does not parse; `Type` when it does not
    /// check, or gives a value no value here is.
    pub fn check(source: &str, inputs: &Inputs) -> Result<Self, ExprError> {
        nesting(source)?;
        on_compile_stack(|| Self::compile(&inputs.env(), source, 0))
    }

    /// The type of its value.
    #[must_use]
    pub fn ty(&self) -> &Type {
        &self.ty
    }

    /// Its value with `values`.
    ///
    /// # Errors
    /// `Budget` when it spent its steps; `Eval` when it failed or gave a
    /// value no value here is.
    pub fn value(&self, values: &Values) -> Result<Value, ExprError> {
        let value = self.eval(&activation(values))?;
        Value::of_cel(&value).map_err(|why| ExprError::at(ExprErrorKind::Eval, self.at, why))
    }

    /// Its value over `local` (a block's variables, converted once), its
    /// steps counted on `meter`: each evaluation has its own budget, and
    /// the block's meter bounds them all.
    ///
    /// # Errors
    /// `Budget` when it spent its steps or the meter's; `Eval` when it
    /// failed or gave a value no value here is.
    pub fn value_in(&self, local: &Local<'_, '_>, meter: &mut Meter) -> Result<Value, ExprError> {
        let value = self.metered(local, meter)?;
        Value::of_cel(&value).map_err(|why| ExprError::at(ExprErrorKind::Eval, self.at, why))
    }

    /// Whether it holds over `local`, its steps counted on `meter`.
    ///
    /// # Errors
    /// As [`Expression::value_in`]'s; `Eval` when it gave no bool.
    pub fn holds_in(&self, local: &Local<'_, '_>, meter: &mut Meter) -> Result<bool, ExprError> {
        match self.metered(local, meter)? {
            Cel::Bool(holds) => Ok(holds),
            other => Err(ExprError::at(
                ExprErrorKind::Eval,
                self.at,
                format!("gave a {}, not a bool", other.cel_type().display_name()),
            )),
        }
    }

    /// The list it gives over `local`, kept as CEL holds it.
    ///
    /// # Errors
    /// As [`Expression::value_in`]'s; `Eval` when it gave no list.
    pub fn items_in(&self, local: &Local<'_, '_>, meter: &mut Meter) -> Result<Items, ExprError> {
        match self.metered(local, meter)? {
            Cel::List(items) => Ok(Items::new(items)),
            other => Err(ExprError::at(
                ExprErrorKind::Eval,
                self.at,
                format!("gave a {}, not a list", other.cel_type().display_name()),
            )),
        }
    }

    /// Its CEL value over `local`, within its budget and what `meter` has
    /// left.
    fn metered(&self, local: &Local<'_, '_>, meter: &mut Meter) -> Result<Cel, ExprError> {
        let steps = STEPS.min(meter.left());
        let budget = Budget::new(steps);
        let value = self.program.eval_with_budget(local, &budget);
        meter.spend(steps - budget.left());
        match value {
            Cel::Error(e) if e.kind == EvalErrorKind::BudgetExceeded && steps < STEPS => {
                Err(ExprError::at(
                    ExprErrorKind::Budget,
                    self.at,
                    format!("the block spent its {} steps", meter.all()),
                ))
            }
            Cel::Error(e) => Err(ExprError::at(
                if e.kind == EvalErrorKind::BudgetExceeded {
                    ExprErrorKind::Budget
                } else {
                    ExprErrorKind::Eval
                },
                self.at,
                e.message.clone(),
            )),
            value => Ok(value),
        }
    }

    /// `source`, at byte `at` of its template, compiled in `env`.
    ///
    /// # Errors
    /// `Limit` over 4 KiB, 1,000 nodes or 32 deep; `Syntax` when it does
    /// not parse; `Type` when it does not check, or gives a value no value
    /// here is.
    pub fn compile(env: &Env, source: &str, at: usize) -> Result<Self, ExprError> {
        if source.len() > MAX_SOURCE {
            return Err(ExprError::at(
                ExprErrorKind::Limit,
                at,
                format!("an expression is at most {MAX_SOURCE} bytes"),
            ));
        }
        let parsed = env.parse(source);
        if let Some(first) = parsed.errors.first() {
            return Err(ExprError::at(
                ExprErrorKind::Syntax,
                at + first.span.start,
                first.message.clone(),
            ));
        }
        let expr = parsed.ast.ok_or_else(|| {
            ExprError::at(ExprErrorKind::Syntax, at, "the expression does not parse")
        })?;
        let (nodes, depth) = size(&expr);
        if nodes > MAX_NODES || depth > MAX_DEPTH {
            return Err(ExprError::at(
                ExprErrorKind::Limit,
                at,
                format!(
                    "an expression has at most {MAX_NODES} nodes {MAX_DEPTH} deep (this one {nodes}, {depth} deep)"
                ),
            ));
        }
        let checked = env.check(&expr);
        if let Some(first) = checked.errors.first() {
            return Err(ExprError::at(
                ExprErrorKind::Type,
                at + first.span.start,
                first.message(),
            ));
        }
        let ast = Ast::new_checked(expr, source, checked);
        let ty = match ast.result_type() {
            None => Type::Dyn,
            Some(cel) => Type::of_cel(cel).ok_or_else(|| {
                ExprError::at(
                    ExprErrorKind::Type,
                    at,
                    format!("gives a {}, which is no value here", cel.display_name()),
                )
            })?,
        };
        let program = env
            .program(&ast)
            .map_err(|e| ExprError::at(ExprErrorKind::Type, at, e.to_string()))?;
        Ok(Self { program, at, ty })
    }

    /// Its value over `activation`, as text.
    ///
    /// # Errors
    /// `Budget` when it spent its steps; `Eval` when it failed; `Type`
    /// when it gave a value text cannot show.
    pub fn text(&self, activation: &dyn Activation) -> Result<String, ExprError> {
        let value = self.eval(activation)?;
        text(&value).ok_or_else(|| {
            ExprError::at(
                ExprErrorKind::Type,
                self.at,
                format!(
                    "gave a {}; text shows a string, a number, a bool, a time or a duration",
                    value.cel_type().display_name()
                ),
            )
        })
    }

    /// Its CEL value over `activation`, within the budget.
    fn eval(&self, activation: &dyn Activation) -> Result<Cel, ExprError> {
        let budget = Budget::new(STEPS);
        match self.program.eval_with_budget(activation, &budget) {
            Cel::Error(e) => Err(ExprError::at(
                if e.kind == EvalErrorKind::BudgetExceeded {
                    ExprErrorKind::Budget
                } else {
                    ExprErrorKind::Eval
                },
                self.at,
                e.message.clone(),
            )),
            value => Ok(value),
        }
    }
}

/// `compile` run on a thread whose stack holds CEL's parser. It recurses
/// once a level of nesting, and 4 KiB of an expression nest about 4,000
/// levels (prefix operators, ternaries): more than a worker thread's stack
/// holds (measured: about 20 MiB in a debug build).
///
/// # Errors
/// `compile`'s; `Limit` when the thread does not start.
pub fn on_compile_stack<T: Send>(
    compile: impl FnOnce() -> Result<T, ExprError> + Send,
) -> Result<T, ExprError> {
    std::thread::scope(|scope| {
        std::thread::Builder::new()
            .stack_size(COMPILE_STACK)
            .spawn_scoped(scope, compile)
            .map_err(|e| {
                ExprError::at(
                    ExprErrorKind::Limit,
                    0,
                    format!("the check could not start: {e}"),
                )
            })?
            .join()
            .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
    })
}

/// How many nodes `expr` has, and how deep it nests.
fn size(expr: &SpannedExpr) -> (usize, usize) {
    let mut nodes = 0;
    let mut deepest = 0;
    let mut open = vec![(expr, 1usize)];
    while let Some((at, depth)) = open.pop() {
        nodes += 1;
        deepest = deepest.max(depth);
        for child in children(at) {
            open.push((child, depth + 1));
        }
    }
    (nodes, deepest)
}

/// The expressions directly under `expr`.
fn children(expr: &SpannedExpr) -> Vec<&SpannedExpr> {
    match &expr.node {
        Expr::List(elements) => elements.iter().map(|e| &e.expr).collect(),
        Expr::Map(entries) => entries.iter().flat_map(|e| [&e.key, &e.value]).collect(),
        Expr::Unary { expr, .. }
        | Expr::Member { expr, .. }
        | Expr::MemberTestOnly { expr, .. } => vec![expr],
        Expr::Binary { left, right, .. } => vec![left, right],
        Expr::Ternary {
            cond,
            then_expr,
            else_expr,
        } => vec![cond, then_expr, else_expr],
        Expr::Index { expr, index, .. } => vec![expr, index],
        Expr::Call { expr, args } => std::iter::once(&**expr).chain(args).collect(),
        Expr::Struct { type_name, fields } => std::iter::once(&**type_name)
            .chain(fields.iter().map(|f| &f.value))
            .collect(),
        Expr::Comprehension(c) => vec![
            &c.iter_range,
            &c.accu_init,
            &c.loop_condition,
            &c.loop_step,
            &c.result,
        ],
        Expr::Bind { init, body, .. } => vec![init, body],
        Expr::Null
        | Expr::Bool(_)
        | Expr::Int(_)
        | Expr::UInt(_)
        | Expr::Float(_)
        | Expr::String(_)
        | Expr::Bytes(_)
        | Expr::Ident(_)
        | Expr::RootIdent(_)
        | Expr::Error => Vec::new(),
    }
}
