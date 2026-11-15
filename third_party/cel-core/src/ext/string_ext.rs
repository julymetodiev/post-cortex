//! String extension library for CEL.
//!
//! This module provides additional string manipulation functions beyond the
//! CEL standard library, matching the cel-go strings extension.
//!
//! # Functions
//!
//! - `charAt(index)` - Returns character at index as a string
//! - `indexOf(substring)` / `indexOf(substring, offset)` - Find first occurrence
//! - `lastIndexOf(substring)` / `lastIndexOf(substring, offset)` - Find last occurrence
//! - `lowerAscii()` - Convert ASCII characters to lowercase
//! - `upperAscii()` - Convert ASCII characters to uppercase
//! - `replace(old, new)` / `replace(old, new, count)` - Replace occurrences
//! - `split(separator)` / `split(separator, limit)` - Split string into list
//! - `substring(start)` / `substring(start, end)` - Extract substring
//! - `trim()` - Remove leading/trailing whitespace
//! - `reverse()` - Reverse the string (Unicode-aware)
//! - `format(args)` - Format string with arguments
//! - `join()` / `join(separator)` - Join list of strings (method on list<string>)
//! - `strings.quote(string)` - Quote a string with escapes

use std::sync::Arc;

use crate::eval::time::{format_duration, format_timestamp};
use crate::eval::{EvalError, Value};
use crate::types::{CelType, FunctionDecl, OverloadDecl};

// ==================== Unicode Helpers ====================

fn codepoint_len(s: &str) -> usize {
    s.chars().count()
}

fn codepoint_to_byte_offset(s: &str, cp_index: usize) -> Option<usize> {
    if cp_index == 0 {
        return Some(0);
    }
    s.char_indices()
        .nth(cp_index)
        .map(|(byte_offset, _)| byte_offset)
        .or_else(|| {
            // cp_index == number of chars means "end of string"
            if cp_index == codepoint_len(s) {
                Some(s.len())
            } else {
                None
            }
        })
}

// ==================== Format Helpers ====================

fn value_type_name(v: &Value) -> &str {
    match v {
        Value::Null => "null_type",
        Value::Bool(_) => "bool",
        Value::Int(_) => "int",
        Value::UInt(_) => "uint",
        Value::Double(_) => "double",
        Value::String(_) => "string",
        Value::Bytes(_) => "bytes",
        Value::List(_) => "list",
        Value::Map(_) => "map",
        Value::Timestamp(_) => "google.protobuf.Timestamp",
        Value::Duration(_) => "google.protobuf.Duration",
        Value::Type(_) => "type",
        Value::Optional(_) => "optional_type",
        Value::Message(m) => m.type_name(),
        Value::Enum(e) => &e.type_name,
        Value::Error(_) => "error",
    }
}

enum FormatClause {
    Literal(String),
    Verb {
        verb: char,
        precision: Option<usize>,
    },
}

fn parse_format_string(fmt: &str) -> Result<Vec<FormatClause>, String> {
    let mut clauses = Vec::new();
    let mut chars = fmt.chars().peekable();
    let mut literal = String::new();

    while let Some(c) = chars.next() {
        if c == '%' {
            match chars.peek() {
                Some('%') => {
                    chars.next();
                    literal.push('%');
                }
                Some(_) => {
                    if !literal.is_empty() {
                        clauses.push(FormatClause::Literal(std::mem::take(&mut literal)));
                    }
                    // Parse optional precision: .N
                    let precision = if chars.peek() == Some(&'.') {
                        chars.next(); // consume '.'
                        let mut num_str = String::new();
                        while let Some(&d) = chars.peek() {
                            if d.is_ascii_digit() {
                                num_str.push(d);
                                chars.next();
                            } else {
                                break;
                            }
                        }
                        Some(
                            num_str
                                .parse::<usize>()
                                .map_err(|_| "invalid precision".to_string())?,
                        )
                    } else {
                        None
                    };
                    // Next char is the verb
                    let verb = chars.next().ok_or("unexpected end of format string")?;
                    match verb {
                        's' | 'd' | 'f' | 'e' | 'b' | 'o' | 'x' | 'X' => {
                            clauses.push(FormatClause::Verb { verb, precision });
                        }
                        other => {
                            return Err(format!(
                                "could not parse formatting clause: unrecognized formatting clause {:?}",
                                other.to_string()
                            ));
                        }
                    }
                }
                None => {
                    return Err("unexpected end of format string after %".to_string());
                }
            }
        } else {
            literal.push(c);
        }
    }

    if !literal.is_empty() {
        clauses.push(FormatClause::Literal(literal));
    }

    Ok(clauses)
}

fn format_value_as_string(v: &Value) -> Result<String, String> {
    match v {
        Value::Null => Ok("null".to_string()),
        Value::Bool(b) => Ok(b.to_string()),
        Value::Int(i) => Ok(i.to_string()),
        Value::UInt(u) => Ok(u.to_string()),
        Value::Double(d) => {
            if d.is_nan() {
                Ok("NaN".to_string())
            } else if d.is_infinite() {
                if d.is_sign_positive() {
                    Ok("Infinity".to_string())
                } else {
                    Ok("-Infinity".to_string())
                }
            } else {
                // Use default float formatting (no trailing .0 normalization for %s)
                let s = format!("{}", d);
                Ok(s)
            }
        }
        Value::String(s) => Ok(s.to_string()),
        Value::Bytes(b) => Ok(String::from_utf8_lossy(b).to_string()),
        Value::Timestamp(t) => Ok(format_timestamp(t)),
        Value::Duration(d) => Ok(format_duration(d)),
        Value::Type(t) => Ok(t.name.to_string()),
        Value::List(items) => {
            let mut parts = Vec::new();
            for item in items.iter() {
                parts.push(format_value_as_string(item)?);
            }
            Ok(format!("[{}]", parts.join(", ")))
        }
        Value::Map(m) => {
            // Collect entries and sort by CEL ordering: int/uint (numeric), string (alpha), bool
            let mut entries: Vec<_> = m.iter().collect();
            entries.sort_by(|(a, _), (b, _)| {
                use crate::eval::MapKey;
                fn key_order(k: &MapKey) -> u8 {
                    match k {
                        MapKey::Int(_) | MapKey::UInt(_) => 0,
                        MapKey::String(_) => 1,
                        MapKey::Bool(_) => 2,
                    }
                }
                let ord = key_order(a).cmp(&key_order(b));
                if ord != std::cmp::Ordering::Equal {
                    return ord;
                }
                // Within same category, use natural ordering
                a.cmp(b)
            });
            let mut parts = Vec::new();
            for (k, v) in entries {
                let key_str = match k {
                    crate::eval::MapKey::Bool(b) => b.to_string(),
                    crate::eval::MapKey::Int(i) => i.to_string(),
                    crate::eval::MapKey::UInt(u) => u.to_string(),
                    crate::eval::MapKey::String(s) => s.to_string(),
                };
                let val_str = format_value_as_string(v)?;
                parts.push(format!("{}: {}", key_str, val_str));
            }
            Ok(format!("{{{}}}", parts.join(", ")))
        }
        _ => Err(format!(
            "string clause can only be used on strings, bools, bytes, ints, doubles, maps, lists, types, durations, and timestamps, was given {}",
            value_type_name(v)
        )),
    }
}

fn format_value(v: &Value, verb: char, precision: Option<usize>) -> Result<String, String> {
    match verb {
        's' => format_value_as_string(v),
        'd' => {
            // Decimal: integers only (+ doubles that are NaN/Inf)
            match v {
                Value::Int(i) => Ok(i.to_string()),
                Value::UInt(u) => Ok(u.to_string()),
                Value::Double(d) if d.is_nan() => Ok("NaN".to_string()),
                Value::Double(d) if d.is_infinite() => {
                    if d.is_sign_positive() {
                        Ok("Infinity".to_string())
                    } else {
                        Ok("-Infinity".to_string())
                    }
                }
                _ => Err(format!(
                    "decimal clause can only be used on integers, was given {}",
                    value_type_name(v)
                )),
            }
        }
        'f' => {
            let prec = precision.unwrap_or(6);
            let d = match v {
                Value::Double(d) => *d,
                Value::Int(i) => *i as f64,
                Value::UInt(u) => *u as f64,
                _ => {
                    return Err(format!(
                        "fixed-point clause can only be used on doubles, was given {}",
                        value_type_name(v)
                    ))
                }
            };
            if d.is_nan() {
                return Ok("NaN".to_string());
            }
            if d.is_infinite() {
                return if d.is_sign_positive() {
                    Ok("Infinity".to_string())
                } else {
                    Ok("-Infinity".to_string())
                };
            }
            Ok(format!("{:.prec$}", d, prec = prec))
        }
        'e' => {
            let prec = precision.unwrap_or(6);
            let d = match v {
                Value::Double(d) => *d,
                Value::Int(i) => *i as f64,
                Value::UInt(u) => *u as f64,
                _ => {
                    return Err(format!(
                        "scientific clause can only be used on doubles, was given {}",
                        value_type_name(v)
                    ))
                }
            };
            if d.is_nan() {
                return Ok("NaN".to_string());
            }
            if d.is_infinite() {
                return if d.is_sign_positive() {
                    Ok("Infinity".to_string())
                } else {
                    Ok("-Infinity".to_string())
                };
            }
            // Rust doesn't have %e built-in, implement manually
            Ok(format_scientific(d, prec))
        }
        'b' => match v {
            Value::Int(i) => Ok(format!("{:b}", i)),
            Value::UInt(u) => Ok(format!("{:b}", u)),
            Value::Bool(b) => Ok(if *b { "1".to_string() } else { "0".to_string() }),
            _ => Err(format!(
                "only integers and bools can be formatted as binary, was given {}",
                value_type_name(v)
            )),
        },
        'o' => match v {
            Value::Int(i) => Ok(format!("{:o}", i)),
            Value::UInt(u) => Ok(format!("{:o}", u)),
            _ => Err(format!(
                "octal clause can only be used on integers, was given {}",
                value_type_name(v)
            )),
        },
        'x' => match v {
            Value::Int(i) => Ok(format!("{:x}", i)),
            Value::UInt(u) => Ok(format!("{:x}", u)),
            Value::String(s) => Ok(s.bytes().map(|b| format!("{:02x}", b)).collect()),
            Value::Bytes(b) => Ok(b.iter().map(|b| format!("{:02x}", b)).collect()),
            _ => Err(format!(
                "only integers, byte buffers, and strings can be formatted as hex, was given {}",
                value_type_name(v)
            )),
        },
        'X' => match v {
            Value::Int(i) => Ok(format!("{:X}", i)),
            Value::UInt(u) => Ok(format!("{:X}", u)),
            Value::String(s) => Ok(s.bytes().map(|b| format!("{:02X}", b)).collect()),
            Value::Bytes(b) => Ok(b.iter().map(|b| format!("{:02X}", b)).collect()),
            _ => Err(format!(
                "only integers, byte buffers, and strings can be formatted as hex, was given {}",
                value_type_name(v)
            )),
        },
        _ => Err(format!("unrecognized verb: {}", verb)),
    }
}

fn format_scientific(d: f64, precision: usize) -> String {
    if d == 0.0 {
        let sign = if d.is_sign_negative() { "-" } else { "" };
        if precision == 0 {
            return format!("{}0e+00", sign);
        }
        return format!("{}0.{:0>width$}e+00", sign, "", width = precision);
    }
    let abs = d.abs();
    let exp = abs.log10().floor() as i32;
    let mantissa = d / 10f64.powi(exp);
    let exp_sign = if exp >= 0 { '+' } else { '-' };
    let exp_abs = exp.unsigned_abs();
    if precision == 0 {
        format!("{:.0}e{}{:02}", mantissa, exp_sign, exp_abs)
    } else {
        format!(
            "{:.prec$}e{}{:02}",
            mantissa,
            exp_sign,
            exp_abs,
            prec = precision
        )
    }
}

// ==================== Extension Declaration ====================

/// Returns the string extension library function declarations.
pub fn string_extension() -> Vec<FunctionDecl> {
    vec![
        // charAt: (string).charAt(int) -> string
        FunctionDecl::new("charAt").with_overload(
            OverloadDecl::method(
                "string_char_at_int",
                vec![CelType::String, CelType::Int],
                CelType::String,
            )
            .with_impl(|args| {
                let s = match &args[0] {
                    Value::String(s) => s,
                    _ => return Value::error(EvalError::invalid_argument("expected string")),
                };
                let idx = match &args[1] {
                    Value::Int(i) => *i,
                    _ => return Value::error(EvalError::invalid_argument("expected int")),
                };
                let len = codepoint_len(s) as i64;
                if idx == len {
                    return Value::String(Arc::from(""));
                }
                if idx < 0 || idx > len {
                    return Value::error(EvalError::range_error(format!(
                        "index out of range: {}",
                        idx
                    )));
                }
                match s.chars().nth(idx as usize) {
                    Some(c) => Value::String(Arc::from(c.to_string())),
                    None => Value::String(Arc::from("")),
                }
            }),
        ),
        // indexOf: two overloads
        FunctionDecl::new("indexOf")
            .with_overload(
                OverloadDecl::method(
                    "string_index_of_string",
                    vec![CelType::String, CelType::String],
                    CelType::Int,
                )
                .with_impl(|args| {
                    let s = match &args[0] {
                        Value::String(s) => s,
                        _ => return Value::error(EvalError::invalid_argument("expected string")),
                    };
                    let substr = match &args[1] {
                        Value::String(s) => s,
                        _ => return Value::error(EvalError::invalid_argument("expected string")),
                    };
                    if substr.is_empty() {
                        return Value::Int(0);
                    }
                    match s.find(substr.as_ref()) {
                        Some(byte_offset) => {
                            let cp_index = s[..byte_offset].chars().count() as i64;
                            Value::Int(cp_index)
                        }
                        None => Value::Int(-1),
                    }
                }),
            )
            .with_overload(
                OverloadDecl::method(
                    "string_index_of_string_int",
                    vec![CelType::String, CelType::String, CelType::Int],
                    CelType::Int,
                )
                .with_impl(|args| {
                    let s = match &args[0] {
                        Value::String(s) => s,
                        _ => return Value::error(EvalError::invalid_argument("expected string")),
                    };
                    let substr = match &args[1] {
                        Value::String(s) => s,
                        _ => return Value::error(EvalError::invalid_argument("expected string")),
                    };
                    let offset = match &args[2] {
                        Value::Int(i) => *i,
                        _ => return Value::error(EvalError::invalid_argument("expected int")),
                    };
                    let len = codepoint_len(s) as i64;
                    if offset < 0 || offset > len {
                        return Value::error(EvalError::range_error(format!(
                            "index out of range: {}",
                            offset
                        )));
                    }
                    if substr.is_empty() {
                        return Value::Int(offset);
                    }
                    let byte_start = match codepoint_to_byte_offset(s, offset as usize) {
                        Some(b) => b,
                        None => return Value::Int(-1),
                    };
                    match s[byte_start..].find(substr.as_ref()) {
                        Some(byte_offset) => {
                            let cp_index = s[..byte_start + byte_offset].chars().count() as i64;
                            Value::Int(cp_index)
                        }
                        None => Value::Int(-1),
                    }
                }),
            ),
        // lastIndexOf
        FunctionDecl::new("lastIndexOf")
            .with_overload(
                OverloadDecl::method(
                    "string_last_index_of_string",
                    vec![CelType::String, CelType::String],
                    CelType::Int,
                )
                .with_impl(|args| {
                    let s = match &args[0] {
                        Value::String(s) => s,
                        _ => return Value::error(EvalError::invalid_argument("expected string")),
                    };
                    let substr = match &args[1] {
                        Value::String(s) => s,
                        _ => return Value::error(EvalError::invalid_argument("expected string")),
                    };
                    if substr.is_empty() {
                        return Value::Int(codepoint_len(s) as i64);
                    }
                    match s.rfind(substr.as_ref()) {
                        Some(byte_offset) => {
                            let cp_index = s[..byte_offset].chars().count() as i64;
                            Value::Int(cp_index)
                        }
                        None => Value::Int(-1),
                    }
                }),
            )
            .with_overload(
                OverloadDecl::method(
                    "string_last_index_of_string_int",
                    vec![CelType::String, CelType::String, CelType::Int],
                    CelType::Int,
                )
                .with_impl(|args| {
                    let s = match &args[0] {
                        Value::String(s) => s,
                        _ => return Value::error(EvalError::invalid_argument("expected string")),
                    };
                    let substr = match &args[1] {
                        Value::String(s) => s,
                        _ => return Value::error(EvalError::invalid_argument("expected string")),
                    };
                    let offset = match &args[2] {
                        Value::Int(i) => *i,
                        _ => return Value::error(EvalError::invalid_argument("expected int")),
                    };
                    let len = codepoint_len(s) as i64;
                    if offset < 0 || offset > len {
                        return Value::error(EvalError::range_error(format!(
                            "index out of range: {}",
                            offset
                        )));
                    }
                    if substr.is_empty() {
                        return Value::Int(offset);
                    }
                    // Search only in s[0..end_byte] where end_byte is the byte offset of
                    // offset + len(substr) code points (so we can find matches starting at offset)
                    let substr_cp_len = codepoint_len(substr);
                    let search_end_cp = (offset as usize) + substr_cp_len;
                    let search_end_byte = if search_end_cp >= codepoint_len(s) {
                        s.len()
                    } else {
                        codepoint_to_byte_offset(s, search_end_cp).unwrap_or(s.len())
                    };
                    let search_slice = &s[..search_end_byte];
                    match search_slice.rfind(substr.as_ref()) {
                        Some(byte_offset) => {
                            let cp_index = s[..byte_offset].chars().count() as i64;
                            Value::Int(cp_index)
                        }
                        None => Value::Int(-1),
                    }
                }),
            ),
        // lowerAscii
        FunctionDecl::new("lowerAscii").with_overload(
            OverloadDecl::method("string_lower_ascii", vec![CelType::String], CelType::String)
                .with_impl(|args| {
                    let s = match &args[0] {
                        Value::String(s) => s,
                        _ => return Value::error(EvalError::invalid_argument("expected string")),
                    };
                    let result: String = s
                        .chars()
                        .map(|c| {
                            if c.is_ascii() {
                                c.to_ascii_lowercase()
                            } else {
                                c
                            }
                        })
                        .collect();
                    Value::String(Arc::from(result))
                }),
        ),
        // upperAscii
        FunctionDecl::new("upperAscii").with_overload(
            OverloadDecl::method("string_upper_ascii", vec![CelType::String], CelType::String)
                .with_impl(|args| {
                    let s = match &args[0] {
                        Value::String(s) => s,
                        _ => return Value::error(EvalError::invalid_argument("expected string")),
                    };
                    let result: String = s
                        .chars()
                        .map(|c| {
                            if c.is_ascii() {
                                c.to_ascii_uppercase()
                            } else {
                                c
                            }
                        })
                        .collect();
                    Value::String(Arc::from(result))
                }),
        ),
        // replace
        FunctionDecl::new("replace")
            .with_overload(
                OverloadDecl::method(
                    "string_replace_string_string",
                    vec![CelType::String, CelType::String, CelType::String],
                    CelType::String,
                )
                .with_impl(|args| {
                    let s = match &args[0] {
                        Value::String(s) => s,
                        _ => return Value::error(EvalError::invalid_argument("expected string")),
                    };
                    let old = match &args[1] {
                        Value::String(s) => s,
                        _ => return Value::error(EvalError::invalid_argument("expected string")),
                    };
                    let new = match &args[2] {
                        Value::String(s) => s,
                        _ => return Value::error(EvalError::invalid_argument("expected string")),
                    };
                    Value::String(Arc::from(s.replace(old.as_ref(), new.as_ref())))
                }),
            )
            .with_overload(
                OverloadDecl::method(
                    "string_replace_string_string_int",
                    vec![
                        CelType::String,
                        CelType::String,
                        CelType::String,
                        CelType::Int,
                    ],
                    CelType::String,
                )
                .with_impl(|args| {
                    let s = match &args[0] {
                        Value::String(s) => s,
                        _ => return Value::error(EvalError::invalid_argument("expected string")),
                    };
                    let old = match &args[1] {
                        Value::String(s) => s,
                        _ => return Value::error(EvalError::invalid_argument("expected string")),
                    };
                    let new = match &args[2] {
                        Value::String(s) => s,
                        _ => return Value::error(EvalError::invalid_argument("expected string")),
                    };
                    let limit = match &args[3] {
                        Value::Int(i) => *i,
                        _ => return Value::error(EvalError::invalid_argument("expected int")),
                    };
                    if limit == 0 {
                        return Value::String(Arc::clone(s));
                    }
                    if limit < 0 {
                        return Value::String(Arc::from(s.replace(old.as_ref(), new.as_ref())));
                    }
                    Value::String(Arc::from(s.replacen(
                        old.as_ref(),
                        new.as_ref(),
                        limit as usize,
                    )))
                }),
            ),
        // split
        FunctionDecl::new("split")
            .with_overload(
                OverloadDecl::method(
                    "string_split_string",
                    vec![CelType::String, CelType::String],
                    CelType::list(CelType::String),
                )
                .with_impl(|args| {
                    let s = match &args[0] {
                        Value::String(s) => s,
                        _ => return Value::error(EvalError::invalid_argument("expected string")),
                    };
                    let sep = match &args[1] {
                        Value::String(s) => s,
                        _ => return Value::error(EvalError::invalid_argument("expected string")),
                    };
                    let parts: Vec<Value> = s
                        .split(sep.as_ref())
                        .map(|p| Value::String(Arc::from(p)))
                        .collect();
                    Value::List(Arc::from(parts))
                }),
            )
            .with_overload(
                OverloadDecl::method(
                    "string_split_string_int",
                    vec![CelType::String, CelType::String, CelType::Int],
                    CelType::list(CelType::String),
                )
                .with_impl(|args| {
                    let s = match &args[0] {
                        Value::String(s) => s,
                        _ => return Value::error(EvalError::invalid_argument("expected string")),
                    };
                    let sep = match &args[1] {
                        Value::String(s) => s,
                        _ => return Value::error(EvalError::invalid_argument("expected string")),
                    };
                    let limit = match &args[2] {
                        Value::Int(i) => *i,
                        _ => return Value::error(EvalError::invalid_argument("expected int")),
                    };
                    if limit == 0 {
                        return Value::List(Arc::from(Vec::<Value>::new()));
                    }
                    if limit < 0 {
                        let parts: Vec<Value> = s
                            .split(sep.as_ref())
                            .map(|p| Value::String(Arc::from(p)))
                            .collect();
                        return Value::List(Arc::from(parts));
                    }
                    let parts: Vec<Value> = s
                        .splitn(limit as usize, sep.as_ref())
                        .map(|p| Value::String(Arc::from(p)))
                        .collect();
                    Value::List(Arc::from(parts))
                }),
            ),
        // substring
        FunctionDecl::new("substring")
            .with_overload(
                OverloadDecl::method(
                    "string_substring_int",
                    vec![CelType::String, CelType::Int],
                    CelType::String,
                )
                .with_impl(|args| {
                    let s = match &args[0] {
                        Value::String(s) => s,
                        _ => return Value::error(EvalError::invalid_argument("expected string")),
                    };
                    let start = match &args[1] {
                        Value::Int(i) => *i,
                        _ => return Value::error(EvalError::invalid_argument("expected int")),
                    };
                    let len = codepoint_len(s) as i64;
                    if start < 0 || start > len {
                        return Value::error(EvalError::range_error(format!(
                            "index out of range: {}",
                            start
                        )));
                    }
                    let byte_start = codepoint_to_byte_offset(s, start as usize).unwrap_or(s.len());
                    Value::String(Arc::from(&s[byte_start..]))
                }),
            )
            .with_overload(
                OverloadDecl::method(
                    "string_substring_int_int",
                    vec![CelType::String, CelType::Int, CelType::Int],
                    CelType::String,
                )
                .with_impl(|args| {
                    let s = match &args[0] {
                        Value::String(s) => s,
                        _ => return Value::error(EvalError::invalid_argument("expected string")),
                    };
                    let start = match &args[1] {
                        Value::Int(i) => *i,
                        _ => return Value::error(EvalError::invalid_argument("expected int")),
                    };
                    let end = match &args[2] {
                        Value::Int(i) => *i,
                        _ => return Value::error(EvalError::invalid_argument("expected int")),
                    };
                    let len = codepoint_len(s) as i64;
                    if start < 0 || start > len {
                        return Value::error(EvalError::range_error(format!(
                            "index out of range: {}",
                            start
                        )));
                    }
                    if end < 0 || end > len {
                        return Value::error(EvalError::range_error(format!(
                            "index out of range: {}",
                            end
                        )));
                    }
                    if start > end {
                        return Value::error(EvalError::range_error(format!(
                            "invalid substring range. start: {}, end: {}",
                            start, end
                        )));
                    }
                    let byte_start = codepoint_to_byte_offset(s, start as usize).unwrap_or(s.len());
                    let byte_end = codepoint_to_byte_offset(s, end as usize).unwrap_or(s.len());
                    Value::String(Arc::from(&s[byte_start..byte_end]))
                }),
            ),
        // trim
        FunctionDecl::new("trim").with_overload(
            OverloadDecl::method("string_trim", vec![CelType::String], CelType::String).with_impl(
                |args| {
                    let s = match &args[0] {
                        Value::String(s) => s,
                        _ => return Value::error(EvalError::invalid_argument("expected string")),
                    };
                    Value::String(Arc::from(s.trim()))
                },
            ),
        ),
        // reverse
        FunctionDecl::new("reverse").with_overload(
            OverloadDecl::method("string_reverse", vec![CelType::String], CelType::String)
                .with_impl(|args| {
                    let s = match &args[0] {
                        Value::String(s) => s,
                        _ => return Value::error(EvalError::invalid_argument("expected string")),
                    };
                    let reversed: String = s.chars().rev().collect();
                    Value::String(Arc::from(reversed))
                }),
        ),
        // format
        FunctionDecl::new("format").with_overload(
            OverloadDecl::method(
                "string_format",
                vec![CelType::String, CelType::list(CelType::Dyn)],
                CelType::String,
            )
            .with_impl(|args| {
                let fmt_str = match &args[0] {
                    Value::String(s) => s,
                    _ => return Value::error(EvalError::invalid_argument("expected string")),
                };
                let list = match &args[1] {
                    Value::List(l) => l,
                    _ => return Value::error(EvalError::invalid_argument("expected list")),
                };

                let clauses = match parse_format_string(fmt_str) {
                    Ok(c) => c,
                    Err(e) => return Value::error(EvalError::invalid_argument(e)),
                };

                let mut result = String::new();
                let mut arg_index = 0usize;

                for clause in &clauses {
                    match clause {
                        FormatClause::Literal(s) => result.push_str(s),
                        FormatClause::Verb { verb, precision } => {
                            if arg_index >= list.len() {
                                return Value::error(EvalError::invalid_argument(format!(
                                    "index {} out of range",
                                    arg_index
                                )));
                            }
                            match format_value(&list[arg_index], *verb, *precision) {
                                Ok(s) => result.push_str(&s),
                                Err(e) => {
                                    return Value::error(EvalError::invalid_argument(format!(
                                        "error during formatting: {}",
                                        e
                                    )))
                                }
                            }
                            arg_index += 1;
                        }
                    }
                }

                Value::String(Arc::from(result))
            }),
        ),
        // join - method on list<string>
        FunctionDecl::new("join")
            .with_overload(
                OverloadDecl::method(
                    "list_string_join",
                    vec![CelType::list(CelType::String)],
                    CelType::String,
                )
                .with_impl(|args| {
                    let list = match &args[0] {
                        Value::List(l) => l,
                        _ => return Value::error(EvalError::invalid_argument("expected list")),
                    };
                    let parts: Vec<&str> = list
                        .iter()
                        .map(|v| match v {
                            Value::String(s) => s.as_ref(),
                            _ => "",
                        })
                        .collect();
                    Value::String(Arc::from(parts.join("")))
                }),
            )
            .with_overload(
                OverloadDecl::method(
                    "list_string_join_string",
                    vec![CelType::list(CelType::String), CelType::String],
                    CelType::String,
                )
                .with_impl(|args| {
                    let list = match &args[0] {
                        Value::List(l) => l,
                        _ => return Value::error(EvalError::invalid_argument("expected list")),
                    };
                    let sep = match &args[1] {
                        Value::String(s) => s,
                        _ => return Value::error(EvalError::invalid_argument("expected string")),
                    };
                    let parts: Vec<&str> = list
                        .iter()
                        .map(|v| match v {
                            Value::String(s) => s.as_ref(),
                            _ => "",
                        })
                        .collect();
                    Value::String(Arc::from(parts.join(sep.as_ref())))
                }),
            ),
        // strings.quote - namespaced standalone function
        FunctionDecl::new("strings.quote").with_overload(
            OverloadDecl::function(
                "strings_quote_string",
                vec![CelType::String],
                CelType::String,
            )
            .with_impl(|args| {
                let s = match &args[0] {
                    Value::String(s) => s,
                    _ => return Value::error(EvalError::invalid_argument("expected string")),
                };
                let mut result = String::with_capacity(s.len() + 2);
                result.push('"');
                for c in s.chars() {
                    match c {
                        '\x07' => result.push_str("\\a"),
                        '\x08' => result.push_str("\\b"),
                        '\x0C' => result.push_str("\\f"),
                        '\n' => result.push_str("\\n"),
                        '\r' => result.push_str("\\r"),
                        '\t' => result.push_str("\\t"),
                        '\x0B' => result.push_str("\\v"),
                        '\\' => result.push_str("\\\\"),
                        '"' => result.push_str("\\\""),
                        _ => result.push(c),
                    }
                }
                result.push('"');
                Value::String(Arc::from(result))
            }),
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_string_extension_count() {
        let funcs = string_extension();
        // 13 functions defined
        assert_eq!(funcs.len(), 13);
    }

    #[test]
    fn test_char_at() {
        let funcs = string_extension();
        let char_at = funcs.iter().find(|f| f.name == "charAt").unwrap();
        assert_eq!(char_at.overloads.len(), 1);
        assert!(char_at.overloads[0].is_member);
    }

    #[test]
    fn test_index_of_overloads() {
        let funcs = string_extension();
        let index_of = funcs.iter().find(|f| f.name == "indexOf").unwrap();
        assert_eq!(index_of.overloads.len(), 2);
    }

    #[test]
    fn test_join_is_member_on_list() {
        let funcs = string_extension();
        let join = funcs.iter().find(|f| f.name == "join").unwrap();
        assert_eq!(join.overloads.len(), 2);
        for overload in &join.overloads {
            assert!(overload.is_member);
            assert_eq!(
                overload.receiver_type(),
                Some(&CelType::list(CelType::String))
            );
        }
    }

    #[test]
    fn test_strings_quote_is_standalone() {
        let funcs = string_extension();
        let quote = funcs.iter().find(|f| f.name == "strings.quote").unwrap();
        assert_eq!(quote.overloads.len(), 1);
        assert!(!quote.overloads[0].is_member);
    }

    // ==================== Runtime Implementation Tests ====================

    fn call_overload(
        funcs: &[FunctionDecl],
        name: &str,
        overload_idx: usize,
        args: &[Value],
    ) -> Value {
        let func = funcs.iter().find(|f| f.name == name).unwrap();
        let overload = &func.overloads[overload_idx];
        let imp = overload.implementation.as_ref().expect("no implementation");
        imp(args)
    }

    fn s(val: &str) -> Value {
        Value::String(Arc::from(val))
    }

    #[test]
    fn test_char_at_impl() {
        let funcs = string_extension();
        assert_eq!(
            call_overload(&funcs, "charAt", 0, &[s("tacocat"), Value::Int(3)]),
            s("o")
        );
        assert_eq!(
            call_overload(&funcs, "charAt", 0, &[s("tacocat"), Value::Int(7)]),
            s("")
        );
        // Unicode
        assert_eq!(
            call_overload(&funcs, "charAt", 0, &[s("©αT"), Value::Int(0)]),
            s("©")
        );
        assert_eq!(
            call_overload(&funcs, "charAt", 0, &[s("©αT"), Value::Int(1)]),
            s("α")
        );
    }

    #[test]
    fn test_lower_ascii_impl() {
        let funcs = string_extension();
        assert_eq!(
            call_overload(&funcs, "lowerAscii", 0, &[s("TacoCat")]),
            s("tacocat")
        );
        assert_eq!(
            call_overload(&funcs, "lowerAscii", 0, &[s("TacoCÆt")]),
            s("tacocÆt")
        );
    }

    #[test]
    fn test_upper_ascii_impl() {
        let funcs = string_extension();
        assert_eq!(
            call_overload(&funcs, "upperAscii", 0, &[s("tacoCat")]),
            s("TACOCAT")
        );
        assert_eq!(
            call_overload(&funcs, "upperAscii", 0, &[s("tacoCαt")]),
            s("TACOCαT")
        );
    }

    #[test]
    fn test_trim_impl() {
        let funcs = string_extension();
        assert_eq!(
            call_overload(&funcs, "trim", 0, &[s("  hello  ")]),
            s("hello")
        );
        assert_eq!(
            call_overload(&funcs, "trim", 0, &[s(" \t\n text \r ")]),
            s("text")
        );
    }

    #[test]
    fn test_reverse_impl() {
        let funcs = string_extension();
        assert_eq!(call_overload(&funcs, "reverse", 0, &[s("")]), s(""));
        assert_eq!(call_overload(&funcs, "reverse", 0, &[s("☺")]), s("☺"));
        assert_eq!(
            call_overload(&funcs, "reverse", 0, &[s("Ta©oCαt")]),
            s("tαCo©aT")
        );
    }

    #[test]
    fn test_index_of_impl() {
        let funcs = string_extension();
        // Basic
        assert_eq!(
            call_overload(&funcs, "indexOf", 0, &[s("tacocat"), s("")]),
            Value::Int(0)
        );
        assert_eq!(
            call_overload(&funcs, "indexOf", 0, &[s("tacocat"), s("ac")]),
            Value::Int(1)
        );
        assert_eq!(
            call_overload(&funcs, "indexOf", 0, &[s("tacocat"), s("none")]),
            Value::Int(-1)
        );
        // With offset
        assert_eq!(
            call_overload(&funcs, "indexOf", 1, &[s("tacocat"), s("a"), Value::Int(3)]),
            Value::Int(5)
        );
        // Unicode
        assert_eq!(
            call_overload(&funcs, "indexOf", 0, &[s("ta©o©αT"), s("©")]),
            Value::Int(2)
        );
        assert_eq!(
            call_overload(&funcs, "indexOf", 1, &[s("ta©o©αT"), s("©"), Value::Int(3)]),
            Value::Int(4)
        );
    }

    #[test]
    fn test_last_index_of_impl() {
        let funcs = string_extension();
        assert_eq!(
            call_overload(&funcs, "lastIndexOf", 0, &[s("tacocat"), s("")]),
            Value::Int(7)
        );
        assert_eq!(
            call_overload(&funcs, "lastIndexOf", 0, &[s("tacocat"), s("at")]),
            Value::Int(5)
        );
        assert_eq!(
            call_overload(&funcs, "lastIndexOf", 0, &[s("tacocat"), s("none")]),
            Value::Int(-1)
        );
        // With offset
        assert_eq!(
            call_overload(
                &funcs,
                "lastIndexOf",
                1,
                &[s("tacocat"), s("a"), Value::Int(3)]
            ),
            Value::Int(1)
        );
        // Unicode
        assert_eq!(
            call_overload(&funcs, "lastIndexOf", 0, &[s("ta©o©αT"), s("©")]),
            Value::Int(4)
        );
        assert_eq!(
            call_overload(
                &funcs,
                "lastIndexOf",
                1,
                &[s("ta©o©αT"), s("©"), Value::Int(3)]
            ),
            Value::Int(2)
        );
    }

    #[test]
    fn test_substring_impl() {
        let funcs = string_extension();
        assert_eq!(
            call_overload(&funcs, "substring", 0, &[s("tacocat"), Value::Int(4)]),
            s("cat")
        );
        assert_eq!(
            call_overload(&funcs, "substring", 0, &[s("tacocat"), Value::Int(7)]),
            s("")
        );
        assert_eq!(
            call_overload(
                &funcs,
                "substring",
                1,
                &[s("tacocat"), Value::Int(0), Value::Int(4)]
            ),
            s("taco")
        );
        assert_eq!(
            call_overload(
                &funcs,
                "substring",
                1,
                &[s("tacocat"), Value::Int(4), Value::Int(4)]
            ),
            s("")
        );
        // Unicode
        assert_eq!(
            call_overload(
                &funcs,
                "substring",
                1,
                &[s("ta©o©αT"), Value::Int(2), Value::Int(6)]
            ),
            s("©o©α")
        );
    }

    #[test]
    fn test_replace_impl() {
        let funcs = string_extension();
        assert_eq!(
            call_overload(
                &funcs,
                "replace",
                0,
                &[s("{0} days {0} hours"), s("{0}"), s("2")]
            ),
            s("2 days 2 hours")
        );
        // With limit
        assert_eq!(
            call_overload(
                &funcs,
                "replace",
                1,
                &[s("{0} days {0} hours"), s("{0}"), s("2"), Value::Int(1)]
            ),
            s("2 days {0} hours")
        );
        // Limit 0 = no change
        assert_eq!(
            call_overload(
                &funcs,
                "replace",
                1,
                &[s("{0} days {0} hours"), s("{0}"), s("2"), Value::Int(0)]
            ),
            s("{0} days {0} hours")
        );
    }

    #[test]
    fn test_split_impl() {
        let funcs = string_extension();
        let result = call_overload(&funcs, "split", 0, &[s("hello world"), s(" ")]);
        assert_eq!(result, Value::List(Arc::from(vec![s("hello"), s("world")])));
        // With limit 0
        let result = call_overload(
            &funcs,
            "split",
            1,
            &[s("hello world"), s(" "), Value::Int(0)],
        );
        assert_eq!(result, Value::List(Arc::from(Vec::<Value>::new())));
        // With limit 1
        let result = call_overload(
            &funcs,
            "split",
            1,
            &[s("hello world events!"), s(" "), Value::Int(1)],
        );
        assert_eq!(
            result,
            Value::List(Arc::from(vec![s("hello world events!")]))
        );
    }

    #[test]
    fn test_join_impl() {
        let funcs = string_extension();
        let list = Value::List(Arc::from(vec![s("x"), s("y")]));
        assert_eq!(
            call_overload(&funcs, "join", 0, std::slice::from_ref(&list)),
            s("xy")
        );
        assert_eq!(call_overload(&funcs, "join", 1, &[list, s("-")]), s("x-y"));
        // Empty list
        let empty = Value::List(Arc::from(Vec::<Value>::new()));
        assert_eq!(
            call_overload(&funcs, "join", 0, std::slice::from_ref(&empty)),
            s("")
        );
        assert_eq!(call_overload(&funcs, "join", 1, &[empty, s("-")]), s(""));
    }

    #[test]
    fn test_strings_quote_impl() {
        let funcs = string_extension();
        assert_eq!(
            call_overload(&funcs, "strings.quote", 0, &[s("verbatim")]),
            s("\"verbatim\"")
        );
        assert_eq!(
            call_overload(&funcs, "strings.quote", 0, &[s("first\nsecond")]),
            s("\"first\\nsecond\"")
        );
        assert_eq!(
            call_overload(&funcs, "strings.quote", 0, &[s("bell\x07")]),
            s("\"bell\\a\"")
        );
        assert_eq!(
            call_overload(&funcs, "strings.quote", 0, &[s("")]),
            s("\"\"")
        );
    }

    #[test]
    fn test_format_basic() {
        let funcs = string_extension();
        let fmt = |f: &str, args: Vec<Value>| {
            call_overload(&funcs, "format", 0, &[s(f), Value::List(Arc::from(args))])
        };

        assert_eq!(fmt("no substitution", vec![]), s("no substitution"));
        assert_eq!(fmt("%s", vec![s("filler")]), s("filler"));
        assert_eq!(fmt("%% and also %%", vec![]), s("% and also %"));
        assert_eq!(fmt("%%%s%%", vec![s("text")]), s("%text%"));
        assert_eq!(fmt("%d", vec![Value::Int(42)]), s("42"));
        assert_eq!(fmt("%d", vec![Value::UInt(64)]), s("64"));
        assert_eq!(fmt("%b", vec![Value::Int(5)]), s("101"));
        assert_eq!(fmt("%o", vec![Value::Int(11)]), s("13"));
        assert_eq!(fmt("%x", vec![Value::Int(30)]), s("1e"));
        assert_eq!(fmt("%X", vec![Value::Int(30)]), s("1E"));
    }

    #[test]
    #[allow(clippy::approx_constant)]
    fn test_format_float() {
        let funcs = string_extension();
        let fmt = |f: &str, args: Vec<Value>| {
            call_overload(&funcs, "format", 0, &[s(f), Value::List(Arc::from(args))])
        };

        assert_eq!(fmt("%.3f", vec![Value::Double(1.2345)]), s("1.234"));
        assert_eq!(fmt("%f", vec![Value::Double(2.71828)]), s("2.718280"));
        assert_eq!(fmt("%f", vec![Value::Int(2)]), s("2.000000"));
        assert_eq!(fmt("%f", vec![Value::Double(f64::NAN)]), s("NaN"));
        assert_eq!(fmt("%f", vec![Value::Double(f64::INFINITY)]), s("Infinity"));
    }

    #[test]
    #[allow(clippy::approx_constant)]
    fn test_format_scientific() {
        let funcs = string_extension();
        let fmt = |f: &str, args: Vec<Value>| {
            call_overload(&funcs, "format", 0, &[s(f), Value::List(Arc::from(args))])
        };

        assert_eq!(fmt("%e", vec![Value::Double(2.71828)]), s("2.718280e+00"));
        assert_eq!(
            fmt("%.6e", vec![Value::Double(1052.032911275)]),
            s("1.052033e+03")
        );
        assert_eq!(fmt("%e", vec![Value::Double(f64::NAN)]), s("NaN"));
        assert_eq!(fmt("%e", vec![Value::Double(f64::INFINITY)]), s("Infinity"));
    }

    #[test]
    fn test_format_hex_on_strings() {
        let funcs = string_extension();
        let fmt = |f: &str, args: Vec<Value>| {
            call_overload(&funcs, "format", 0, &[s(f), Value::List(Arc::from(args))])
        };

        assert_eq!(
            fmt("%x", vec![s("Hello world!")]),
            s("48656c6c6f20776f726c6421")
        );
        assert_eq!(
            fmt("%X", vec![s("Hello world!")]),
            s("48656C6C6F20776F726C6421")
        );
    }
}
