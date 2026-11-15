//! CEL parser - hand-written recursive descent with inline macro expansion.

use std::collections::HashMap;

use super::lexer::{SpannedToken, Token};
use super::macros::{MacroContext, MacroExpansion, MacroRegistry};
use crate::types::{Expr, ListElement, MapEntry, Span, Spanned, SpannedExpr, StructField};

/// Parse error with span information.
#[derive(Debug, Clone, PartialEq)]
pub struct ParseError {
    pub message: String,
    pub span: Span,
}

/// Map of expression IDs to the original macro call expressions.
/// Used to preserve source information for IDE features.
pub type MacroCalls = HashMap<i64, SpannedExpr>;

/// Recursive descent parser for CEL expressions.
pub struct Parser<'a> {
    tokens: &'a [SpannedToken],
    pos: usize,
    /// Counter for generating unique node IDs (starts at 1)
    next_id: i64,
    /// Map of comprehension/expansion IDs to original macro call expressions.
    macro_calls: MacroCalls,
    /// Accumulated macro expansion errors (non-fatal).
    macro_errors: Vec<ParseError>,
    /// Registry of macros for expansion.
    macros: MacroRegistry,
}

impl<'a> Parser<'a> {
    /// Create a new parser for the given token stream with standard macros.
    pub fn new(tokens: &'a [SpannedToken]) -> Self {
        Self::with_macros(tokens, MacroRegistry::standard())
    }

    /// Create a new parser with a custom macro registry.
    pub fn with_macros(tokens: &'a [SpannedToken], macros: MacroRegistry) -> Self {
        Self {
            tokens,
            pos: 0,
            next_id: 1,
            macro_calls: HashMap::new(),
            macro_errors: Vec::new(),
            macros,
        }
    }

    /// Take the macro_calls map, replacing it with an empty map.
    pub fn take_macro_calls(&mut self) -> MacroCalls {
        std::mem::take(&mut self.macro_calls)
    }

    /// Take accumulated macro errors.
    pub fn take_macro_errors(&mut self) -> Vec<ParseError> {
        std::mem::take(&mut self.macro_errors)
    }

    /// Add a macro error (non-fatal).
    fn add_macro_error(&mut self, message: String, span: Span) {
        self.macro_errors.push(ParseError { message, span });
    }

    /// Allocate the next unique node ID.
    fn next_id(&mut self) -> i64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    // === Utility Methods ===

    /// Peek at the current token without consuming it.
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos).map(|(t, _)| t)
    }

    /// Peek at the current token with its span.
    fn peek_with_span(&self) -> Option<(&Token, &Span)> {
        self.tokens.get(self.pos).map(|(t, s)| (t, s))
    }

    /// Get the span of the current token.
    fn peek_span(&self) -> Span {
        self.tokens
            .get(self.pos)
            .map(|(_, s)| s.clone())
            .unwrap_or_else(|| self.eof_span())
    }

    /// Get the span representing end-of-input.
    fn eof_span(&self) -> Span {
        let end = self.tokens.last().map(|(_, s)| s.end).unwrap_or(0);
        end..end
    }

    /// Advance to the next token, returning the current one.
    fn advance(&mut self) -> Option<&SpannedToken> {
        let token = self.tokens.get(self.pos);
        if token.is_some() {
            self.pos += 1;
        }
        token
    }

    /// Check if the current token matches the given token.
    fn check(&self, token: &Token) -> bool {
        self.peek() == Some(token)
    }

    /// Consume the current token if it matches, returning true if consumed.
    fn match_token(&mut self, token: &Token) -> bool {
        if self.check(token) {
            self.advance();
            true
        } else {
            false
        }
    }

    /// Expect a specific token, returning an error if not found.
    fn expect(&mut self, token: &Token) -> Result<Span, ParseError> {
        if self.check(token) {
            let span = self.peek_span();
            self.advance();
            Ok(span)
        } else {
            Err(ParseError {
                message: format!("expected '{}', found {:?}", token, self.peek()),
                span: self.peek_span(),
            })
        }
    }

    /// Check if we've reached the end of the token stream.
    pub fn at_end(&self) -> bool {
        self.pos >= self.tokens.len()
    }

    // === Expression Parsing ===

    /// Parse an expression (entry point).
    pub fn parse_expr(&mut self) -> Result<SpannedExpr, ParseError> {
        self.parse_ternary()
    }

    /// Parse ternary conditional: expr ? expr : expr
    fn parse_ternary(&mut self) -> Result<SpannedExpr, ParseError> {
        let cond = self.parse_or()?;

        if self.match_token(&Token::Question) {
            let then_expr = self.parse_expr()?;
            self.expect(&Token::Colon)?;
            let else_expr = self.parse_expr()?;
            let span = cond.span.start..else_expr.span.end;

            Ok(Spanned::new(
                self.next_id(),
                Expr::Ternary {
                    cond: Box::new(cond),
                    then_expr: Box::new(then_expr),
                    else_expr: Box::new(else_expr),
                },
                span,
            ))
        } else {
            Ok(cond)
        }
    }

    /// Parse logical OR: expr || expr
    fn parse_or(&mut self) -> Result<SpannedExpr, ParseError> {
        use crate::types::BinaryOp;

        let mut left = self.parse_and()?;

        while self.match_token(&Token::Or) {
            let right = self.parse_and()?;
            let span = left.span.start..right.span.end;
            left = Spanned::new(
                self.next_id(),
                Expr::Binary {
                    op: BinaryOp::Or,
                    left: Box::new(left),
                    right: Box::new(right),
                },
                span,
            );
        }

        Ok(left)
    }

    /// Parse logical AND: expr && expr
    fn parse_and(&mut self) -> Result<SpannedExpr, ParseError> {
        use crate::types::BinaryOp;

        let mut left = self.parse_relation()?;

        while self.match_token(&Token::And) {
            let right = self.parse_relation()?;
            let span = left.span.start..right.span.end;
            left = Spanned::new(
                self.next_id(),
                Expr::Binary {
                    op: BinaryOp::And,
                    left: Box::new(left),
                    right: Box::new(right),
                },
                span,
            );
        }

        Ok(left)
    }

    /// Parse relational operators: == != < <= > >= in
    fn parse_relation(&mut self) -> Result<SpannedExpr, ParseError> {
        let mut left = self.parse_addition()?;

        while let Some(op) = self.peek_relop() {
            self.advance();
            let right = self.parse_addition()?;
            let span = left.span.start..right.span.end;
            left = Spanned::new(
                self.next_id(),
                Expr::Binary {
                    op,
                    left: Box::new(left),
                    right: Box::new(right),
                },
                span,
            );
        }

        Ok(left)
    }

    /// Check if the current token is a relational operator.
    fn peek_relop(&self) -> Option<crate::types::BinaryOp> {
        use crate::types::BinaryOp;

        match self.peek()? {
            Token::EqEq => Some(BinaryOp::Eq),
            Token::Ne => Some(BinaryOp::Ne),
            Token::Lt => Some(BinaryOp::Lt),
            Token::Le => Some(BinaryOp::Le),
            Token::Gt => Some(BinaryOp::Gt),
            Token::Ge => Some(BinaryOp::Ge),
            Token::In => Some(BinaryOp::In),
            _ => None,
        }
    }

    /// Parse additive operators: + -
    fn parse_addition(&mut self) -> Result<SpannedExpr, ParseError> {
        use crate::types::BinaryOp;

        let mut left = self.parse_mult()?;

        loop {
            let op = if self.match_token(&Token::Plus) {
                BinaryOp::Add
            } else if self.match_token(&Token::Minus) {
                BinaryOp::Sub
            } else {
                break;
            };

            let right = self.parse_mult()?;
            let span = left.span.start..right.span.end;
            left = Spanned::new(
                self.next_id(),
                Expr::Binary {
                    op,
                    left: Box::new(left),
                    right: Box::new(right),
                },
                span,
            );
        }

        Ok(left)
    }

    /// Parse multiplicative operators: * / %
    fn parse_mult(&mut self) -> Result<SpannedExpr, ParseError> {
        use crate::types::BinaryOp;

        let mut left = self.parse_unary()?;

        loop {
            let op = if self.match_token(&Token::Star) {
                BinaryOp::Mul
            } else if self.match_token(&Token::Slash) {
                BinaryOp::Div
            } else if self.match_token(&Token::Percent) {
                BinaryOp::Mod
            } else {
                break;
            };

            let right = self.parse_unary()?;
            let span = left.span.start..right.span.end;
            left = Spanned::new(
                self.next_id(),
                Expr::Binary {
                    op,
                    left: Box::new(left),
                    right: Box::new(right),
                },
                span,
            );
        }

        Ok(left)
    }

    /// Parse unary operators: - !
    fn parse_unary(&mut self) -> Result<SpannedExpr, ParseError> {
        use crate::types::UnaryOp;

        let start = self.peek_span().start;

        if self.match_token(&Token::Minus) {
            // Special case: -9223372036854775808 is i64::MIN
            // The literal 9223372036854775808 overflows i64, but -9223372036854775808 is valid
            if let Some((Token::IntOverflow(s), span)) = self.peek_with_span() {
                if s == "9223372036854775808" {
                    let end = span.end;
                    self.advance();
                    return Ok(Spanned::new(
                        self.next_id(),
                        Expr::Int(i64::MIN),
                        start..end,
                    ));
                }
            }
            let expr = self.parse_unary()?;
            let span = start..expr.span.end;
            Ok(Spanned::new(
                self.next_id(),
                Expr::Unary {
                    op: UnaryOp::Neg,
                    expr: Box::new(expr),
                },
                span,
            ))
        } else if self.match_token(&Token::Not) {
            let expr = self.parse_unary()?;
            let span = start..expr.span.end;
            Ok(Spanned::new(
                self.next_id(),
                Expr::Unary {
                    op: UnaryOp::Not,
                    expr: Box::new(expr),
                },
                span,
            ))
        } else {
            self.parse_postfix()
        }
    }

    /// Parse postfix operators: . [] () {}
    fn parse_postfix(&mut self) -> Result<SpannedExpr, ParseError> {
        let mut expr = self.parse_atom()?;

        loop {
            if self.check(&Token::LParen) {
                // Function call
                expr = self.parse_call(expr)?;
            } else if self.check(&Token::LBracket) {
                // Index
                expr = self.parse_index(expr)?;
            } else if self.check(&Token::Dot) {
                // Member access
                expr = self.parse_member(expr)?;
            } else if self.check(&Token::LBrace) && self.is_type_expr(&expr) {
                // Struct literal (only if expr is an ident or member chain)
                expr = self.parse_struct_init(expr)?;
            } else {
                break;
            }
        }

        Ok(expr)
    }

    /// Check if the expression can be used as a type name for struct literals.
    fn is_type_expr(&self, expr: &SpannedExpr) -> bool {
        matches!(
            expr.node,
            Expr::Ident(_) | Expr::RootIdent(_) | Expr::Member { .. }
        )
    }

    /// Parse a function call: expr(args...)
    /// Also handles inline macro expansion using the macro registry.
    fn parse_call(&mut self, callee: SpannedExpr) -> Result<SpannedExpr, ParseError> {
        let start = callee.span.start;
        self.expect(&Token::LParen)?;

        let mut args = Vec::new();
        if !self.check(&Token::RParen) {
            args.push(self.parse_expr()?);
            while self.match_token(&Token::Comma) {
                if self.check(&Token::RParen) {
                    break; // trailing comma
                }
                args.push(self.parse_expr()?);
            }
        }

        let end_span = self.expect(&Token::RParen)?;
        let span = start..end_span.end;

        // Try macro expansion using the registry
        if let Some(expanded) = self.try_macro_expansion(&callee, span.clone(), args.clone()) {
            return Ok(expanded);
        }

        // Not a macro - return regular Call node
        Ok(Spanned::new(
            self.next_id(),
            Expr::Call {
                expr: Box::new(callee),
                args,
            },
            span,
        ))
    }

    /// Try to expand a call as a macro using the registry.
    /// Returns None if no matching macro is found or if the macro doesn't apply.
    /// Returns Some(Expr::Error) if the macro name matches but expansion fails
    /// (wrong arity or expansion error), preventing misleading checker errors.
    fn try_macro_expansion(
        &mut self,
        callee: &SpannedExpr,
        span: Span,
        args: Vec<SpannedExpr>,
    ) -> Option<SpannedExpr> {
        // Extract call info: (name, receiver, is_receiver)
        let (name, receiver, is_receiver) = self.extract_call_info(callee)?;

        // Look up macro in registry with actual arg count
        let macro_def = match self.macros.lookup(&name, args.len(), is_receiver) {
            Some(m) => m,
            None => {
                // Name matched a macro but arg count didn't — generate arity error
                let desc = self
                    .macros
                    .expected_args_description(&name, is_receiver)
                    .unwrap_or_else(|| "a different number of arguments".to_string());
                self.add_macro_error(
                    format!("{}() requires {}, got {}", name, desc, args.len()),
                    span.clone(),
                );
                let id = self.next_id;
                self.next_id += 1;
                return Some(Spanned::new(id, Expr::Error, span));
            }
        };

        // Create macro context
        let mut next_id_fn = || {
            let id = self.next_id;
            self.next_id += 1;
            id
        };

        // Store macro call closure
        let macro_calls = &mut self.macro_calls;
        let mut store_fn = |call_id: i64,
                            span: &Span,
                            receiver: &SpannedExpr,
                            method_name: &str,
                            args: &[SpannedExpr]| {
            // Build the original call expression: receiver.method(args...)
            let method_expr = Spanned::new(
                0, // ID doesn't matter for stored calls
                Expr::Member {
                    expr: Box::new(receiver.clone()),
                    field: method_name.to_string(),
                    optional: false,
                },
                span.clone(),
            );

            let original_call = Spanned::new(
                call_id,
                Expr::Call {
                    expr: Box::new(method_expr),
                    args: args.to_vec(),
                },
                span.clone(),
            );

            macro_calls.insert(call_id, original_call);
        };

        let mut ctx = MacroContext::new(&mut next_id_fn, Some(&mut store_fn));

        // Call the expander
        let result = (macro_def.expander)(&mut ctx, span.clone(), receiver, args.clone());

        // Handle expansion errors
        for (message, error_span) in ctx.take_errors() {
            self.add_macro_error(message, error_span);
        }

        match result {
            MacroExpansion::Expanded(expr) => Some(expr),
            MacroExpansion::Error(msg) => {
                self.add_macro_error(msg, span.clone());
                let id = self.next_id;
                self.next_id += 1;
                Some(Spanned::new(id, Expr::Error, span))
            }
        }
    }

    /// Extract call information from a callee expression.
    /// Returns (name, receiver, is_receiver) or None if not a macro candidate.
    fn extract_call_info(
        &self,
        callee: &SpannedExpr,
    ) -> Option<(String, Option<SpannedExpr>, bool)> {
        match &callee.node {
            // Global function call: name(args)
            Expr::Ident(name) => {
                // Check if this is a global macro
                if self.macros.lookup(name, 0, false).is_some()
                    || self.macros.lookup(name, 1, false).is_some()
                    || self.macros.lookup(name, 2, false).is_some()
                {
                    Some((name.clone(), None, false))
                } else {
                    None
                }
            }
            // Method call: receiver.name(args)
            Expr::Member { expr, field, .. } => {
                // First check if receiver.field is a global macro (like cel.bind)
                if let Expr::Ident(receiver_name) = &expr.node {
                    let full_name = format!("{}.{}", receiver_name, field);
                    if self.macros.lookup(&full_name, 0, false).is_some()
                        || self.macros.lookup(&full_name, 1, false).is_some()
                        || self.macros.lookup(&full_name, 2, false).is_some()
                        || self.macros.lookup(&full_name, 3, false).is_some()
                    {
                        return Some((full_name, None, false));
                    }
                }
                // Check if this is a receiver macro
                if self.macros.lookup(field, 0, true).is_some()
                    || self.macros.lookup(field, 1, true).is_some()
                    || self.macros.lookup(field, 2, true).is_some()
                    || self.macros.lookup(field, 3, true).is_some()
                    || self.macros.lookup(field, 4, true).is_some()
                {
                    Some((field.clone(), Some((**expr).clone()), true))
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    /// Parse an index operation: expr[index] or expr[?index]
    fn parse_index(&mut self, base: SpannedExpr) -> Result<SpannedExpr, ParseError> {
        let start = base.span.start;
        self.expect(&Token::LBracket)?;
        let optional = self.match_token(&Token::Question);
        let index = self.parse_expr()?;
        let end_span = self.expect(&Token::RBracket)?;

        Ok(Spanned::new(
            self.next_id(),
            Expr::Index {
                expr: Box::new(base),
                index: Box::new(index),
                optional,
            },
            start..end_span.end,
        ))
    }

    /// Parse member access: expr.field or expr.?field
    fn parse_member(&mut self, base: SpannedExpr) -> Result<SpannedExpr, ParseError> {
        let start = base.span.start;
        self.expect(&Token::Dot)?;
        let optional = self.match_token(&Token::Question);

        let (field, end) = match self.advance() {
            Some((Token::Ident(name), span)) => (name.clone(), span.end),
            Some((Token::Reserved(name), span)) => (name.clone(), span.end),
            other => {
                return Err(ParseError {
                    message: format!(
                        "expected identifier after '.', found {:?}",
                        other.map(|(t, _)| t)
                    ),
                    span: self.peek_span(),
                });
            }
        };

        Ok(Spanned::new(
            self.next_id(),
            Expr::Member {
                expr: Box::new(base),
                field,
                optional,
            },
            start..end,
        ))
    }

    /// Parse struct initialization: Type{field: value, ...}
    fn parse_struct_init(&mut self, type_name: SpannedExpr) -> Result<SpannedExpr, ParseError> {
        let start = type_name.span.start;
        self.expect(&Token::LBrace)?;

        let mut fields = Vec::new();
        if !self.check(&Token::RBrace) {
            fields.push(self.parse_struct_field()?);
            while self.match_token(&Token::Comma) {
                if self.check(&Token::RBrace) {
                    break; // trailing comma
                }
                fields.push(self.parse_struct_field()?);
            }
        }

        let end_span = self.expect(&Token::RBrace)?;

        Ok(Spanned::new(
            self.next_id(),
            Expr::Struct {
                type_name: Box::new(type_name),
                fields,
            },
            start..end_span.end,
        ))
    }

    /// Parse a struct field: [?]name: value
    fn parse_struct_field(&mut self) -> Result<StructField, ParseError> {
        let optional = self.match_token(&Token::Question);

        let name = match self.advance() {
            Some((Token::Ident(name), _)) => name.clone(),
            Some((Token::Reserved(name), _)) => name.clone(),
            other => {
                return Err(ParseError {
                    message: format!("expected field name, found {:?}", other.map(|(t, _)| t)),
                    span: self.peek_span(),
                });
            }
        };

        self.expect(&Token::Colon)?;
        let value = self.parse_expr()?;

        Ok(StructField {
            name,
            value,
            optional,
        })
    }

    /// Parse an atom: literal, identifier, parenthesized expression, list, or map.
    fn parse_atom(&mut self) -> Result<SpannedExpr, ParseError> {
        let span = self.peek_span();

        // Clone the token to avoid borrowing issues
        let token = self.peek().cloned();

        match token {
            // Literals
            Some(Token::Int(n)) => {
                self.advance();
                Ok(Spanned::new(self.next_id(), Expr::Int(n), span))
            }
            Some(Token::UInt(n)) => {
                self.advance();
                Ok(Spanned::new(self.next_id(), Expr::UInt(n), span))
            }
            Some(Token::Float(n)) => {
                self.advance();
                Ok(Spanned::new(self.next_id(), Expr::Float(n), span))
            }
            Some(Token::String(s)) => {
                self.advance();
                Ok(Spanned::new(self.next_id(), Expr::String(s), span))
            }
            Some(Token::Bytes(b)) => {
                self.advance();
                Ok(Spanned::new(self.next_id(), Expr::Bytes(b), span))
            }
            Some(Token::True) => {
                self.advance();
                Ok(Spanned::new(self.next_id(), Expr::Bool(true), span))
            }
            Some(Token::False) => {
                self.advance();
                Ok(Spanned::new(self.next_id(), Expr::Bool(false), span))
            }
            Some(Token::Null) => {
                self.advance();
                Ok(Spanned::new(self.next_id(), Expr::Null, span))
            }

            // Identifier
            Some(Token::Ident(name)) => {
                self.advance();
                Ok(Spanned::new(self.next_id(), Expr::Ident(name), span))
            }

            // Reserved word - error
            Some(Token::Reserved(word)) => Err(ParseError {
                message: format!(
                    "'{}' is a reserved word and cannot be used as an identifier",
                    word
                ),
                span,
            }),

            // Integer overflow - error (unless preceded by -, which is handled in parse_unary)
            Some(Token::IntOverflow(s)) => Err(ParseError {
                message: format!("integer literal '{}' overflows i64", s),
                span,
            }),

            // Root identifier: .name
            Some(Token::Dot) => {
                self.advance();
                match self.advance() {
                    Some((Token::Ident(name), end_span)) => {
                        let name = name.clone();
                        let end = end_span.end;
                        Ok(Spanned::new(
                            self.next_id(),
                            Expr::RootIdent(name),
                            span.start..end,
                        ))
                    }
                    _ => Err(ParseError {
                        message: "expected identifier after '.'".to_string(),
                        span: self.peek_span(),
                    }),
                }
            }

            // Parenthesized expression
            Some(Token::LParen) => {
                self.advance();
                let expr = self.parse_expr()?;
                self.expect(&Token::RParen)?;
                Ok(expr)
            }

            // List literal
            Some(Token::LBracket) => self.parse_list(),

            // Map literal
            Some(Token::LBrace) => self.parse_map(),

            // Error cases
            Some(token) => Err(ParseError {
                message: format!("unexpected token: {:?}", token),
                span,
            }),

            None => Err(ParseError {
                message: "unexpected end of input".to_string(),
                span: self.eof_span(),
            }),
        }
    }

    /// Parse a list element: [?]expr
    fn parse_list_element(&mut self) -> Result<ListElement, ParseError> {
        let optional = self.match_token(&Token::Question);
        let expr = self.parse_expr()?;
        Ok(ListElement { expr, optional })
    }

    /// Parse a list literal: [[?]expr, [?]expr, ...]
    fn parse_list(&mut self) -> Result<SpannedExpr, ParseError> {
        let start = self.peek_span().start;
        self.expect(&Token::LBracket)?;

        let mut items = Vec::new();
        if !self.check(&Token::RBracket) {
            items.push(self.parse_list_element()?);
            while self.match_token(&Token::Comma) {
                if self.check(&Token::RBracket) {
                    break; // trailing comma
                }
                items.push(self.parse_list_element()?);
            }
        }

        let end_span = self.expect(&Token::RBracket)?;

        Ok(Spanned::new(
            self.next_id(),
            Expr::List(items),
            start..end_span.end,
        ))
    }

    /// Parse a map entry: [?]expr: expr
    fn parse_map_entry(&mut self) -> Result<MapEntry, ParseError> {
        let optional = self.match_token(&Token::Question);
        let key = self.parse_expr()?;
        self.expect(&Token::Colon)?;
        let value = self.parse_expr()?;
        Ok(MapEntry {
            key,
            value,
            optional,
        })
    }

    /// Parse a map literal: {[?]expr: expr, [?]expr: expr, ...}
    fn parse_map(&mut self) -> Result<SpannedExpr, ParseError> {
        let start = self.peek_span().start;
        self.expect(&Token::LBrace)?;

        let mut entries = Vec::new();
        if !self.check(&Token::RBrace) {
            entries.push(self.parse_map_entry()?);

            while self.match_token(&Token::Comma) {
                if self.check(&Token::RBrace) {
                    break; // trailing comma
                }
                entries.push(self.parse_map_entry()?);
            }
        }

        let end_span = self.expect(&Token::RBrace)?;

        Ok(Spanned::new(
            self.next_id(),
            Expr::Map(entries),
            start..end_span.end,
        ))
    }
}

/// Parse tokens into an AST with inline macro expansion using standard macros.
/// Returns the AST, any parse errors encountered, and a map of macro call IDs to original expressions.
/// Macro expansion errors (wrong arity, invalid arguments) are included in the errors list
/// and produce `Expr::Error` nodes in the AST to prevent misleading checker errors.
#[allow(dead_code)]
pub fn parse_tokens(tokens: &[SpannedToken]) -> (Option<SpannedExpr>, Vec<ParseError>, MacroCalls) {
    if tokens.is_empty() {
        return (
            None,
            vec![ParseError {
                message: "empty input".to_string(),
                span: 0..0,
            }],
            MacroCalls::new(),
        );
    }

    let mut parser = Parser::new(tokens);
    match parser.parse_expr() {
        Ok(ast) => {
            let macro_calls = parser.take_macro_calls();
            let mut errors = parser.take_macro_errors();
            if parser.at_end() {
                (Some(ast), errors, macro_calls)
            } else {
                errors.push(ParseError {
                    message: "unexpected tokens after expression".to_string(),
                    span: parser.peek_span(),
                });
                (Some(ast), errors, macro_calls)
            }
        }
        Err(e) => {
            let mut errors = parser.take_macro_errors();
            errors.push(e);
            (None, errors, MacroCalls::new())
        }
    }
}

/// Parse tokens into an AST with inline macro expansion using a custom macro registry.
/// Returns the AST, any parse errors encountered, and a map of macro call IDs to original expressions.
/// Macro expansion errors (wrong arity, invalid arguments) are included in the errors list
/// and produce `Expr::Error` nodes in the AST to prevent misleading checker errors.
pub fn parse_tokens_with_macros(
    tokens: &[SpannedToken],
    macros: MacroRegistry,
) -> (Option<SpannedExpr>, Vec<ParseError>, MacroCalls) {
    if tokens.is_empty() {
        return (
            None,
            vec![ParseError {
                message: "empty input".to_string(),
                span: 0..0,
            }],
            MacroCalls::new(),
        );
    }

    let mut parser = Parser::with_macros(tokens, macros);
    match parser.parse_expr() {
        Ok(ast) => {
            let macro_calls = parser.take_macro_calls();
            let mut errors = parser.take_macro_errors();
            if parser.at_end() {
                (Some(ast), errors, macro_calls)
            } else {
                errors.push(ParseError {
                    message: "unexpected tokens after expression".to_string(),
                    span: parser.peek_span(),
                });
                (Some(ast), errors, macro_calls)
            }
        }
        Err(e) => {
            let mut errors = parser.take_macro_errors();
            errors.push(e);
            (None, errors, MacroCalls::new())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::lexer::lex;
    use super::*;
    use crate::types::{BinaryOp, UnaryOp};

    fn parse_expr(input: &str) -> SpannedExpr {
        let tokens = lex(input).unwrap();
        let (ast, errors, _macro_calls) = parse_tokens(&tokens);
        assert!(errors.is_empty(), "unexpected errors: {:?}", errors);
        ast.expect("expected AST")
    }

    fn parse_expr_node(input: &str) -> Expr {
        parse_expr(input).node
    }

    #[test]
    fn parse_literals() {
        assert_eq!(parse_expr_node("123"), Expr::Int(123));
        assert_eq!(parse_expr_node("123u"), Expr::UInt(123));
        assert_eq!(parse_expr_node("1.5"), Expr::Float(1.5));
        assert_eq!(
            parse_expr_node(r#""hello""#),
            Expr::String("hello".to_string())
        );
        assert_eq!(parse_expr_node("true"), Expr::Bool(true));
        assert_eq!(parse_expr_node("false"), Expr::Bool(false));
        assert_eq!(parse_expr_node("null"), Expr::Null);
    }

    #[test]
    fn parse_identifier() {
        assert_eq!(parse_expr_node("foo"), Expr::Ident("foo".to_string()));
    }

    #[test]
    fn parse_list() {
        if let Expr::List(items) = parse_expr_node("[1, 2, 3]") {
            assert_eq!(items.len(), 3);
            assert_eq!(items[0].expr.node, Expr::Int(1));
            assert_eq!(items[1].expr.node, Expr::Int(2));
            assert_eq!(items[2].expr.node, Expr::Int(3));
            assert!(!items[0].optional);
            assert!(!items[1].optional);
            assert!(!items[2].optional);
        } else {
            panic!("expected list");
        }
    }

    #[test]
    fn parse_map() {
        if let Expr::Map(entries) = parse_expr_node(r#"{"a": 1, "b": 2}"#) {
            assert_eq!(entries.len(), 2);
            assert_eq!(entries[0].key.node, Expr::String("a".to_string()));
            assert_eq!(entries[0].value.node, Expr::Int(1));
            assert!(!entries[0].optional);
        } else {
            panic!("expected map");
        }
    }

    #[test]
    fn parse_binary_ops() {
        if let Expr::Binary { op, left, right } = parse_expr_node("1 + 2") {
            assert_eq!(op, BinaryOp::Add);
            assert_eq!(left.node, Expr::Int(1));
            assert_eq!(right.node, Expr::Int(2));
        } else {
            panic!("expected binary");
        }
    }

    #[test]
    fn parse_precedence() {
        // 1 + 2 * 3 should parse as 1 + (2 * 3)
        if let Expr::Binary { op, left, right } = parse_expr_node("1 + 2 * 3") {
            assert_eq!(op, BinaryOp::Add);
            assert_eq!(left.node, Expr::Int(1));
            if let Expr::Binary {
                op: inner_op,
                left: inner_left,
                right: inner_right,
            } = &right.node
            {
                assert_eq!(*inner_op, BinaryOp::Mul);
                assert_eq!(inner_left.node, Expr::Int(2));
                assert_eq!(inner_right.node, Expr::Int(3));
            } else {
                panic!("expected inner binary");
            }
        } else {
            panic!("expected binary");
        }
    }

    #[test]
    fn parse_associativity() {
        // 1 - 2 - 3 should parse as (1 - 2) - 3 (left associative)
        if let Expr::Binary { op, left, right } = parse_expr_node("1 - 2 - 3") {
            assert_eq!(op, BinaryOp::Sub);
            assert_eq!(right.node, Expr::Int(3));
            if let Expr::Binary {
                op: inner_op,
                left: inner_left,
                right: inner_right,
            } = &left.node
            {
                assert_eq!(*inner_op, BinaryOp::Sub);
                assert_eq!(inner_left.node, Expr::Int(1));
                assert_eq!(inner_right.node, Expr::Int(2));
            } else {
                panic!("expected inner binary");
            }
        } else {
            panic!("expected binary");
        }
    }

    #[test]
    fn parse_unary() {
        if let Expr::Unary { op, expr } = parse_expr_node("-x") {
            assert_eq!(op, UnaryOp::Neg);
            assert_eq!(expr.node, Expr::Ident("x".to_string()));
        } else {
            panic!("expected unary");
        }
    }

    #[test]
    fn parse_member_access() {
        if let Expr::Member {
            expr,
            field,
            optional,
        } = parse_expr_node("a.b")
        {
            assert_eq!(expr.node, Expr::Ident("a".to_string()));
            assert_eq!(field, "b");
            assert!(!optional);
        } else {
            panic!("expected member access");
        }
    }

    #[test]
    fn parse_index() {
        if let Expr::Index {
            expr,
            index,
            optional,
        } = parse_expr_node("a[0]")
        {
            assert_eq!(expr.node, Expr::Ident("a".to_string()));
            assert_eq!(index.node, Expr::Int(0));
            assert!(!optional);
        } else {
            panic!("expected index");
        }
    }

    #[test]
    fn parse_call() {
        if let Expr::Call { expr, args } = parse_expr_node("f(x, y)") {
            assert_eq!(expr.node, Expr::Ident("f".to_string()));
            assert_eq!(args.len(), 2);
            assert_eq!(args[0].node, Expr::Ident("x".to_string()));
            assert_eq!(args[1].node, Expr::Ident("y".to_string()));
        } else {
            panic!("expected call");
        }
    }

    #[test]
    fn parse_ternary() {
        if let Expr::Ternary {
            cond,
            then_expr,
            else_expr,
        } = parse_expr_node("a ? b : c")
        {
            assert_eq!(cond.node, Expr::Ident("a".to_string()));
            assert_eq!(then_expr.node, Expr::Ident("b".to_string()));
            assert_eq!(else_expr.node, Expr::Ident("c".to_string()));
        } else {
            panic!("expected ternary");
        }
    }

    #[test]
    fn parse_chained_member_access() {
        if let Expr::Member { expr, field, .. } = parse_expr_node("a.b.c") {
            assert_eq!(field, "c");
            if let Expr::Member {
                expr: inner_expr,
                field: inner_field,
                ..
            } = &expr.node
            {
                assert_eq!(inner_expr.node, Expr::Ident("a".to_string()));
                assert_eq!(inner_field, "b");
            } else {
                panic!("expected inner member");
            }
        } else {
            panic!("expected member access");
        }
    }

    // === ID Assignment Tests ===

    #[test]
    fn ids_start_at_one() {
        let ast = parse_expr("123");
        assert_eq!(ast.id, 1);
    }

    #[test]
    fn ids_are_unique_in_expression() {
        let ast = parse_expr("1 + 2");

        // Collect all IDs from the expression tree
        fn collect_ids(expr: &SpannedExpr, ids: &mut Vec<i64>) {
            ids.push(expr.id);
            match &expr.node {
                Expr::Binary { left, right, .. } => {
                    collect_ids(left, ids);
                    collect_ids(right, ids);
                }
                Expr::Unary { expr, .. } => {
                    collect_ids(expr, ids);
                }
                Expr::List(elements) => {
                    for e in elements {
                        collect_ids(&e.expr, ids);
                    }
                }
                _ => {}
            }
        }

        let mut ids = Vec::new();
        collect_ids(&ast, &mut ids);

        // All IDs should be unique
        let mut sorted = ids.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(ids.len(), sorted.len(), "IDs should be unique");
    }

    #[test]
    fn ids_reset_per_parse() {
        // First parse
        let ast1 = parse_expr("123");

        // Second parse (separate call)
        let ast2 = parse_expr("456");

        // Both should start at 1 since each parse gets a fresh counter
        assert_eq!(ast1.id, 1);
        assert_eq!(ast2.id, 1);
    }

    #[test]
    fn ids_are_depth_first() {
        // For "1 + 2", the parser creates nodes in this order:
        // 1. Parse left operand (1) -> ID 1
        // 2. Parse right operand (2) -> ID 2
        // 3. Create binary node -> ID 3
        let ast = parse_expr("1 + 2");

        if let Expr::Binary { left, right, .. } = &ast.node {
            // Child nodes should have lower IDs than parent
            assert!(
                left.id < ast.id,
                "left child should have lower ID than parent"
            );
            assert!(
                right.id < ast.id,
                "right child should have lower ID than parent"
            );
            // Left should be parsed before right
            assert!(
                left.id < right.id,
                "left child should have lower ID than right child"
            );
        } else {
            panic!("expected binary");
        }
    }

    // === Two-Variable Comprehension Macro Tests ===

    #[test]
    fn expand_exists_3arg() {
        let ast = parse_expr("[1,2].exists(i, v, i < v)");
        if let Expr::Comprehension(comp) = &ast.node {
            assert_eq!(comp.iter_var, "i");
            assert_eq!(comp.iter_var2, "v");
        } else {
            panic!("expected comprehension, got {:?}", ast.node);
        }
    }

    #[test]
    fn expand_all_3arg() {
        let ast = parse_expr("[1,2,3].all(i, v, v > 0)");
        if let Expr::Comprehension(comp) = &ast.node {
            assert_eq!(comp.iter_var, "i");
            assert_eq!(comp.iter_var2, "v");
        } else {
            panic!("expected comprehension, got {:?}", ast.node);
        }
    }

    #[test]
    fn expand_exists_one_3arg() {
        let ast = parse_expr("[7].exists_one(i, v, i == 0 && v == 7)");
        if let Expr::Comprehension(comp) = &ast.node {
            assert_eq!(comp.iter_var, "i");
            assert_eq!(comp.iter_var2, "v");
        } else {
            panic!("expected comprehension, got {:?}", ast.node);
        }
    }

    #[test]
    fn expand_transform_list_3arg() {
        let ast = parse_expr("[2,4,6].transformList(i, v, v / 2)");
        if let Expr::Comprehension(comp) = &ast.node {
            assert_eq!(comp.iter_var, "i");
            assert_eq!(comp.iter_var2, "v");
        } else {
            panic!("expected comprehension, got {:?}", ast.node);
        }
    }

    #[test]
    fn expand_transform_list_4arg() {
        let ast = parse_expr("[2,4,6].transformList(i, v, i != 1, v / 2)");
        if let Expr::Comprehension(comp) = &ast.node {
            assert_eq!(comp.iter_var, "i");
            assert_eq!(comp.iter_var2, "v");
        } else {
            panic!("expected comprehension, got {:?}", ast.node);
        }
    }

    #[test]
    fn expand_transform_map_3arg() {
        let ast = parse_expr(r#"{"foo": "bar"}.transformMap(k, v, k + v)"#);
        if let Expr::Comprehension(comp) = &ast.node {
            assert_eq!(comp.iter_var, "k");
            assert_eq!(comp.iter_var2, "v");
        } else {
            panic!("expected comprehension, got {:?}", ast.node);
        }
    }

    #[test]
    fn expand_transform_map_4arg() {
        let ast = parse_expr(r#"{"foo": "bar"}.transformMap(k, v, k == "foo", k + v)"#);
        if let Expr::Comprehension(comp) = &ast.node {
            assert_eq!(comp.iter_var, "k");
            assert_eq!(comp.iter_var2, "v");
        } else {
            panic!("expected comprehension, got {:?}", ast.node);
        }
    }

    #[test]
    fn exists_2arg_still_works() {
        // Ensure 2-arg form still works
        let ast = parse_expr("[1,2].exists(x, x > 0)");
        if let Expr::Comprehension(comp) = &ast.node {
            assert_eq!(comp.iter_var, "x");
            assert!(
                comp.iter_var2.is_empty(),
                "2-arg form should have empty iter_var2"
            );
        } else {
            panic!("expected comprehension, got {:?}", ast.node);
        }
    }

    #[test]
    fn wrong_arg_count_returns_error() {
        // exists with 1 arg should return Expr::Error with a helpful arity error
        let tokens = lex("[1,2].exists(x)").unwrap();
        let (ast, errors, _) = parse_tokens(&tokens);
        assert!(ast.is_some());
        assert!(
            errors
                .iter()
                .any(|e| e.message.contains("exists()") && e.message.contains("requires")),
            "expected arity error for exists(), got: {:?}",
            errors
        );
    }

    // === Macro Registry Tests ===

    #[test]
    fn parse_with_no_macros() {
        let tokens = lex("[1,2].all(x, x > 0)").unwrap();
        let (ast, errors, _) = parse_tokens_with_macros(&tokens, MacroRegistry::new());
        assert!(errors.is_empty());
        let ast = ast.unwrap();
        // Without macros, all() should be a regular call
        if let Expr::Call { .. } = &ast.node {
            // Expected
        } else {
            panic!("expected Call without macros, got {:?}", ast.node);
        }
    }

    #[test]
    fn parse_with_custom_macros() {
        use super::super::macros::{ArgCount, Macro, MacroContext, MacroExpansion, MacroStyle};
        use crate::types::Span;

        fn custom_has_expander(
            ctx: &mut MacroContext,
            span: Span,
            _receiver: Option<SpannedExpr>,
            args: Vec<SpannedExpr>,
        ) -> MacroExpansion {
            let arg = args.into_iter().next().unwrap();
            match arg.node {
                Expr::Member { expr, field, .. } => MacroExpansion::Expanded(Spanned::new(
                    ctx.next_id(),
                    Expr::MemberTestOnly { expr, field },
                    span,
                )),
                _ => MacroExpansion::Error("has() requires member expression".to_string()),
            }
        }

        // Create a registry with only 'has' macro
        let mut registry = MacroRegistry::new();
        registry.register(Macro::new(
            "has",
            MacroStyle::Global,
            ArgCount::Exact(1),
            custom_has_expander,
        ));

        // has() should expand
        let tokens = lex("has(m.x)").unwrap();
        let (ast, errors, _) = parse_tokens_with_macros(&tokens, registry.clone());
        assert!(errors.is_empty());
        let ast = ast.unwrap();
        assert!(matches!(ast.node, Expr::MemberTestOnly { .. }));

        // all() should NOT expand (not in our custom registry)
        let tokens = lex("[1,2].all(x, x > 0)").unwrap();
        let (ast, errors, _) = parse_tokens_with_macros(&tokens, registry);
        assert!(errors.is_empty());
        let ast = ast.unwrap();
        assert!(matches!(ast.node, Expr::Call { .. }));
    }

    #[test]
    fn macro_has_zero_args_gives_arity_error() {
        let tokens = lex("has()").unwrap();
        let (ast, errors, _) = parse_tokens(&tokens);
        assert!(ast.is_some());
        assert!(
            errors.iter().any(|e| e.message.contains("requires")),
            "expected arity error, got: {:?}",
            errors
        );
        // Should produce Expr::Error, not an undeclared reference
        assert!(matches!(ast.unwrap().node, Expr::Error));
    }

    #[test]
    fn macro_has_too_many_args_gives_arity_error() {
        let tokens = lex("has(a.b, c.d)").unwrap();
        let (ast, errors, _) = parse_tokens(&tokens);
        assert!(ast.is_some());
        assert!(
            errors.iter().any(|e| e.message.contains("requires")),
            "expected arity error, got: {:?}",
            errors
        );
        assert!(matches!(ast.unwrap().node, Expr::Error));
    }

    #[test]
    fn macro_has_non_field_arg_gives_expansion_error() {
        let tokens = lex("has(1 + 2)").unwrap();
        let (ast, errors, _) = parse_tokens(&tokens);
        assert!(ast.is_some());
        assert!(
            errors.iter().any(|e| e.message.contains("field selection")),
            "expected field selection error, got: {:?}",
            errors
        );
        assert!(matches!(ast.unwrap().node, Expr::Error));
    }

    #[test]
    fn macro_all_zero_args_gives_arity_error() {
        let tokens = lex("[1].all()").unwrap();
        let (ast, errors, _) = parse_tokens(&tokens);
        assert!(ast.is_some());
        assert!(
            errors
                .iter()
                .any(|e| e.message.contains("all()") && e.message.contains("requires")),
            "expected arity error for all(), got: {:?}",
            errors
        );
    }

    #[test]
    fn macro_filter_too_many_args_gives_arity_error() {
        let tokens = lex("[1].filter(x, y, z)").unwrap();
        let (ast, errors, _) = parse_tokens(&tokens);
        assert!(ast.is_some());
        assert!(
            errors
                .iter()
                .any(|e| e.message.contains("filter()") && e.message.contains("requires")),
            "expected arity error for filter(), got: {:?}",
            errors
        );
    }

    #[test]
    fn macro_errors_do_not_produce_undeclared_reference() {
        // All these should produce macro-level errors, not "undeclared reference" errors
        let cases = vec!["has()", "has(a.b, c.d)", "has(1 + 2)"];
        for input in cases {
            let tokens = lex(input).unwrap();
            let (_ast, errors, _) = parse_tokens(&tokens);
            assert!(
                !errors.iter().any(|e| e.message.contains("undeclared")),
                "input '{}' should not produce 'undeclared reference' error, got: {:?}",
                input,
                errors
            );
        }
    }
}
