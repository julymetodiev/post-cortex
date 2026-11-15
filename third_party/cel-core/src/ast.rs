//! Unified AST representation for CEL expressions.
//!
//! This module provides the `Ast` type which wraps a parsed (and optionally type-checked)
//! CEL expression. It follows the cel-go architecture pattern where a single `Ast` type
//! can represent both parsed and checked expressions.
//!
//! # Example
//!
//! ```
//! use cel_core::{Env, Ast};
//!
//! let env = Env::with_standard_library()
//!     .with_variable("x", cel_core::CelType::Int);
//!
//! // Compile returns a checked Ast
//! let ast = env.compile("x + 1").unwrap();
//! assert!(ast.is_checked());
//!
//! // Convert to CEL string
//! let cel_string = ast.to_cel_string();
//! ```
//!
//! # Proto Conversion
//!
//! For proto wire format conversion (interop with cel-go, cel-cpp), use the
//! `cel-core-proto` crate:
//!
//! ```ignore
//! use cel_core_proto::AstToProto;
//!
//! let parsed_expr = ast.to_parsed_expr();
//! let checked_expr = ast.to_checked_expr()?;
//! ```

use std::sync::Arc;

use crate::checker::CheckResult;
use crate::types::{CelType, SpannedExpr};

/// Error type for Ast operations.
#[derive(Debug, Clone)]
pub enum AstError {
    /// The AST has not been type-checked, but a checked operation was requested.
    NotChecked,
    /// Error during conversion or other operations.
    Other(String),
}

impl std::fmt::Display for AstError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AstError::NotChecked => write!(f, "AST has not been type-checked"),
            AstError::Other(msg) => write!(f, "{}", msg),
        }
    }
}

impl std::error::Error for AstError {}

/// Unified AST representation matching cel-go's Ast type.
///
/// An `Ast` can be either checked (has type info) or unchecked (parsed only).
/// This follows the cel-go pattern where a single type represents both states.
///
/// # Creating an Ast
///
/// - Use `Env::compile()` to get a checked `Ast`
/// - Use `Env::parse_only()` to get an unchecked `Ast`
///
/// # Proto Conversion
///
/// For proto wire format conversion, use the `cel-core-proto` crate directly.
#[derive(Debug, Clone)]
pub struct Ast {
    /// The expression tree.
    expr: SpannedExpr,
    /// The original source text.
    source: Arc<str>,
    /// Type checking results (None if unchecked).
    type_info: Option<CheckResult>,
}

impl Ast {
    /// Create an unchecked AST (after parsing, before type checking).
    pub fn new_unchecked(expr: SpannedExpr, source: impl Into<Arc<str>>) -> Self {
        Self {
            expr,
            source: source.into(),
            type_info: None,
        }
    }

    /// Create a checked AST (after type checking).
    pub fn new_checked(
        expr: SpannedExpr,
        source: impl Into<Arc<str>>,
        check_result: CheckResult,
    ) -> Self {
        Self {
            expr,
            source: source.into(),
            type_info: Some(check_result),
        }
    }

    /// Returns true if this AST has been type-checked.
    pub fn is_checked(&self) -> bool {
        self.type_info.is_some()
    }

    /// Get the expression tree.
    pub fn expr(&self) -> &SpannedExpr {
        &self.expr
    }

    /// Get the original source text.
    pub fn source(&self) -> &str {
        &self.source
    }

    /// Get type checking results (if checked).
    pub fn type_info(&self) -> Option<&CheckResult> {
        self.type_info.as_ref()
    }

    /// Get the result type of the expression (if checked).
    ///
    /// Returns the type of the root expression from the type map.
    pub fn result_type(&self) -> Option<&CelType> {
        self.type_info
            .as_ref()
            .and_then(|info| info.type_map.get(&self.expr.id))
    }

    // ==================== String Conversion ====================

    /// Convert the AST back to CEL source text.
    ///
    /// The output is a valid CEL expression that is semantically equivalent
    /// to the original. Formatting may differ (whitespace, parenthesization).
    ///
    /// # Example
    ///
    /// ```
    /// use cel_core::Env;
    ///
    /// let env = Env::with_standard_library();
    /// let ast = env.compile("1 + 2 * 3").unwrap();
    /// assert_eq!(ast.to_cel_string(), "1 + 2 * 3");
    /// ```
    pub fn to_cel_string(&self) -> String {
        crate::unparser::ast_to_string(&self.expr)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Env;

    #[test]
    fn test_ast_is_checked() {
        let env = Env::with_standard_library().with_variable("x", CelType::Int);

        let ast = env.compile("x + 1").unwrap();
        assert!(ast.is_checked());

        let unchecked = env.parse_only("x + 1").unwrap();
        assert!(!unchecked.is_checked());
    }

    #[test]
    fn test_ast_result_type() {
        let env = Env::with_standard_library().with_variable("x", CelType::Int);

        let ast = env.compile("x + 1").unwrap();
        assert_eq!(ast.result_type(), Some(&CelType::Int));

        let unchecked = env.parse_only("x + 1").unwrap();
        assert!(unchecked.result_type().is_none());
    }

    #[test]
    fn test_ast_to_cel_string() {
        let env = Env::with_standard_library().with_variable("x", CelType::Int);

        let ast = env.compile("x + 1").unwrap();
        assert_eq!(ast.to_cel_string(), "x + 1");

        // Works for unchecked too
        let unchecked = env.parse_only("x * 2 + 3").unwrap();
        assert_eq!(unchecked.to_cel_string(), "x * 2 + 3");
    }

    #[test]
    fn test_ast_to_cel_string_complex() {
        let env = Env::with_standard_library();

        let ast = env.compile("1 + 2 * 3").unwrap();
        assert_eq!(ast.to_cel_string(), "1 + 2 * 3");

        let ast = env.compile("(1 + 2) * 3").unwrap();
        assert_eq!(ast.to_cel_string(), "(1 + 2) * 3");
    }
}
