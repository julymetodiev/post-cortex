//! Math extension library for CEL.
//!
//! This module provides additional math functions beyond the CEL standard library,
//! matching the cel-go math extension.
//!
//! # Functions
//!
//! - `math.greatest(...)` - Returns greatest of arguments (variadic or list)
//! - `math.least(...)` - Returns least of arguments (variadic or list)
//! - `math.ceil(double)` - Ceiling function
//! - `math.floor(double)` - Floor function
//! - `math.round(double)` - Round to nearest integer
//! - `math.trunc(double)` - Truncate toward zero
//! - `math.abs(number)` - Absolute value
//! - `math.sign(number)` - Sign of number (-1, 0, or 1)
//! - `math.isNaN(double)` - Check if NaN
//! - `math.isInf(double)` - Check if infinite
//! - `math.isFinite(double)` - Check if finite
//! - `math.bitAnd(int, int)` - Bitwise AND
//! - `math.bitOr(int, int)` - Bitwise OR
//! - `math.bitXor(int, int)` - Bitwise XOR
//! - `math.bitNot(int)` - Bitwise NOT
//! - `math.bitShiftLeft(int, int)` - Left shift
//! - `math.bitShiftRight(int, int)` - Right shift

use std::cmp::Ordering;

use crate::eval::{EvalError, Value};
use crate::types::{CelType, FunctionDecl, OverloadDecl};

/// Returns the math extension library function declarations.
pub fn math_extension() -> Vec<FunctionDecl> {
    let mut funcs = Vec::new();

    // math.greatest and math.least with multiple arities
    funcs.push(build_minmax_function("math.greatest", true));
    funcs.push(build_minmax_function("math.least", false));

    // Simple double functions
    funcs.push(
        FunctionDecl::new("math.ceil").with_overload(
            OverloadDecl::function("math_ceil_double", vec![CelType::Double], CelType::Double)
                .with_impl(|args| match &args[0] {
                    Value::Double(v) => Value::Double(v.ceil()),
                    _ => Value::error(EvalError::invalid_argument("expected double")),
                }),
        ),
    );
    funcs.push(
        FunctionDecl::new("math.floor").with_overload(
            OverloadDecl::function("math_floor_double", vec![CelType::Double], CelType::Double)
                .with_impl(|args| match &args[0] {
                    Value::Double(v) => Value::Double(v.floor()),
                    _ => Value::error(EvalError::invalid_argument("expected double")),
                }),
        ),
    );
    funcs.push(
        FunctionDecl::new("math.round").with_overload(
            OverloadDecl::function("math_round_double", vec![CelType::Double], CelType::Double)
                .with_impl(|args| match &args[0] {
                    Value::Double(v) => {
                        // Round half away from zero (CEL spec behavior)
                        let rounded = if *v >= 0.0 {
                            (*v + 0.5).floor()
                        } else {
                            (*v - 0.5).ceil()
                        };
                        Value::Double(rounded)
                    }
                    _ => Value::error(EvalError::invalid_argument("expected double")),
                }),
        ),
    );
    funcs.push(
        FunctionDecl::new("math.trunc").with_overload(
            OverloadDecl::function("math_trunc_double", vec![CelType::Double], CelType::Double)
                .with_impl(|args| match &args[0] {
                    Value::Double(v) => Value::Double(v.trunc()),
                    _ => Value::error(EvalError::invalid_argument("expected double")),
                }),
        ),
    );

    // math.abs
    funcs.push(
        FunctionDecl::new("math.abs")
            .with_overload(
                OverloadDecl::function("math_abs_int", vec![CelType::Int], CelType::Int).with_impl(
                    |args| match &args[0] {
                        Value::Int(v) => match v.checked_abs() {
                            Some(r) => Value::Int(r),
                            None => Value::error(EvalError::overflow("integer overflow in abs")),
                        },
                        _ => Value::error(EvalError::invalid_argument("expected int")),
                    },
                ),
            )
            .with_overload(
                OverloadDecl::function("math_abs_uint", vec![CelType::UInt], CelType::UInt)
                    .with_impl(|args| match &args[0] {
                        Value::UInt(v) => Value::UInt(*v),
                        _ => Value::error(EvalError::invalid_argument("expected uint")),
                    }),
            )
            .with_overload(
                OverloadDecl::function("math_abs_double", vec![CelType::Double], CelType::Double)
                    .with_impl(|args| match &args[0] {
                        Value::Double(v) => Value::Double(v.abs()),
                        _ => Value::error(EvalError::invalid_argument("expected double")),
                    }),
            ),
    );

    // math.sign
    funcs.push(
        FunctionDecl::new("math.sign")
            .with_overload(
                OverloadDecl::function("math_sign_int", vec![CelType::Int], CelType::Int)
                    .with_impl(|args| match &args[0] {
                        Value::Int(v) => Value::Int(v.signum()),
                        _ => Value::error(EvalError::invalid_argument("expected int")),
                    }),
            )
            .with_overload(
                OverloadDecl::function("math_sign_uint", vec![CelType::UInt], CelType::UInt)
                    .with_impl(|args| match &args[0] {
                        Value::UInt(v) => Value::UInt(if *v == 0 { 0 } else { 1 }),
                        _ => Value::error(EvalError::invalid_argument("expected uint")),
                    }),
            )
            .with_overload(
                OverloadDecl::function("math_sign_double", vec![CelType::Double], CelType::Double)
                    .with_impl(|args| match &args[0] {
                        Value::Double(v) => {
                            if v.is_nan() {
                                Value::Double(f64::NAN)
                            } else if *v > 0.0 {
                                Value::Double(1.0)
                            } else if *v < 0.0 {
                                Value::Double(-1.0)
                            } else {
                                Value::Double(0.0)
                            }
                        }
                        _ => Value::error(EvalError::invalid_argument("expected double")),
                    }),
            ),
    );

    // math.isNaN, isInf, isFinite
    funcs.push(
        FunctionDecl::new("math.isNaN").with_overload(
            OverloadDecl::function("math_isnan_double", vec![CelType::Double], CelType::Bool)
                .with_impl(|args| match &args[0] {
                    Value::Double(v) => Value::Bool(v.is_nan()),
                    _ => Value::error(EvalError::invalid_argument("expected double")),
                }),
        ),
    );
    funcs.push(
        FunctionDecl::new("math.isInf").with_overload(
            OverloadDecl::function("math_isinf_double", vec![CelType::Double], CelType::Bool)
                .with_impl(|args| match &args[0] {
                    Value::Double(v) => Value::Bool(v.is_infinite()),
                    _ => Value::error(EvalError::invalid_argument("expected double")),
                }),
        ),
    );
    funcs.push(
        FunctionDecl::new("math.isFinite").with_overload(
            OverloadDecl::function("math_isfinite_double", vec![CelType::Double], CelType::Bool)
                .with_impl(|args| match &args[0] {
                    Value::Double(v) => Value::Bool(v.is_finite()),
                    _ => Value::error(EvalError::invalid_argument("expected double")),
                }),
        ),
    );

    // Bit operations
    add_bit_operations(&mut funcs);

    funcs
}

/// Compare two numeric values, returning an ordering.
/// Handles cross-type comparison between Int, UInt, and Double.
fn compare_numeric(a: &Value, b: &Value) -> Option<Ordering> {
    match (a, b) {
        (Value::Int(x), Value::Int(y)) => x.partial_cmp(y),
        (Value::UInt(x), Value::UInt(y)) => x.partial_cmp(y),
        (Value::Double(x), Value::Double(y)) => x.partial_cmp(y),
        (Value::Int(x), Value::UInt(y)) => {
            if *x < 0 {
                Some(Ordering::Less)
            } else {
                (*x as u64).partial_cmp(y)
            }
        }
        (Value::UInt(x), Value::Int(y)) => {
            if *y < 0 {
                Some(Ordering::Greater)
            } else {
                x.partial_cmp(&(*y as u64))
            }
        }
        (Value::Int(x), Value::Double(y)) => (*x as f64).partial_cmp(y),
        (Value::Double(x), Value::Int(y)) => x.partial_cmp(&(*y as f64)),
        (Value::UInt(x), Value::Double(y)) => (*x as f64).partial_cmp(y),
        (Value::Double(x), Value::UInt(y)) => x.partial_cmp(&(*y as f64)),
        _ => None,
    }
}

/// Shared implementation for math.greatest.
/// Works for any arity and for list arguments.
fn greatest_impl(args: &[Value]) -> Value {
    // If single arg is a list, operate on list elements
    let values: &[Value] = if args.len() == 1 {
        if let Value::List(list) = &args[0] {
            list
        } else {
            // Single non-list value: identity
            return args[0].clone();
        }
    } else {
        args
    };

    if values.is_empty() {
        return Value::error(EvalError::invalid_argument(
            "math.greatest requires at least one argument",
        ));
    }

    // Check for error values
    for v in values {
        if let Value::Error(_) = v {
            return v.clone();
        }
    }

    let mut best = &values[0];
    for v in &values[1..] {
        match compare_numeric(v, best) {
            Some(Ordering::Greater) => best = v,
            None => {
                // NaN: if best is NaN, replace it; if v is NaN, keep best
                if let Value::Double(d) = best {
                    if d.is_nan() {
                        best = v;
                        continue;
                    }
                }
                if let Value::Double(d) = v {
                    if d.is_nan() {
                        continue;
                    }
                }
                return Value::error(EvalError::invalid_argument(
                    "math.greatest: incomparable types",
                ));
            }
            _ => {}
        }
    }
    best.clone()
}

/// Shared implementation for math.least.
/// Works for any arity and for list arguments.
fn least_impl(args: &[Value]) -> Value {
    // If single arg is a list, operate on list elements
    let values: &[Value] = if args.len() == 1 {
        if let Value::List(list) = &args[0] {
            list
        } else {
            // Single non-list value: identity
            return args[0].clone();
        }
    } else {
        args
    };

    if values.is_empty() {
        return Value::error(EvalError::invalid_argument(
            "math.least requires at least one argument",
        ));
    }

    // Check for error values
    for v in values {
        if let Value::Error(_) = v {
            return v.clone();
        }
    }

    let mut best = &values[0];
    for v in &values[1..] {
        match compare_numeric(v, best) {
            Some(Ordering::Less) => best = v,
            None => {
                // NaN: if best is NaN, replace it; if v is NaN, keep best
                if let Value::Double(d) = best {
                    if d.is_nan() {
                        best = v;
                        continue;
                    }
                }
                if let Value::Double(d) = v {
                    if d.is_nan() {
                        continue;
                    }
                }
                return Value::error(EvalError::invalid_argument(
                    "math.least: incomparable types",
                ));
            }
            _ => {}
        }
    }
    best.clone()
}

fn build_minmax_function(name: &str, is_greatest: bool) -> FunctionDecl {
    let base = name.replace('.', "_");
    let mut decl = FunctionDecl::new(name);

    let shared_impl: fn(&[Value]) -> Value = if is_greatest {
        greatest_impl
    } else {
        least_impl
    };

    // Unary (identity)
    for (suffix, cel_type) in [
        ("int", CelType::Int),
        ("uint", CelType::UInt),
        ("double", CelType::Double),
    ] {
        decl = decl.with_overload(
            OverloadDecl::function(
                format!("{}_{}", base, suffix),
                vec![cel_type.clone()],
                cel_type,
            )
            .with_impl(shared_impl),
        );
    }

    // Binary same-type
    for (suffix, cel_type) in [
        ("int", CelType::Int),
        ("uint", CelType::UInt),
        ("double", CelType::Double),
    ] {
        decl = decl.with_overload(
            OverloadDecl::function(
                format!("{}_{}_{}", base, suffix, suffix),
                vec![cel_type.clone(), cel_type.clone()],
                cel_type,
            )
            .with_impl(shared_impl),
        );
    }

    // Binary mixed-type -> Dyn
    let types = [
        ("int", CelType::Int),
        ("uint", CelType::UInt),
        ("double", CelType::Double),
    ];
    for (name1, type1) in &types {
        for (name2, type2) in &types {
            if name1 != name2 {
                decl = decl.with_overload(
                    OverloadDecl::function(
                        format!("{}_{}_{}", base, name1, name2),
                        vec![type1.clone(), type2.clone()],
                        CelType::Dyn,
                    )
                    .with_impl(shared_impl),
                );
            }
        }
    }

    // Ternary and higher arities (3-6 args) for common use cases
    for arity in 3..=6 {
        // All same type
        for (suffix, cel_type) in [
            ("int", CelType::Int),
            ("uint", CelType::UInt),
            ("double", CelType::Double),
        ] {
            decl = decl.with_overload(
                OverloadDecl::function(
                    format!("{}_{}{}", base, suffix, arity),
                    vec![cel_type.clone(); arity],
                    cel_type,
                )
                .with_impl(shared_impl),
            );
        }
        // Mixed -> Dyn (just one overload for mixed types)
        decl = decl.with_overload(
            OverloadDecl::function(
                format!("{}_dyn{}", base, arity),
                vec![CelType::Dyn; arity],
                CelType::Dyn,
            )
            .with_impl(shared_impl),
        );
    }

    // List overloads
    for (suffix, cel_type) in [
        ("int", CelType::Int),
        ("uint", CelType::UInt),
        ("double", CelType::Double),
    ] {
        decl = decl.with_overload(
            OverloadDecl::function(
                format!("{}_list_{}", base, suffix),
                vec![CelType::list(cel_type.clone())],
                cel_type,
            )
            .with_impl(shared_impl),
        );
    }
    decl = decl.with_overload(
        OverloadDecl::function(
            format!("{}_list_dyn", base),
            vec![CelType::list(CelType::Dyn)],
            CelType::Dyn,
        )
        .with_impl(shared_impl),
    );

    decl
}

fn add_bit_operations(funcs: &mut Vec<FunctionDecl>) {
    // math.bitAnd
    funcs.push(
        FunctionDecl::new("math.bitAnd")
            .with_overload(
                OverloadDecl::function(
                    "math_bitand_int_int",
                    vec![CelType::Int, CelType::Int],
                    CelType::Int,
                )
                .with_impl(|args| match (&args[0], &args[1]) {
                    (Value::Int(a), Value::Int(b)) => Value::Int(a & b),
                    _ => Value::error(EvalError::invalid_argument("expected int, int")),
                }),
            )
            .with_overload(
                OverloadDecl::function(
                    "math_bitand_uint_uint",
                    vec![CelType::UInt, CelType::UInt],
                    CelType::UInt,
                )
                .with_impl(|args| match (&args[0], &args[1]) {
                    (Value::UInt(a), Value::UInt(b)) => Value::UInt(a & b),
                    _ => Value::error(EvalError::invalid_argument("expected uint, uint")),
                }),
            ),
    );

    // math.bitOr
    funcs.push(
        FunctionDecl::new("math.bitOr")
            .with_overload(
                OverloadDecl::function(
                    "math_bitor_int_int",
                    vec![CelType::Int, CelType::Int],
                    CelType::Int,
                )
                .with_impl(|args| match (&args[0], &args[1]) {
                    (Value::Int(a), Value::Int(b)) => Value::Int(a | b),
                    _ => Value::error(EvalError::invalid_argument("expected int, int")),
                }),
            )
            .with_overload(
                OverloadDecl::function(
                    "math_bitor_uint_uint",
                    vec![CelType::UInt, CelType::UInt],
                    CelType::UInt,
                )
                .with_impl(|args| match (&args[0], &args[1]) {
                    (Value::UInt(a), Value::UInt(b)) => Value::UInt(a | b),
                    _ => Value::error(EvalError::invalid_argument("expected uint, uint")),
                }),
            ),
    );

    // math.bitXor
    funcs.push(
        FunctionDecl::new("math.bitXor")
            .with_overload(
                OverloadDecl::function(
                    "math_bitxor_int_int",
                    vec![CelType::Int, CelType::Int],
                    CelType::Int,
                )
                .with_impl(|args| match (&args[0], &args[1]) {
                    (Value::Int(a), Value::Int(b)) => Value::Int(a ^ b),
                    _ => Value::error(EvalError::invalid_argument("expected int, int")),
                }),
            )
            .with_overload(
                OverloadDecl::function(
                    "math_bitxor_uint_uint",
                    vec![CelType::UInt, CelType::UInt],
                    CelType::UInt,
                )
                .with_impl(|args| match (&args[0], &args[1]) {
                    (Value::UInt(a), Value::UInt(b)) => Value::UInt(a ^ b),
                    _ => Value::error(EvalError::invalid_argument("expected uint, uint")),
                }),
            ),
    );

    // math.bitNot (unary)
    funcs.push(
        FunctionDecl::new("math.bitNot")
            .with_overload(
                OverloadDecl::function("math_bitnot_int", vec![CelType::Int], CelType::Int)
                    .with_impl(|args| match &args[0] {
                        Value::Int(a) => Value::Int(!a),
                        _ => Value::error(EvalError::invalid_argument("expected int")),
                    }),
            )
            .with_overload(
                OverloadDecl::function("math_bitnot_uint", vec![CelType::UInt], CelType::UInt)
                    .with_impl(|args| match &args[0] {
                        Value::UInt(a) => Value::UInt(!a),
                        _ => Value::error(EvalError::invalid_argument("expected uint")),
                    }),
            ),
    );

    // math.bitShiftLeft
    funcs.push(
        FunctionDecl::new("math.bitShiftLeft")
            .with_overload(
                OverloadDecl::function(
                    "math_bitshiftleft_int_int",
                    vec![CelType::Int, CelType::Int],
                    CelType::Int,
                )
                .with_impl(|args| match (&args[0], &args[1]) {
                    (Value::Int(a), Value::Int(b)) => {
                        if *b < 0 {
                            return Value::error(EvalError::invalid_argument(
                                "math.bitShiftLeft: negative shift amount",
                            ));
                        }
                        if *b >= 64 {
                            return Value::Int(0);
                        }
                        Value::Int((*a as u64).wrapping_shl(*b as u32) as i64)
                    }
                    (Value::UInt(a), Value::Int(b)) => {
                        if *b < 0 {
                            return Value::error(EvalError::invalid_argument(
                                "math.bitShiftLeft: negative shift amount",
                            ));
                        }
                        if *b >= 64 {
                            return Value::UInt(0);
                        }
                        Value::UInt(a.wrapping_shl(*b as u32))
                    }
                    _ => Value::error(EvalError::invalid_argument("expected int/uint, int")),
                }),
            )
            .with_overload(
                OverloadDecl::function(
                    "math_bitshiftleft_uint_int",
                    vec![CelType::UInt, CelType::Int],
                    CelType::UInt,
                )
                .with_impl(|args| match (&args[0], &args[1]) {
                    (Value::UInt(a), Value::Int(b)) => {
                        if *b < 0 {
                            return Value::error(EvalError::invalid_argument(
                                "math.bitShiftLeft: negative shift amount",
                            ));
                        }
                        if *b >= 64 {
                            return Value::UInt(0);
                        }
                        Value::UInt(a.wrapping_shl(*b as u32))
                    }
                    _ => Value::error(EvalError::invalid_argument("expected uint, int")),
                }),
            ),
    );

    // math.bitShiftRight
    funcs.push(
        FunctionDecl::new("math.bitShiftRight")
            .with_overload(
                OverloadDecl::function(
                    "math_bitshiftright_int_int",
                    vec![CelType::Int, CelType::Int],
                    CelType::Int,
                )
                .with_impl(|args| match (&args[0], &args[1]) {
                    (Value::Int(a), Value::Int(b)) => {
                        if *b < 0 {
                            return Value::error(EvalError::invalid_argument(
                                "math.bitShiftRight: negative shift amount",
                            ));
                        }
                        if *b >= 64 {
                            return Value::Int(0);
                        }
                        // Logical (unsigned) right shift per CEL spec
                        Value::Int((*a as u64).wrapping_shr(*b as u32) as i64)
                    }
                    (Value::UInt(a), Value::Int(b)) => {
                        if *b < 0 {
                            return Value::error(EvalError::invalid_argument(
                                "math.bitShiftRight: negative shift amount",
                            ));
                        }
                        if *b >= 64 {
                            return Value::UInt(0);
                        }
                        Value::UInt(a.wrapping_shr(*b as u32))
                    }
                    _ => Value::error(EvalError::invalid_argument("expected int/uint, int")),
                }),
            )
            .with_overload(
                OverloadDecl::function(
                    "math_bitshiftright_uint_int",
                    vec![CelType::UInt, CelType::Int],
                    CelType::UInt,
                )
                .with_impl(|args| match (&args[0], &args[1]) {
                    (Value::UInt(a), Value::Int(b)) => {
                        if *b < 0 {
                            return Value::error(EvalError::invalid_argument(
                                "math.bitShiftRight: negative shift amount",
                            ));
                        }
                        if *b >= 64 {
                            return Value::UInt(0);
                        }
                        Value::UInt(a.wrapping_shr(*b as u32))
                    }
                    _ => Value::error(EvalError::invalid_argument("expected uint, int")),
                }),
            ),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_math_extension_has_functions() {
        let funcs = math_extension();
        // Should have: greatest, least, ceil, floor, round, trunc, abs, sign,
        // isNaN, isInf, isFinite, bitAnd, bitOr, bitXor, bitNot, bitShiftLeft, bitShiftRight
        assert!(funcs.len() >= 17);
    }

    #[test]
    fn test_math_greatest_overloads() {
        let funcs = math_extension();
        let greatest = funcs.iter().find(|f| f.name == "math.greatest").unwrap();

        // Check unary overloads exist
        assert!(greatest
            .overloads
            .iter()
            .any(|o| o.id == "math_greatest_int" && o.params.len() == 1));
        assert!(greatest
            .overloads
            .iter()
            .any(|o| o.id == "math_greatest_double" && o.params.len() == 1));

        // Check binary same-type overloads
        assert!(greatest
            .overloads
            .iter()
            .any(|o| o.id == "math_greatest_int_int" && o.params.len() == 2));

        // Check ternary overloads
        assert!(greatest
            .overloads
            .iter()
            .any(|o| o.id == "math_greatest_int3" && o.params.len() == 3));

        // Check list overloads
        assert!(greatest
            .overloads
            .iter()
            .any(|o| o.id == "math_greatest_list_int"));
    }

    #[test]
    fn test_math_abs_overloads() {
        let funcs = math_extension();
        let abs = funcs.iter().find(|f| f.name == "math.abs").unwrap();
        assert_eq!(abs.overloads.len(), 3); // int, uint, double
    }

    #[test]
    fn test_bit_operations() {
        let funcs = math_extension();

        let bit_and = funcs.iter().find(|f| f.name == "math.bitAnd").unwrap();
        assert_eq!(bit_and.overloads.len(), 2); // int, uint

        let bit_not = funcs.iter().find(|f| f.name == "math.bitNot").unwrap();
        assert_eq!(bit_not.overloads.len(), 2); // int, uint
    }

    #[test]
    fn test_all_functions_are_standalone() {
        let funcs = math_extension();
        for func in &funcs {
            for overload in &func.overloads {
                assert!(
                    !overload.is_member,
                    "Expected {} to be standalone, but it's a member function",
                    overload.id
                );
            }
        }
    }

    #[test]
    fn test_ceil_impl() {
        let result = (math_extension()
            .iter()
            .find(|f| f.name == "math.ceil")
            .unwrap()
            .overloads[0]
            .implementation
            .as_ref()
            .unwrap())(&[Value::Double(1.5)]);
        assert_eq!(result, Value::Double(2.0));
    }

    #[test]
    fn test_floor_impl() {
        let result = (math_extension()
            .iter()
            .find(|f| f.name == "math.floor")
            .unwrap()
            .overloads[0]
            .implementation
            .as_ref()
            .unwrap())(&[Value::Double(1.5)]);
        assert_eq!(result, Value::Double(1.0));
    }

    #[test]
    fn test_round_half_away_from_zero() {
        let round_fn = math_extension()
            .iter()
            .find(|f| f.name == "math.round")
            .unwrap()
            .overloads[0]
            .implementation
            .clone()
            .unwrap();

        assert_eq!(round_fn(&[Value::Double(2.5)]), Value::Double(3.0));
        assert_eq!(round_fn(&[Value::Double(-2.5)]), Value::Double(-3.0));
        assert_eq!(round_fn(&[Value::Double(1.4)]), Value::Double(1.0));
        assert_eq!(round_fn(&[Value::Double(-1.4)]), Value::Double(-1.0));
    }

    #[test]
    fn test_abs_overflow() {
        let abs_fn = math_extension()
            .iter()
            .find(|f| f.name == "math.abs")
            .unwrap()
            .overloads
            .iter()
            .find(|o| o.id == "math_abs_int")
            .unwrap()
            .implementation
            .clone()
            .unwrap();

        // Normal case
        assert_eq!(abs_fn(&[Value::Int(-5)]), Value::Int(5));
        // Overflow case
        assert!(matches!(abs_fn(&[Value::Int(i64::MIN)]), Value::Error(_)));
    }

    #[test]
    fn test_greatest_basic() {
        assert_eq!(
            greatest_impl(&[Value::Int(1), Value::Int(3), Value::Int(2)]),
            Value::Int(3)
        );
        assert_eq!(
            greatest_impl(&[Value::Double(1.5), Value::Double(2.5)]),
            Value::Double(2.5)
        );
    }

    #[test]
    fn test_least_basic() {
        assert_eq!(
            least_impl(&[Value::Int(1), Value::Int(3), Value::Int(2)]),
            Value::Int(1)
        );
        assert_eq!(
            least_impl(&[Value::UInt(5), Value::UInt(2)]),
            Value::UInt(2)
        );
    }

    #[test]
    fn test_greatest_mixed_types() {
        assert_eq!(
            greatest_impl(&[Value::Int(1), Value::UInt(5)]),
            Value::UInt(5)
        );
        assert_eq!(
            greatest_impl(&[Value::Int(-1), Value::UInt(0)]),
            Value::UInt(0)
        );
    }

    #[test]
    fn test_greatest_empty_list() {
        use std::sync::Arc;
        let empty: Arc<[Value]> = Arc::from(vec![]);
        assert!(matches!(
            greatest_impl(&[Value::List(empty)]),
            Value::Error(_)
        ));
    }

    #[test]
    fn test_bitshift_negative() {
        let shift_fn = math_extension()
            .iter()
            .find(|f| f.name == "math.bitShiftLeft")
            .unwrap()
            .overloads
            .iter()
            .find(|o| o.id == "math_bitshiftleft_int_int")
            .unwrap()
            .implementation
            .clone()
            .unwrap();

        assert!(matches!(
            shift_fn(&[Value::Int(1), Value::Int(-1)]),
            Value::Error(_)
        ));
    }
}
