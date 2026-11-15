//! Encoders extension library for CEL.
//!
//! This module provides encoding/decoding functions for CEL,
//! matching the cel-go encoders extension.
//!
//! # Functions
//!
//! - `base64.encode(bytes) -> string` - Encodes bytes to base64 string
//! - `base64.decode(string) -> bytes` - Decodes base64 string to bytes

use std::sync::Arc;

use crate::eval::{EvalError, Value};
use crate::types::{CelType, FunctionDecl, OverloadDecl};

/// Encode bytes as standard base64 (RFC 4648).
fn base64_encode(data: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

    let mut result = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = if chunk.len() > 1 { chunk[1] as u32 } else { 0 };
        let b2 = if chunk.len() > 2 { chunk[2] as u32 } else { 0 };
        let triple = (b0 << 16) | (b1 << 8) | b2;

        result.push(ALPHABET[((triple >> 18) & 0x3F) as usize] as char);
        result.push(ALPHABET[((triple >> 12) & 0x3F) as usize] as char);
        if chunk.len() > 1 {
            result.push(ALPHABET[((triple >> 6) & 0x3F) as usize] as char);
        } else {
            result.push('=');
        }
        if chunk.len() > 2 {
            result.push(ALPHABET[(triple & 0x3F) as usize] as char);
        } else {
            result.push('=');
        }
    }
    result
}

/// Decode a standard base64 string (RFC 4648) to bytes.
/// Supports both padded and unpadded input.
fn base64_decode(input: &str) -> Result<Vec<u8>, String> {
    const DECODE_TABLE: [u8; 256] = {
        let mut table = [0xFFu8; 256];
        let alphabet = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut i = 0;
        while i < 64 {
            table[alphabet[i] as usize] = i as u8;
            i += 1;
        }
        table
    };

    // Strip padding
    let input = input.trim_end_matches('=');
    let len = input.len();

    let mut result = Vec::with_capacity(len * 3 / 4);
    let bytes = input.as_bytes();
    let mut i = 0;

    while i + 4 <= len {
        let a = DECODE_TABLE[bytes[i] as usize];
        let b = DECODE_TABLE[bytes[i + 1] as usize];
        let c = DECODE_TABLE[bytes[i + 2] as usize];
        let d = DECODE_TABLE[bytes[i + 3] as usize];
        if a == 0xFF || b == 0xFF || c == 0xFF || d == 0xFF {
            return Err("invalid base64 character".to_string());
        }
        let triple = (a as u32) << 18 | (b as u32) << 12 | (c as u32) << 6 | (d as u32);
        result.push((triple >> 16) as u8);
        result.push((triple >> 8) as u8);
        result.push(triple as u8);
        i += 4;
    }

    let remaining = len - i;
    match remaining {
        2 => {
            let a = DECODE_TABLE[bytes[i] as usize];
            let b = DECODE_TABLE[bytes[i + 1] as usize];
            if a == 0xFF || b == 0xFF {
                return Err("invalid base64 character".to_string());
            }
            let triple = (a as u32) << 18 | (b as u32) << 12;
            result.push((triple >> 16) as u8);
        }
        3 => {
            let a = DECODE_TABLE[bytes[i] as usize];
            let b = DECODE_TABLE[bytes[i + 1] as usize];
            let c = DECODE_TABLE[bytes[i + 2] as usize];
            if a == 0xFF || b == 0xFF || c == 0xFF {
                return Err("invalid base64 character".to_string());
            }
            let triple = (a as u32) << 18 | (b as u32) << 12 | (c as u32) << 6;
            result.push((triple >> 16) as u8);
            result.push((triple >> 8) as u8);
        }
        1 => return Err("invalid base64 input length".to_string()),
        0 => {}
        _ => unreachable!(),
    }

    Ok(result)
}

/// Returns the encoders extension library function declarations.
pub fn encoders_extension() -> Vec<FunctionDecl> {
    vec![
        // base64.encode(bytes) -> string
        FunctionDecl::new("base64.encode").with_overload(
            OverloadDecl::function("base64_encode_bytes", vec![CelType::Bytes], CelType::String)
                .with_impl(|args| match &args[0] {
                    Value::Bytes(b) => Value::String(Arc::from(base64_encode(b))),
                    _ => Value::error(EvalError::type_mismatch(
                        "bytes",
                        &args[0].cel_type().display_name(),
                    )),
                }),
        ),
        // base64.decode(string) -> bytes
        FunctionDecl::new("base64.decode").with_overload(
            OverloadDecl::function(
                "base64_decode_string",
                vec![CelType::String],
                CelType::Bytes,
            )
            .with_impl(|args| match &args[0] {
                Value::String(s) => match base64_decode(s) {
                    Ok(bytes) => Value::Bytes(Arc::from(bytes)),
                    Err(e) => Value::error(EvalError::internal(e)),
                },
                _ => Value::error(EvalError::type_mismatch(
                    "string",
                    &args[0].cel_type().display_name(),
                )),
            }),
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encoders_extension_has_functions() {
        let funcs = encoders_extension();
        assert_eq!(funcs.len(), 2);
    }

    #[test]
    fn test_base64_encode() {
        let funcs = encoders_extension();
        let encode = funcs.iter().find(|f| f.name == "base64.encode").unwrap();
        assert_eq!(encode.overloads.len(), 1);
        assert_eq!(encode.overloads[0].id, "base64_encode_bytes");
        assert_eq!(encode.overloads[0].params, vec![CelType::Bytes]);
        assert_eq!(encode.overloads[0].result, CelType::String);
    }

    #[test]
    fn test_base64_decode() {
        let funcs = encoders_extension();
        let decode = funcs.iter().find(|f| f.name == "base64.decode").unwrap();
        assert_eq!(decode.overloads.len(), 1);
        assert_eq!(decode.overloads[0].id, "base64_decode_string");
        assert_eq!(decode.overloads[0].params, vec![CelType::String]);
        assert_eq!(decode.overloads[0].result, CelType::Bytes);
    }

    #[test]
    fn test_all_functions_are_standalone() {
        let funcs = encoders_extension();
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
    fn test_base64_encode_impl() {
        let result = base64_encode(b"hello");
        assert_eq!(result, "aGVsbG8=");
    }

    #[test]
    fn test_base64_decode_impl() {
        // With padding
        assert_eq!(base64_decode("aGVsbG8=").unwrap(), b"hello");
        // Without padding
        assert_eq!(base64_decode("aGVsbG8").unwrap(), b"hello");
    }

    #[test]
    fn test_base64_roundtrip() {
        let data = b"Hello World!";
        let encoded = base64_encode(data);
        let decoded = base64_decode(&encoded).unwrap();
        assert_eq!(decoded, data);
    }
}
