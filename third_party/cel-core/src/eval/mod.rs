//! CEL Evaluation Engine.
//!
//! This module provides the runtime evaluation infrastructure for CEL expressions.
//! It follows the cel-go architecture pattern where:
//!
//! - `Value` represents runtime values
//! - `Activation` provides variable bindings
//! - `Program` wraps a compiled expression with its function registry
//! - `Evaluator` performs tree-walking evaluation
//!
//! # Example
//!
//! ```
//! use cel_core::{Env, CelType};
//! use cel_core::{Value, MapActivation, Activation};
//!
//! let env = Env::with_standard_library()
//!     .with_variable("x", CelType::Int);
//!
//! let ast = env.compile("x + 1").unwrap();
//! let program = env.program(&ast).unwrap();
//!
//! let mut activation = MapActivation::new();
//! activation.insert("x", Value::Int(41));
//!
//! let result = program.eval(&activation);
//! assert_eq!(result, Value::Int(42));
//! ```

mod activation;
mod budget;
mod builtins;
mod conversions;
mod error;
mod evaluator;
mod functions;
pub(crate) mod message;
mod operators;
mod program;
pub(crate) mod proto_registry;
mod references;
mod resolve;
pub mod time;
mod value;

pub use activation::{
    Activation, EmptyActivation, HierarchicalActivation, MapActivation, SharedActivation,
};
pub use budget::Budget;
pub use error::{EvalError, EvalErrorKind};
pub(crate) use evaluator::Evaluator;
pub(crate) use references::References;
pub(crate) use functions::{Function, FunctionRegistry, Overload};
pub use message::MessageValue;
pub use program::Program;
pub use proto_registry::{ProtoRegistry, ProtoTypeResolver, StructFieldValue};
pub use value::{
    Duration, EnumValue, MapKey, OptionalValue, Timestamp, TypeValue, Value, ValueError, ValueMap,
};
