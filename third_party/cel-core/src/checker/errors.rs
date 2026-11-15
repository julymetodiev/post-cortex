//! Error types for type checking.
//!
//! This module provides structured error types for reporting type check failures.

use crate::types::{CelType, Span};
use std::fmt;

/// A type checking error.
#[derive(Debug, Clone)]
pub struct CheckError {
    /// The kind of error.
    pub kind: CheckErrorKind,
    /// The source span where the error occurred.
    pub span: Span,
    /// The expression ID where the error occurred.
    pub expr_id: i64,
}

impl CheckError {
    /// Create a new check error.
    pub fn new(kind: CheckErrorKind, span: Span, expr_id: i64) -> Self {
        Self {
            kind,
            span,
            expr_id,
        }
    }

    /// Create an undeclared reference error.
    pub fn undeclared_reference(name: &str, span: Span, expr_id: i64) -> Self {
        Self::new(
            CheckErrorKind::UndeclaredReference {
                container: String::new(),
                name: name.to_string(),
            },
            span,
            expr_id,
        )
    }

    /// Create an undeclared reference error with container.
    pub fn undeclared_reference_in(container: &str, name: &str, span: Span, expr_id: i64) -> Self {
        Self::new(
            CheckErrorKind::UndeclaredReference {
                container: container.to_string(),
                name: name.to_string(),
            },
            span,
            expr_id,
        )
    }

    /// Create a no matching overload error.
    pub fn no_matching_overload(
        function: &str,
        arg_types: Vec<CelType>,
        span: Span,
        expr_id: i64,
    ) -> Self {
        Self::new(
            CheckErrorKind::NoMatchingOverload {
                function: function.to_string(),
                arg_types,
            },
            span,
            expr_id,
        )
    }

    /// Create a type mismatch error.
    pub fn type_mismatch(expected: CelType, actual: CelType, span: Span, expr_id: i64) -> Self {
        Self::new(
            CheckErrorKind::TypeMismatch { expected, actual },
            span,
            expr_id,
        )
    }

    /// Create an undefined field error.
    pub fn undefined_field(type_name: &str, field: &str, span: Span, expr_id: i64) -> Self {
        Self::new(
            CheckErrorKind::UndefinedField {
                type_name: type_name.to_string(),
                field: field.to_string(),
            },
            span,
            expr_id,
        )
    }

    /// Create an error with a custom message.
    pub fn other(msg: impl Into<String>, span: Span, expr_id: i64) -> Self {
        Self::new(CheckErrorKind::Other(msg.into()), span, expr_id)
    }

    /// Get the error message.
    pub fn message(&self) -> String {
        self.kind.to_string()
    }
}

impl fmt::Display for CheckError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.kind)
    }
}

impl std::error::Error for CheckError {}

/// The kind of type checking error.
#[derive(Debug, Clone)]
pub enum CheckErrorKind {
    /// Reference to an undeclared variable or function.
    UndeclaredReference {
        /// Container namespace (empty if none).
        container: String,
        /// The name that was not found.
        name: String,
    },

    /// No function overload matches the provided arguments.
    NoMatchingOverload {
        /// The function name.
        function: String,
        /// The types of arguments provided.
        arg_types: Vec<CelType>,
    },

    /// Type mismatch between expected and actual types.
    TypeMismatch {
        /// The expected type.
        expected: CelType,
        /// The actual type found.
        actual: CelType,
    },

    /// Field not found on a type.
    UndefinedField {
        /// The type name.
        type_name: String,
        /// The field that was not found.
        field: String,
    },

    /// Type is not assignable to another type.
    NotAssignable {
        /// The source type.
        from: CelType,
        /// The target type.
        to: CelType,
    },

    /// Aggregate literal contains heterogeneous types.
    HeterogeneousAggregate {
        /// The types found in the aggregate.
        types: Vec<CelType>,
    },

    /// Expression cannot be used as a type.
    NotAType {
        /// The expression text (for error messages).
        expr: String,
    },

    /// General type error with custom message.
    Other(String),
}

impl fmt::Display for CheckErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CheckErrorKind::UndeclaredReference { container, name } => {
                if container.is_empty() {
                    write!(f, "undeclared reference to '{}'", name)
                } else {
                    write!(
                        f,
                        "undeclared reference to '{}' in container '{}'",
                        name, container
                    )
                }
            }
            CheckErrorKind::NoMatchingOverload {
                function,
                arg_types,
            } => {
                let types: Vec<_> = arg_types.iter().map(|t| t.display_name()).collect();
                write!(
                    f,
                    "no matching overload for '{}' with argument types ({})",
                    function,
                    types.join(", ")
                )
            }
            CheckErrorKind::TypeMismatch { expected, actual } => {
                write!(
                    f,
                    "expected type '{}' but found '{}'",
                    expected.display_name(),
                    actual.display_name()
                )
            }
            CheckErrorKind::UndefinedField { type_name, field } => {
                write!(f, "undefined field '{}' on type '{}'", field, type_name)
            }
            CheckErrorKind::NotAssignable { from, to } => {
                write!(
                    f,
                    "type '{}' is not assignable to '{}'",
                    from.display_name(),
                    to.display_name()
                )
            }
            CheckErrorKind::HeterogeneousAggregate { types } => {
                let type_names: Vec<_> = types.iter().map(|t| t.display_name()).collect();
                write!(
                    f,
                    "aggregate literal contains heterogeneous types: {}",
                    type_names.join(", ")
                )
            }
            CheckErrorKind::NotAType { expr } => {
                write!(f, "expression '{}' is not a type", expr)
            }
            CheckErrorKind::Other(msg) => write!(f, "{}", msg),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_undeclared_reference() {
        let err = CheckError::undeclared_reference("foo", 0..3, 1);
        assert!(err.message().contains("undeclared reference"));
        assert!(err.message().contains("foo"));
    }

    #[test]
    fn test_no_matching_overload() {
        let err =
            CheckError::no_matching_overload("_+_", vec![CelType::String, CelType::Int], 0..5, 1);
        assert!(err.message().contains("no matching overload"));
        assert!(err.message().contains("_+_"));
    }

    #[test]
    fn test_type_mismatch() {
        let err = CheckError::type_mismatch(CelType::Bool, CelType::Int, 0..1, 1);
        assert!(err.message().contains("expected"));
        assert!(err.message().contains("bool"));
        assert!(err.message().contains("int"));
    }

    #[test]
    fn test_undefined_field() {
        let err = CheckError::undefined_field("MyMessage", "unknown", 0..7, 1);
        assert!(err.message().contains("undefined field"));
        assert!(err.message().contains("unknown"));
        assert!(err.message().contains("MyMessage"));
    }
}
