//! CEL (Common Expression Language) parser.

mod lexer;
pub mod macros;
mod parse;

use crate::types::{Span, SpannedExpr};

pub use macros::{
    ArgCount, Macro, MacroContext, MacroExpander, MacroExpansion, MacroRegistry, MacroStyle,
};
pub use parse::MacroCalls;

/// A parse error with source location.
#[derive(Debug, Clone)]
pub struct ParseError {
    pub message: String,
    pub span: Span,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} at {}..{}",
            self.message, self.span.start, self.span.end
        )
    }
}

impl std::error::Error for ParseError {}

/// Options for parsing CEL expressions.
#[derive(Debug, Clone, Default)]
pub struct ParseOptions {
    /// Custom macro registry. If None, uses standard CEL macros.
    pub macros: Option<MacroRegistry>,
}

impl ParseOptions {
    /// Create parse options with default settings (standard macros).
    pub fn new() -> Self {
        Self::default()
    }

    /// Create parse options with a custom macro registry.
    pub fn with_macros(macros: MacroRegistry) -> Self {
        Self {
            macros: Some(macros),
        }
    }

    /// Create parse options with no macros (all macro calls become regular calls).
    pub fn without_macros() -> Self {
        Self {
            macros: Some(MacroRegistry::new()),
        }
    }
}

/// Result of parsing a CEL expression.
///
/// Supports error recovery: may return both an AST and errors.
/// The AST may contain `Expr::Error` nodes where parsing failed.
/// Includes macro_calls map for IDE features.
#[derive(Debug, Clone)]
pub struct ParseResult {
    /// The parsed AST, if any parsing succeeded.
    /// May contain `Expr::Error` nodes for unparseable sections.
    /// Macros (has, all, exists, exists_one, map, filter) are expanded inline.
    pub ast: Option<SpannedExpr>,
    /// Map of comprehension/expansion IDs to original macro call expressions.
    /// Used for IDE features like hover to show the original source form.
    pub macro_calls: MacroCalls,
    /// Any parse errors encountered.
    pub errors: Vec<ParseError>,
}

impl ParseResult {
    /// Returns true if parsing completed without errors.
    pub fn is_ok(&self) -> bool {
        self.errors.is_empty() && self.ast.is_some()
    }

    /// Returns true if there are any parse errors.
    pub fn is_err(&self) -> bool {
        !self.errors.is_empty()
    }

    /// Converts to a Result, discarding partial AST on error.
    pub fn into_result(self) -> Result<SpannedExpr, Vec<ParseError>> {
        if self.errors.is_empty() {
            self.ast.ok_or_else(Vec::new)
        } else {
            Err(self.errors)
        }
    }

    /// Unwraps the errors, panicking if there are none.
    pub fn unwrap_err(self) -> Vec<ParseError> {
        if self.errors.is_empty() {
            panic!("called unwrap_err on a ParseResult with no errors");
        }
        self.errors
    }
}

/// Parse a CEL expression from source.
///
/// Returns a `ParseResult` which may contain both a partial AST and errors
/// when error recovery is successful. Macros (has, all, exists, exists_one,
/// map, filter) are expanded inline during parsing.
///
/// The `macro_calls` field in the result maps comprehension/expansion IDs
/// to the original macro call expressions, which is useful for IDE features.
pub fn parse(input: &str) -> ParseResult {
    parse_with_options(input, ParseOptions::new())
}

/// Parse a CEL expression with custom options.
///
/// This allows configuring the macro registry used during parsing.
/// Use `ParseOptions::without_macros()` to disable macro expansion,
/// or `ParseOptions::with_macros(registry)` for custom macros.
pub fn parse_with_options(input: &str, options: ParseOptions) -> ParseResult {
    // First, lex the input
    let tokens = match lexer::lex(input) {
        Ok(tokens) => tokens,
        Err(e) => {
            return ParseResult {
                ast: None,
                macro_calls: MacroCalls::new(),
                errors: vec![ParseError {
                    message: e.message,
                    span: e.span,
                }],
            };
        }
    };

    // Get macro registry
    let macros = options.macros.unwrap_or_else(MacroRegistry::standard);

    // Parse the tokens (with inline macro expansion)
    let (ast, parse_errors, macro_calls) = parse::parse_tokens_with_macros(&tokens, macros);

    let errors: Vec<ParseError> = parse_errors
        .into_iter()
        .map(|e| ParseError {
            message: e.message,
            span: e.span,
        })
        .collect();

    ParseResult {
        ast,
        macro_calls,
        errors,
    }
}
