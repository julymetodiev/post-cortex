//! Evaluation error types.

use std::fmt;

/// An error that occurred during CEL evaluation.
#[derive(Debug, Clone)]
pub struct EvalError {
    /// The error message.
    pub message: String,
    /// The kind of error.
    pub kind: EvalErrorKind,
}

/// The kind of evaluation error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvalErrorKind {
    /// Division by zero.
    DivisionByZero,
    /// Modulo by zero.
    ModuloByZero,
    /// Integer overflow.
    Overflow,
    /// Value out of valid range.
    RangeError,
    /// Type mismatch at runtime.
    TypeMismatch,
    /// Unknown identifier (variable not found).
    UnknownIdentifier,
    /// Unknown function.
    UnknownFunction,
    /// Index out of bounds.
    IndexOutOfBounds,
    /// Key not found in map.
    KeyNotFound,
    /// Invalid argument.
    InvalidArgument,
    /// No matching overload found.
    NoMatchingOverload,
    /// Field not found on struct/message.
    FieldNotFound,
    /// Invalid conversion.
    InvalidConversion,
    /// Internal error (unexpected state).
    Internal,
    /// The evaluation's budget of steps is spent (pcx patch).
    BudgetExceeded,
}

impl EvalError {
    /// Create a new error with the given kind and message.
    pub fn new(kind: EvalErrorKind, message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            kind,
        }
    }

    /// The evaluation's budget of steps is spent (pcx patch).
    pub fn budget_exceeded() -> Self {
        Self::new(
            EvalErrorKind::BudgetExceeded,
            "the evaluation's budget of steps is spent",
        )
    }

    /// Create a division by zero error.
    pub fn division_by_zero() -> Self {
        Self::new(EvalErrorKind::DivisionByZero, "division by zero")
    }

    /// Create a modulo by zero error.
    pub fn modulo_by_zero() -> Self {
        Self::new(EvalErrorKind::ModuloByZero, "modulo by zero")
    }

    /// Create an overflow error.
    pub fn overflow(message: impl Into<String>) -> Self {
        Self::new(EvalErrorKind::Overflow, message)
    }

    /// Create a range error (value out of valid range).
    pub fn range_error(message: impl Into<String>) -> Self {
        Self::new(EvalErrorKind::RangeError, message)
    }

    /// Create a type mismatch error.
    pub fn type_mismatch(expected: &str, actual: &str) -> Self {
        Self::new(
            EvalErrorKind::TypeMismatch,
            format!("expected {}, got {}", expected, actual),
        )
    }

    /// Create an unknown identifier error.
    pub fn unknown_identifier(name: &str) -> Self {
        Self::new(
            EvalErrorKind::UnknownIdentifier,
            format!("unknown identifier: {}", name),
        )
    }

    /// Create an unknown function error.
    pub fn unknown_function(name: &str) -> Self {
        Self::new(
            EvalErrorKind::UnknownFunction,
            format!("unknown function: {}", name),
        )
    }

    /// Create an index out of bounds error.
    pub fn index_out_of_bounds(index: i64, len: usize) -> Self {
        Self::new(
            EvalErrorKind::IndexOutOfBounds,
            format!("index {} out of bounds for length {}", index, len),
        )
    }

    /// Create a key not found error.
    pub fn key_not_found(key: &str) -> Self {
        Self::new(
            EvalErrorKind::KeyNotFound,
            format!("key not found: {}", key),
        )
    }

    /// Create an invalid argument error.
    pub fn invalid_argument(message: impl Into<String>) -> Self {
        Self::new(EvalErrorKind::InvalidArgument, message)
    }

    /// Create a no matching overload error.
    pub fn no_matching_overload(func: &str) -> Self {
        Self::new(
            EvalErrorKind::NoMatchingOverload,
            format!("no matching overload for function: {}", func),
        )
    }

    /// Create a field not found error.
    pub fn field_not_found(field: &str) -> Self {
        Self::new(
            EvalErrorKind::FieldNotFound,
            format!("field not found: {}", field),
        )
    }

    /// Create an invalid conversion error.
    pub fn invalid_conversion(from: &str, to: &str) -> Self {
        Self::new(
            EvalErrorKind::InvalidConversion,
            format!("cannot convert {} to {}", from, to),
        )
    }

    /// Create an internal error.
    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(EvalErrorKind::Internal, message)
    }
}

impl fmt::Display for EvalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for EvalError {}

impl From<&str> for EvalError {
    fn from(s: &str) -> Self {
        Self::new(EvalErrorKind::Internal, s)
    }
}

impl From<String> for EvalError {
    fn from(s: String) -> Self {
        Self::new(EvalErrorKind::Internal, s)
    }
}
