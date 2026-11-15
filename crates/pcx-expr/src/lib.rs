// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Templates of text and `{{ <CEL> }}` parts (docs/modules/workflow.md,
//! templates and expressions; docs/modules/mail.md, templates), pure: a
//! template is checked against the inputs it declares when it is saved,
//! and filled with values when it is used, each expression within the
//! limits (4 KiB, an AST of at most 1,000 nodes and 32 deep, a budget of
//! steps). Expressions alone (a workflow's conditions) are checked and
//! evaluated the same way, against variables that may be proto messages of
//! a descriptor pool. The fields of a schema, and JSON read by them. The one crate that reaches `cel-core`.

#![forbid(unsafe_code)]

mod bound;
pub mod constraints;
mod error;
mod expression;
mod functions;
mod inputs;
mod scan;
pub mod schema;
mod template;
mod types;
mod value;

pub use bound::{Bound, Items, Local, Meter};
pub use error::{ExprError, ExprErrorKind};
pub use expression::Expression;
pub use inputs::Inputs;
pub use template::{Escape, Template};
pub use types::Type;
pub use value::{Entries, List, Value, Values};
