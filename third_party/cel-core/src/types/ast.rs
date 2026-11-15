//! CEL Abstract Syntax Tree definitions.
//!
//! This module contains the AST types that represent parsed CEL expressions.
//! These types are shared across the parser, checker, and other CEL components.

/// Source span for error reporting and LSP features.
/// Uses byte offsets into the source string.
pub type Span = std::ops::Range<usize>;

/// AST node with source location and unique ID.
#[derive(Debug, Clone, PartialEq)]
pub struct Spanned<T> {
    /// Unique identifier for this node (1-indexed, assigned during parsing)
    pub id: i64,
    pub node: T,
    pub span: Span,
}

impl<T> Spanned<T> {
    pub fn new(id: i64, node: T, span: Span) -> Self {
        Self { id, node, span }
    }
}

/// A spanned expression.
pub type SpannedExpr = Spanned<Expr>;

/// A list element that may be optional.
#[derive(Debug, Clone, PartialEq)]
pub struct ListElement {
    pub expr: SpannedExpr,
    pub optional: bool,
}

/// A map entry that may be optional.
#[derive(Debug, Clone, PartialEq)]
pub struct MapEntry {
    pub key: SpannedExpr,
    pub value: SpannedExpr,
    pub optional: bool,
}

/// A struct field that may be optional.
#[derive(Debug, Clone, PartialEq)]
pub struct StructField {
    pub name: String,
    pub value: SpannedExpr,
    pub optional: bool,
}

/// Data for a comprehension expression (result of macro expansion).
///
/// Represents the expansion of macros like `all`, `exists`, `exists_one`, `map`, `filter`.
///
/// Semantics:
/// ```text
/// let accu_var = accu_init
/// for (let iter_var, iter_var2 in iter_range) {
///    if (!loop_condition) { break }
///    accu_var = loop_step
/// }
/// return result
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct ComprehensionData {
    /// The name of the first iteration variable.
    pub iter_var: String,
    /// The name of the second iteration variable (for V2 macros), empty if not set.
    pub iter_var2: String,
    /// The range over which the comprehension iterates.
    pub iter_range: Box<SpannedExpr>,
    /// The name of the accumulator variable.
    pub accu_var: String,
    /// The initial value of the accumulator.
    pub accu_init: Box<SpannedExpr>,
    /// Returns false when the result has been computed (short-circuit condition).
    pub loop_condition: Box<SpannedExpr>,
    /// Computes the next value of the accumulator.
    pub loop_step: Box<SpannedExpr>,
    /// Computes the final result from the accumulator.
    pub result: Box<SpannedExpr>,
}

/// CEL expression.
#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    // Literals
    Null,
    Bool(bool),
    Int(i64),
    UInt(u64),
    Float(f64),
    String(String),
    Bytes(Vec<u8>),

    // Identifiers
    Ident(String),
    /// Root-scoped identifier (`.name`) - resolves in root scope only
    RootIdent(String),

    // Collections
    List(Vec<ListElement>),
    Map(Vec<MapEntry>),

    // Operations
    Unary {
        op: UnaryOp,
        expr: Box<SpannedExpr>,
    },
    Binary {
        op: BinaryOp,
        left: Box<SpannedExpr>,
        right: Box<SpannedExpr>,
    },
    Ternary {
        cond: Box<SpannedExpr>,
        then_expr: Box<SpannedExpr>,
        else_expr: Box<SpannedExpr>,
    },

    // Access
    Member {
        expr: Box<SpannedExpr>,
        field: String,
        /// Whether this is an optional select (x.?y instead of x.y)
        optional: bool,
    },
    Index {
        expr: Box<SpannedExpr>,
        index: Box<SpannedExpr>,
        /// Whether this is an optional index (x[?k] instead of x[k])
        optional: bool,
    },
    Call {
        expr: Box<SpannedExpr>,
        args: Vec<SpannedExpr>,
    },
    /// Struct/message literal: TypeName{field: value, ...}
    /// The type_name is the expression preceding the braces (Ident, RootIdent, or Member chain)
    Struct {
        type_name: Box<SpannedExpr>,
        fields: Vec<StructField>,
    },

    /// Comprehension expression (result of macro expansion).
    Comprehension(ComprehensionData),

    /// Member test expression (result of `has(m.x)` macro expansion).
    ///
    /// Tests for presence of a field without accessing its value.
    /// Corresponds to `Select` with `test_only=true` in the proto format.
    MemberTestOnly {
        expr: Box<SpannedExpr>,
        field: String,
    },

    /// Variable binding expression (result of `cel.bind(x, init, body)` macro).
    ///
    /// Binds a variable to a value for use within a scoped expression.
    /// The variable is only visible within the body expression.
    ///
    /// # Example
    /// `cel.bind(msg, "hello", msg + msg)` evaluates to `"hellohello"`
    Bind {
        /// The name of the variable to bind
        var_name: String,
        /// The initializer expression for the variable
        init: Box<SpannedExpr>,
        /// The body expression where the variable is in scope
        body: Box<SpannedExpr>,
    },

    /// Placeholder for parse errors (enables partial AST).
    /// Used during error recovery to represent unparseable sections.
    Error,
}

/// Unary operators.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOp {
    /// Arithmetic negation (`-`)
    Neg,
    /// Logical negation (`!`)
    Not,
}

/// Binary operators.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryOp {
    // Arithmetic
    Add,
    Sub,
    Mul,
    Div,
    Mod,

    // Comparison
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,

    // Membership
    In,

    // Logical
    And,
    Or,
}
