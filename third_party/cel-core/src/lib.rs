//! CEL-Core: High-level API for the Common Expression Language
//!
//! This crate provides a unified `Env` for working with CEL expressions,
//! following the cel-go architecture pattern.
//!
//! # Quick Start
//!
//! ```
//! use cel_core::Env;
//! use cel_core::CelType;
//!
//! // Create an environment with standard library and a variable
//! let env = Env::with_standard_library()
//!     .with_variable("x", CelType::Int);
//!
//! // Parse and type-check in one step
//! let ast = env.compile("x + 1").unwrap();
//! assert!(ast.is_checked());
//! ```
//!
//! # Architecture
//!
//! The `Env` struct coordinates:
//! - **Parser**: Converts source text into an AST
//! - **Checker**: Type-checks expressions and resolves references
//! - **Variables**: User-defined variable declarations
//! - **Functions**: Standard library + custom function declarations
//!
//! # Modules
//!
//! - `types`: Core type system, AST, and declarations
//! - `parser`: Lexer, parser, and macro expansion
//! - `checker`: Type checking and overload resolution
//! - `ext`: Extension libraries (strings, math, encoders, optionals)
//!
//! # Proto Conversion
//!
//! For proto wire format conversion (interop with cel-go, cel-cpp), use the
//! `cel-core-proto` crate which provides `CheckedExpr` and `ParsedExpr` types.

mod ast;
mod env;
mod unparser;

// Core modules
mod checker;
mod eval;
pub mod ext;
pub mod parser;
pub mod types;

pub use ast::{Ast, AstError};
pub use env::{AbbrevError, Abbreviations, CompileError, Env};
pub use unparser::ast_to_string;

// Re-export from checker module
pub use checker::{CheckError, CheckErrorKind, CheckResult, ReferenceInfo};

// Re-export from eval module
pub use eval::time;
pub use eval::{
    Activation, Budget, EmptyActivation, EvalError, EvalErrorKind, HierarchicalActivation,
    MapActivation, Program, Value, ValueError,
};
pub use eval::{
    Duration, EnumValue, MapKey, OptionalValue, SharedActivation, Timestamp, TypeValue, ValueMap,
};
pub use eval::{MessageValue, ProtoRegistry, ProtoTypeResolver, StructFieldValue};

// Re-export from parser module
pub use parser::{parse, ParseError, ParseResult};

// Re-export from types module
pub use types::proto::{proto_message_to_cel_type, ResolvedProtoType};
pub use types::{
    BinaryOp, CelType, CelValue, ComprehensionData, Expr, FunctionDecl, ListElement, MapEntry,
    OverloadDecl, Span, Spanned, SpannedExpr, StructField, UnaryOp, VariableDecl,
};
