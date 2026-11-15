//! CEL expression unparser (AST to source text).
//!
//! This module converts a CEL AST back into source text. The output is
//! semantically equivalent to the original expression but may differ in
//! formatting (whitespace, parenthesization, etc.).
//!
//! # Example
//!
//! ```
//! use cel_core::ast_to_string;
//! use cel_core::parse;
//!
//! let ast = parse("x + 1").ast.unwrap();
//! let source = ast_to_string(&ast);
//! assert_eq!(source, "x + 1");
//! ```

use crate::types::{
    BinaryOp, ComprehensionData, Expr, ListElement, MapEntry, SpannedExpr, StructField, UnaryOp,
};

/// Convert a CEL AST to source text.
///
/// The output is a valid CEL expression that is semantically equivalent
/// to the input AST. Formatting may differ from the original source.
pub fn ast_to_string(expr: &SpannedExpr) -> String {
    unparse(&expr.node)
}

/// Returns the precedence of a binary operator (higher = binds tighter).
fn precedence(op: BinaryOp) -> u8 {
    match op {
        BinaryOp::Or => 1,
        BinaryOp::And => 2,
        BinaryOp::Eq
        | BinaryOp::Ne
        | BinaryOp::Lt
        | BinaryOp::Le
        | BinaryOp::Gt
        | BinaryOp::Ge
        | BinaryOp::In => 3,
        BinaryOp::Add | BinaryOp::Sub => 4,
        BinaryOp::Mul | BinaryOp::Div | BinaryOp::Mod => 5,
    }
}

/// Returns the operator symbol for a binary operator.
fn binary_op_symbol(op: BinaryOp) -> &'static str {
    match op {
        BinaryOp::Add => "+",
        BinaryOp::Sub => "-",
        BinaryOp::Mul => "*",
        BinaryOp::Div => "/",
        BinaryOp::Mod => "%",
        BinaryOp::Eq => "==",
        BinaryOp::Ne => "!=",
        BinaryOp::Lt => "<",
        BinaryOp::Le => "<=",
        BinaryOp::Gt => ">",
        BinaryOp::Ge => ">=",
        BinaryOp::In => "in",
        BinaryOp::And => "&&",
        BinaryOp::Or => "||",
    }
}

/// Returns the operator symbol for a unary operator.
fn unary_op_symbol(op: UnaryOp) -> &'static str {
    match op {
        UnaryOp::Neg => "-",
        UnaryOp::Not => "!",
    }
}

/// Unparse an expression to a string.
fn unparse(expr: &Expr) -> String {
    match expr {
        // Literals
        Expr::Null => "null".to_string(),
        Expr::Bool(b) => b.to_string(),
        Expr::Int(n) => n.to_string(),
        Expr::UInt(n) => format!("{}u", n),
        Expr::Float(f) => format_float(*f),
        Expr::String(s) => format!("\"{}\"", escape_string(s)),
        Expr::Bytes(b) => format!("b\"{}\"", escape_bytes(b)),

        // Identifiers
        Expr::Ident(name) => name.clone(),
        Expr::RootIdent(name) => format!(".{}", name),

        // Collections
        Expr::List(elements) => unparse_list(elements),
        Expr::Map(entries) => unparse_map(entries),

        // Operations
        Expr::Unary { op, expr } => unparse_unary(*op, expr),
        Expr::Binary { op, left, right } => unparse_binary(*op, left, right),
        Expr::Ternary {
            cond,
            then_expr,
            else_expr,
        } => {
            format!(
                "{} ? {} : {}",
                unparse_with_parens_if_needed(&cond.node, Some(0)),
                unparse(&then_expr.node),
                unparse(&else_expr.node)
            )
        }

        // Access
        Expr::Member {
            expr,
            field,
            optional,
        } => {
            let op = if *optional { ".?" } else { "." };
            format!("{}{}{}", unparse_primary(&expr.node), op, field)
        }
        Expr::Index {
            expr,
            index,
            optional,
        } => {
            let brackets = if *optional { "[?" } else { "[" };
            format!(
                "{}{}{}]",
                unparse_primary(&expr.node),
                brackets,
                unparse(&index.node)
            )
        }
        Expr::Call { expr, args } => unparse_call(expr, args),
        Expr::Struct { type_name, fields } => unparse_struct(type_name, fields),

        // Macro expansions - unparse back to macro syntax where possible
        Expr::Comprehension(comp) => unparse_comprehension(comp),
        Expr::MemberTestOnly { expr, field } => {
            format!("has({}.{})", unparse(&expr.node), field)
        }
        Expr::Bind {
            var_name,
            init,
            body,
        } => {
            format!(
                "cel.bind({}, {}, {})",
                var_name,
                unparse(&init.node),
                unparse(&body.node)
            )
        }

        Expr::Error => "<error>".to_string(),
    }
}

/// Format a float, ensuring it always has a decimal point or exponent.
fn format_float(f: f64) -> String {
    if f.is_nan() {
        return "double(\"NaN\")".to_string();
    }
    if f.is_infinite() {
        return if f.is_sign_positive() {
            "double(\"Infinity\")".to_string()
        } else {
            "double(\"-Infinity\")".to_string()
        };
    }

    let s = f.to_string();
    // Ensure we have a decimal point or exponent to distinguish from int
    if s.contains('.') || s.contains('e') || s.contains('E') {
        s
    } else {
        format!("{}.0", s)
    }
}

/// Escape a string for CEL output.
fn escape_string(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => result.push_str("\\\\"),
            '"' => result.push_str("\\\""),
            '\n' => result.push_str("\\n"),
            '\r' => result.push_str("\\r"),
            '\t' => result.push_str("\\t"),
            c if c.is_control() => {
                // Use Unicode escape for other control characters
                result.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => result.push(c),
        }
    }
    result
}

/// Escape bytes for CEL output.
fn escape_bytes(bytes: &[u8]) -> String {
    let mut result = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        match b {
            b'\\' => result.push_str("\\\\"),
            b'"' => result.push_str("\\\""),
            b'\n' => result.push_str("\\n"),
            b'\r' => result.push_str("\\r"),
            b'\t' => result.push_str("\\t"),
            b if b.is_ascii_graphic() || b == b' ' => result.push(b as char),
            b => result.push_str(&format!("\\x{:02x}", b)),
        }
    }
    result
}

/// Unparse a list expression.
fn unparse_list(elements: &[ListElement]) -> String {
    let items: Vec<String> = elements
        .iter()
        .map(|elem| {
            if elem.optional {
                format!("?{}", unparse(&elem.expr.node))
            } else {
                unparse(&elem.expr.node)
            }
        })
        .collect();
    format!("[{}]", items.join(", "))
}

/// Unparse a map expression.
fn unparse_map(entries: &[MapEntry]) -> String {
    let items: Vec<String> = entries
        .iter()
        .map(|entry| {
            let key = unparse(&entry.key.node);
            let value = unparse(&entry.value.node);
            if entry.optional {
                format!("?{}: {}", key, value)
            } else {
                format!("{}: {}", key, value)
            }
        })
        .collect();
    format!("{{{}}}", items.join(", "))
}

/// Unparse a unary expression.
fn unparse_unary(op: UnaryOp, expr: &SpannedExpr) -> String {
    let op_str = unary_op_symbol(op);
    // Need parens around binary expressions and ternaries
    match &expr.node {
        Expr::Binary { .. } | Expr::Ternary { .. } => {
            format!("{}({})", op_str, unparse(&expr.node))
        }
        _ => format!("{}{}", op_str, unparse(&expr.node)),
    }
}

/// Unparse a binary expression with proper precedence handling.
fn unparse_binary(op: BinaryOp, left: &SpannedExpr, right: &SpannedExpr) -> String {
    let op_prec = precedence(op);
    let left_str = unparse_with_parens_if_needed(&left.node, Some(op_prec));
    let right_str = unparse_with_parens_if_needed(&right.node, Some(op_prec));
    format!("{} {} {}", left_str, binary_op_symbol(op), right_str)
}

/// Unparse an expression, adding parentheses if needed based on precedence.
fn unparse_with_parens_if_needed(expr: &Expr, parent_prec: Option<u8>) -> String {
    match expr {
        Expr::Binary { op, .. } => {
            let expr_prec = precedence(*op);
            if let Some(p) = parent_prec {
                if expr_prec < p {
                    return format!("({})", unparse(expr));
                }
            }
            unparse(expr)
        }
        Expr::Ternary { .. } => {
            // Ternary always needs parens when nested in binary
            if parent_prec.is_some() {
                format!("({})", unparse(expr))
            } else {
                unparse(expr)
            }
        }
        _ => unparse(expr),
    }
}

/// Unparse a primary expression (may need parens for member/index access).
fn unparse_primary(expr: &Expr) -> String {
    match expr {
        Expr::Binary { .. } | Expr::Ternary { .. } | Expr::Unary { .. } => {
            format!("({})", unparse(expr))
        }
        _ => unparse(expr),
    }
}

/// Unparse a function call.
fn unparse_call(expr: &SpannedExpr, args: &[SpannedExpr]) -> String {
    let args_str: Vec<String> = args.iter().map(|a| unparse(&a.node)).collect();

    // Check if this is a method call (expr is Member) or function call (expr is Ident)
    match &expr.node {
        Expr::Ident(name) => {
            // Global function call
            format!("{}({})", name, args_str.join(", "))
        }
        Expr::Member {
            expr: receiver,
            field,
            optional: false,
        } => {
            // Method call: receiver.method(args)
            format!(
                "{}.{}({})",
                unparse_primary(&receiver.node),
                field,
                args_str.join(", ")
            )
        }
        Expr::Member {
            expr: receiver,
            field,
            optional: true,
        } => {
            // Optional method call: receiver.?method(args)
            format!(
                "{}.?{}({})",
                unparse_primary(&receiver.node),
                field,
                args_str.join(", ")
            )
        }
        _ => {
            // Generic callable expression
            format!("{}({})", unparse_primary(&expr.node), args_str.join(", "))
        }
    }
}

/// Unparse a struct literal.
fn unparse_struct(type_name: &SpannedExpr, fields: &[StructField]) -> String {
    let type_str = unparse(&type_name.node);
    let fields_str: Vec<String> = fields
        .iter()
        .map(|f| {
            if f.optional {
                format!("?{}: {}", f.name, unparse(&f.value.node))
            } else {
                format!("{}: {}", f.name, unparse(&f.value.node))
            }
        })
        .collect();
    format!("{}{{{}}}", type_str, fields_str.join(", "))
}

/// Unparse a comprehension back to macro syntax.
///
/// This attempts to recognize common macro patterns and unparse them appropriately.
/// For unrecognized patterns, it falls back to a generic representation.
fn unparse_comprehension(comp: &ComprehensionData) -> String {
    // Try to recognize common macro patterns

    // Check for `all` pattern: accu_init=true, loop_step=accu && condition, result=accu
    if matches!(&comp.accu_init.node, Expr::Bool(true)) {
        if let Expr::Binary {
            op: BinaryOp::And,
            left,
            right,
        } = &comp.loop_step.node
        {
            if matches!(&left.node, Expr::Ident(name) if name == &comp.accu_var)
                && matches!(&comp.result.node, Expr::Ident(name) if name == &comp.accu_var)
            {
                let range_str = unparse(&comp.iter_range.node);
                let condition_str = unparse(&right.node);
                return format!("{}.all({}, {})", range_str, comp.iter_var, condition_str);
            }
        }
    }

    // Check for `exists` pattern: accu_init=false, loop_step=accu || condition, result=accu
    if matches!(&comp.accu_init.node, Expr::Bool(false)) {
        if let Expr::Binary {
            op: BinaryOp::Or,
            left,
            right,
        } = &comp.loop_step.node
        {
            if matches!(&left.node, Expr::Ident(name) if name == &comp.accu_var)
                && matches!(&comp.result.node, Expr::Ident(name) if name == &comp.accu_var)
            {
                let range_str = unparse(&comp.iter_range.node);
                let condition_str = unparse(&right.node);
                return format!("{}.exists({}, {})", range_str, comp.iter_var, condition_str);
            }
        }
    }

    // Check for `map` pattern: accu_init=[], loop_step=accu + [transform], result=accu
    if matches!(&comp.accu_init.node, Expr::List(elems) if elems.is_empty()) {
        if let Expr::Binary {
            op: BinaryOp::Add,
            left,
            right,
        } = &comp.loop_step.node
        {
            if matches!(&left.node, Expr::Ident(name) if name == &comp.accu_var) {
                if let Expr::List(elems) = &right.node {
                    if elems.len() == 1
                        && !elems[0].optional
                        && matches!(&comp.result.node, Expr::Ident(name) if name == &comp.accu_var)
                    {
                        let range_str = unparse(&comp.iter_range.node);
                        let transform_str = unparse(&elems[0].expr.node);
                        return format!("{}.map({}, {})", range_str, comp.iter_var, transform_str);
                    }
                }
            }
        }
    }

    // Check for `filter` pattern: accu_init=[], loop_step=accu + ([iter_var] if condition else []), result=accu
    // This is more complex, so we'll use a simplified check
    if matches!(&comp.accu_init.node, Expr::List(elems) if elems.is_empty()) {
        if let Expr::Ternary {
            cond,
            then_expr,
            else_expr,
        } = &comp.loop_step.node
        {
            if let Expr::List(then_elems) = &then_expr.node {
                if then_elems.len() == 1 {
                    if let Expr::Ident(elem_name) = &then_elems[0].expr.node {
                        if elem_name == &comp.iter_var {
                            if let Expr::List(else_elems) = &else_expr.node {
                                if else_elems.is_empty()
                                    && matches!(&comp.result.node, Expr::Ident(name) if name == &comp.accu_var)
                                {
                                    let range_str = unparse(&comp.iter_range.node);
                                    let condition_str = unparse(&cond.node);
                                    return format!(
                                        "{}.filter({}, {})",
                                        range_str, comp.iter_var, condition_str
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // Fallback: generic comprehension representation
    // This is not valid CEL syntax but provides a readable representation
    let iter_vars = if comp.iter_var2.is_empty() {
        comp.iter_var.to_string()
    } else {
        format!("{}, {}", comp.iter_var, comp.iter_var2)
    };

    format!(
        "__comprehension__({}, {}, {}, {}, {}, {}, {})",
        unparse(&comp.iter_range.node),
        iter_vars,
        comp.accu_var,
        unparse(&comp.accu_init.node),
        unparse(&comp.loop_condition.node),
        unparse(&comp.loop_step.node),
        unparse(&comp.result.node)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse;

    fn roundtrip(source: &str) -> String {
        let ast = parse(source).ast.expect("parse failed");
        ast_to_string(&ast)
    }

    #[test]
    fn test_literals() {
        assert_eq!(roundtrip("null"), "null");
        assert_eq!(roundtrip("true"), "true");
        assert_eq!(roundtrip("false"), "false");
        assert_eq!(roundtrip("42"), "42");
        assert_eq!(roundtrip("42u"), "42u");
        assert_eq!(roundtrip("3.14"), "3.14");
        assert_eq!(roundtrip("\"hello\""), "\"hello\"");
        assert_eq!(roundtrip("b\"bytes\""), "b\"bytes\"");
    }

    #[test]
    fn test_string_escaping() {
        assert_eq!(roundtrip(r#""hello\nworld""#), "\"hello\\nworld\"");
        assert_eq!(roundtrip(r#""tab\there""#), "\"tab\\there\"");
        assert_eq!(roundtrip(r#""quote\"here""#), "\"quote\\\"here\"");
    }

    #[test]
    fn test_identifiers() {
        assert_eq!(roundtrip("x"), "x");
        assert_eq!(roundtrip("foo_bar"), "foo_bar");
    }

    #[test]
    fn test_collections() {
        assert_eq!(roundtrip("[]"), "[]");
        assert_eq!(roundtrip("[1, 2, 3]"), "[1, 2, 3]");
        assert_eq!(roundtrip("{}"), "{}");
        assert_eq!(roundtrip("{\"a\": 1, \"b\": 2}"), "{\"a\": 1, \"b\": 2}");
    }

    #[test]
    fn test_unary() {
        assert_eq!(roundtrip("-x"), "-x");
        assert_eq!(roundtrip("!x"), "!x");
        assert_eq!(roundtrip("--x"), "--x");
    }

    #[test]
    fn test_binary() {
        assert_eq!(roundtrip("x + y"), "x + y");
        assert_eq!(roundtrip("x - y"), "x - y");
        assert_eq!(roundtrip("x * y"), "x * y");
        assert_eq!(roundtrip("x / y"), "x / y");
        assert_eq!(roundtrip("x % y"), "x % y");
        assert_eq!(roundtrip("x == y"), "x == y");
        assert_eq!(roundtrip("x != y"), "x != y");
        assert_eq!(roundtrip("x < y"), "x < y");
        assert_eq!(roundtrip("x <= y"), "x <= y");
        assert_eq!(roundtrip("x > y"), "x > y");
        assert_eq!(roundtrip("x >= y"), "x >= y");
        assert_eq!(roundtrip("x in y"), "x in y");
        assert_eq!(roundtrip("x && y"), "x && y");
        assert_eq!(roundtrip("x || y"), "x || y");
    }

    #[test]
    fn test_precedence() {
        // Ensure parentheses are added where needed
        assert_eq!(roundtrip("(x + y) * z"), "(x + y) * z");
        assert_eq!(roundtrip("x * (y + z)"), "x * (y + z)");
        // Same precedence doesn't need parens
        assert_eq!(roundtrip("x + y + z"), "x + y + z");
    }

    #[test]
    fn test_ternary() {
        assert_eq!(roundtrip("x ? y : z"), "x ? y : z");
        assert_eq!(roundtrip("a ? b ? c : d : e"), "a ? b ? c : d : e");
    }

    #[test]
    fn test_member_access() {
        assert_eq!(roundtrip("x.y"), "x.y");
        assert_eq!(roundtrip("x.y.z"), "x.y.z");
    }

    #[test]
    fn test_index_access() {
        assert_eq!(roundtrip("x[0]"), "x[0]");
        assert_eq!(roundtrip("x[\"key\"]"), "x[\"key\"]");
    }

    #[test]
    fn test_function_calls() {
        assert_eq!(roundtrip("f()"), "f()");
        assert_eq!(roundtrip("f(x)"), "f(x)");
        assert_eq!(roundtrip("f(x, y, z)"), "f(x, y, z)");
    }

    #[test]
    fn test_method_calls() {
        assert_eq!(roundtrip("x.size()"), "x.size()");
        assert_eq!(roundtrip("x.contains(y)"), "x.contains(y)");
        assert_eq!(
            roundtrip("\"hello\".startsWith(\"h\")"),
            "\"hello\".startsWith(\"h\")"
        );
    }

    #[test]
    fn test_complex_expression() {
        assert_eq!(
            roundtrip("x > 0 && y < 10 || z == \"test\""),
            "x > 0 && y < 10 || z == \"test\""
        );
    }

    #[test]
    fn test_has_macro() {
        // has() macro expands to MemberTestOnly
        assert_eq!(roundtrip("has(x.y)"), "has(x.y)");
    }
}
