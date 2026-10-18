// Copyright (c) 2026 Julius ML
// MIT License

//! Minimal JSONPath subset parser and executor for the `select` parameter
//! on `AssembleContext`.
//!
//! **Allowed syntax:**
//! - `$` (root)
//! - `.field` (child access)
//! - `[n]` / `[-n]` (index, including negative from end)
//! - `[a:b]` (slice, no step)
//! - `*` or `[*]` (wildcard)
//! - `[0,3,7]` (union of indices)
//!
//! **Forbidden (parse-time error with line+col):**
//! - `..` recursive descent
//! - `[?(...)]` filter expressions
//! - functions: `length()`, `match()`, `search()`, `count()`
//! - `[a:b:c]` step in slice

use serde_json::Value;
use std::fmt;

// ---------------------------------------------------------------------------
// AST
// ---------------------------------------------------------------------------

/// A single segment in a parsed JSONPath expression.
#[derive(Debug, Clone, PartialEq)]
pub enum PathSegment {
    /// `.field` — named child access.
    Field(String),
    /// `[n]` — single index (negative = from end).
    Index(isize),
    /// `[a:b]` — slice. `None` on either bound means unbounded on that side.
    Slice {
        /// Inclusive lower bound (negative = from end, `None` = `0`).
        start: Option<isize>,
        /// Exclusive upper bound (negative = from end, `None` = array length).
        end: Option<isize>,
    },
    /// `[*]` or `.*` — wildcard (all children).
    Wildcard,
    /// `[0,3,7]` — union of specific indices.
    Union(Vec<isize>),
}

/// Parsed JSONPath expression.
#[derive(Debug, Clone, PartialEq)]
pub struct JsonPath {
    segments: Vec<PathSegment>,
}

// ---------------------------------------------------------------------------
// Parse error
// ---------------------------------------------------------------------------

/// Error returned when a JSONPath expression is invalid or uses forbidden
/// syntax.  Carries byte-offset (approximated as line+col) and a short
/// message suitable for a 400 response.
#[derive(Debug, Clone)]
pub struct ParseError {
    /// Byte offset in the input where the error was detected.
    pub byte_offset: usize,
    /// Human-readable error description.
    pub message: String,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "at offset {}: {}", self.byte_offset, self.message)
    }
}

impl std::error::Error for ParseError {}

// ---------------------------------------------------------------------------
// Parser
// ---------------------------------------------------------------------------

/// Parse a JSONPath expression string into a [`JsonPath`].
///
/// Returns an error for forbidden constructs (`..`, filters, functions,
/// slice step).
pub fn parse(input: &str) -> Result<JsonPath, ParseError> {
    let bytes = input.as_bytes();
    let len = bytes.len();
    let mut pos = 0;

    // Must start with '$'
    if pos >= len || bytes[pos] != b'$' {
        return Err(ParseError {
            byte_offset: pos,
            message: "JSONPath must start with '$'".into(),
        });
    }
    pos += 1; // skip '$'

    let mut segments = Vec::new();

    while pos < len {
        match bytes[pos] {
            b'.' => {
                pos += 1;
                // Check for forbidden recursive descent '..'
                if pos < len && bytes[pos] == b'.' {
                    return Err(ParseError {
                        byte_offset: pos - 1,
                        message: "recursive descent (..) is not allowed".into(),
                    });
                }
                // Wildcard: .*
                if pos < len && bytes[pos] == b'*' {
                    pos += 1;
                    segments.push(PathSegment::Wildcard);
                    continue;
                }
                // Field name
                let start = pos;
                while pos < len && is_field_char(bytes[pos]) {
                    pos += 1;
                }
                if pos == start {
                    return Err(ParseError {
                        byte_offset: pos,
                        message: "expected field name after '.'".into(),
                    });
                }
                let field_name = std::str::from_utf8(&bytes[start..pos]).unwrap().to_string();
                // Check for forbidden function call: field followed by '('
                if pos < len && bytes[pos] == b'(' {
                    return Err(ParseError {
                        byte_offset: start,
                        message: format!("function calls ({}()) are not allowed", field_name),
                    });
                }
                segments.push(PathSegment::Field(field_name));
            }
            b'[' => {
                pos += 1; // skip '['

                // Check for forbidden filter [?(...)]
                if pos < len && bytes[pos] == b'?' {
                    return Err(ParseError {
                        byte_offset: pos - 1,
                        message: "filter expressions [?(...)] are not allowed".into(),
                    });
                }

                // Wildcard: [*]
                if pos < len && bytes[pos] == b'*' {
                    pos += 1;
                    expect_byte(bytes, &mut pos, b']', "*")?;
                    segments.push(PathSegment::Wildcard);
                    continue;
                }

                // Now we expect: integer, slice, or union
                // Peek to distinguish slice [a:b] vs union [a,b] vs index [a]
                let content_start = pos;
                let content = read_bracket_content(bytes, &mut pos)?;
                expect_byte(bytes, &mut pos, b']', &content)?;

                // Parse content
                let segment = parse_bracket_content(content_start, &content)?;
                segments.push(segment);
            }
            _ => {
                return Err(ParseError {
                    byte_offset: pos,
                    message: format!(
                        "unexpected character '{}'; expected '.' or '['",
                        bytes[pos] as char
                    ),
                });
            }
        }
    }

    Ok(JsonPath { segments })
}

/// Is this byte a valid field-name character (alphanumeric, underscore)?
fn is_field_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// Read everything until `]`, returning the raw content string.
fn read_bracket_content(bytes: &[u8], pos: &mut usize) -> Result<String, ParseError> {
    let start = *pos;
    let mut depth = 1u32;
    while *pos < bytes.len() && depth > 0 {
        match bytes[*pos] {
            b']' => {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            b'[' => {
                depth += 1;
            }
            _ => {}
        }
        *pos += 1;
    }
    Ok(std::str::from_utf8(&bytes[start..*pos])
        .unwrap()
        .trim()
        .to_string())
}

/// Expect a specific byte at current position, advancing past it.
fn expect_byte(
    bytes: &[u8],
    pos: &mut usize,
    expected: u8,
    context: &str,
) -> Result<(), ParseError> {
    if *pos < bytes.len() && bytes[*pos] == expected {
        *pos += 1;
        Ok(())
    } else {
        Err(ParseError {
            byte_offset: *pos,
            message: format!("expected '{}' after '{}'", expected as char, context),
        })
    }
}

/// Parse the content inside `[...]` into a segment.
fn parse_bracket_content(offset: usize, content: &str) -> Result<PathSegment, ParseError> {
    let content = content.trim();

    // Check for forbidden function calls
    if content.ends_with("()") {
        return Err(ParseError {
            byte_offset: offset,
            message: format!("function calls ({}) are not allowed", content),
        });
    }

    // Check if it contains ':' → slice
    if content.contains(':') {
        let parts: Vec<&str> = content.split(':').collect();
        if parts.len() > 2 {
            return Err(ParseError {
                byte_offset: offset,
                message: "slice step [a:b:c] is not allowed".into(),
            });
        }
        let start = if parts[0].trim().is_empty() {
            None
        } else {
            Some(parse_int(offset, parts[0].trim())?)
        };
        let end = if parts[1].trim().is_empty() {
            None
        } else {
            Some(parse_int(offset, parts[1].trim())?)
        };
        return Ok(PathSegment::Slice { start, end });
    }

    // Check if it contains ',' → union of indices
    if content.contains(',') {
        let parts: Vec<&str> = content.split(',').collect();
        let mut indices = Vec::with_capacity(parts.len());
        for p in &parts {
            indices.push(parse_int(offset, p.trim())?);
        }
        return Ok(PathSegment::Union(indices));
    }

    // Single integer index
    let idx = parse_int(offset, content)?;
    Ok(PathSegment::Index(idx))
}

/// Parse a (possibly negative) integer from a trimmed string.
fn parse_int(offset: usize, s: &str) -> Result<isize, ParseError> {
    s.parse::<isize>().map_err(|_| ParseError {
        byte_offset: offset,
        message: format!("expected integer, got '{}'", s),
    })
}

// ---------------------------------------------------------------------------
// Executor
// ---------------------------------------------------------------------------

/// Execute a parsed [`JsonPath`] against a JSON [`Value`].
///
/// Returns the matched value. If nothing matches, returns `Value::Null`.
pub fn execute(path: &JsonPath, root: &Value) -> Value {
    // Walk by reference; only the final matches are cloned.
    let mut current: Vec<&Value> = vec![root];

    for segment in &path.segments {
        let mut next = Vec::new();
        for val in current {
            match segment {
                PathSegment::Field(name) => {
                    if let Some(child) = val.get(name) {
                        next.push(child);
                    }
                }
                PathSegment::Index(idx) => {
                    if let Some(v) = index_value(val, *idx) {
                        next.push(v);
                    }
                }
                PathSegment::Slice { start, end } => {
                    next.extend(slice_value(val, *start, *end));
                }
                PathSegment::Wildcard => match val {
                    Value::Array(arr) => next.extend(arr),
                    Value::Object(map) => next.extend(map.values()),
                    _ => {}
                },
                PathSegment::Union(indices) => {
                    for &idx in indices {
                        if let Some(v) = index_value(val, idx) {
                            next.push(v);
                        }
                    }
                }
            }
        }
        current = next;
    }

    match current.as_slice() {
        [] => Value::Null,
        [single] => (*single).clone(),
        many => Value::Array(many.iter().map(|v| (*v).clone()).collect()),
    }
}

/// Apply a `select` specification against a JSON response.
///
/// **Form A** (`select_string`): single JSONPath → returns the matched value
/// directly (or `Null` if nothing matched).
///
/// **Form B** (`select_map`): map of output-key → JSONPath → returns an
/// object with the same keys, each containing the match result.
pub fn apply_select(
    response_json: &Value,
    select_string: Option<&str>,
    select_map: Option<&std::collections::HashMap<String, String>>,
) -> Result<Value, ParseError> {
    if let Some(expr) = select_string {
        let path = parse(expr)?;
        Ok(execute(&path, response_json))
    } else if let Some(map) = select_map {
        let mut result = serde_json::Map::with_capacity(map.len());
        for (key, expr) in map {
            let path = parse(expr)?;
            let val = execute(&path, response_json);
            result.insert(key.clone(), val);
        }
        Ok(Value::Object(result))
    } else {
        Ok(response_json.clone())
    }
}

// ---------------------------------------------------------------------------
// Executor helpers
// ---------------------------------------------------------------------------

fn index_value(val: &Value, idx: isize) -> Option<&Value> {
    match val {
        Value::Array(arr) => {
            if idx >= 0 {
                arr.get(idx as usize)
            } else {
                let from_end = idx.unsigned_abs();
                if from_end <= arr.len() {
                    arr.get(arr.len() - from_end)
                } else {
                    None
                }
            }
        }
        _ => None,
    }
}

fn slice_value(val: &Value, start: Option<isize>, end: Option<isize>) -> &[Value] {
    match val {
        Value::Array(arr) => {
            let len = arr.len() as isize;
            let s = start.unwrap_or(0);
            let s = if s < 0 { (len + s).max(0) } else { s } as usize;
            let e = end.unwrap_or(len);
            let e = if e < 0 { (len + e).max(0) } else { e.min(len) } as usize;
            if s >= e { &[] } else { &arr[s..e] }
        }
        _ => &[],
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // ---- Parser positive cases ----

    #[test]
    fn parse_root_only() {
        let p = parse("$").unwrap();
        assert!(p.segments.is_empty());
    }

    #[test]
    fn parse_field() {
        let p = parse("$.formatted_text").unwrap();
        assert_eq!(
            p.segments,
            vec![PathSegment::Field("formatted_text".into())]
        );
    }

    #[test]
    fn parse_nested_fields() {
        let p = parse("$.items[0].text").unwrap();
        assert_eq!(
            p.segments,
            vec![
                PathSegment::Field("items".into()),
                PathSegment::Index(0),
                PathSegment::Field("text".into()),
            ]
        );
    }

    #[test]
    fn parse_positive_index() {
        let p = parse("$.items[3]").unwrap();
        assert_eq!(p.segments[1], PathSegment::Index(3));
    }

    #[test]
    fn parse_negative_index() {
        let p = parse("$.items[-2]").unwrap();
        assert_eq!(p.segments[1], PathSegment::Index(-2));
    }

    #[test]
    fn parse_slice() {
        let p = parse("$.items[0:3]").unwrap();
        assert_eq!(
            p.segments[1],
            PathSegment::Slice {
                start: Some(0),
                end: Some(3)
            }
        );
    }

    #[test]
    fn parse_slice_open_start() {
        let p = parse("$.items[:5]").unwrap();
        assert_eq!(
            p.segments[1],
            PathSegment::Slice {
                start: None,
                end: Some(5)
            }
        );
    }

    #[test]
    fn parse_slice_open_end() {
        let p = parse("$.items[2:]").unwrap();
        assert_eq!(
            p.segments[1],
            PathSegment::Slice {
                start: Some(2),
                end: None
            }
        );
    }

    #[test]
    fn parse_wildcard_bracket() {
        let p = parse("$.items[*]").unwrap();
        assert_eq!(p.segments[1], PathSegment::Wildcard);
    }

    #[test]
    fn parse_wildcard_dot() {
        let p = parse("$.items.*").unwrap();
        assert_eq!(p.segments[1], PathSegment::Wildcard);
    }

    #[test]
    fn parse_union() {
        let p = parse("$.items[0,3,7]").unwrap();
        assert_eq!(p.segments[1], PathSegment::Union(vec![0, 3, 7]));
    }

    // ---- Parser forbidden constructs ----

    #[test]
    fn parse_error_recursive_descent() {
        let e = parse("$..name").unwrap_err();
        assert!(e.message.contains("recursive descent"));
    }

    #[test]
    fn parse_error_filter_expression() {
        let e = parse("$.items[?(@.score > 0.5)]").unwrap_err();
        assert!(e.message.contains("filter"));
    }

    #[test]
    fn parse_error_function_length() {
        let e = parse("$.items.length()").unwrap_err();
        assert!(e.message.contains("function"));
    }

    #[test]
    fn parse_error_slice_step() {
        let e = parse("$.items[0:10:2]").unwrap_err();
        assert!(e.message.contains("step"));
    }

    #[test]
    fn parse_error_no_dollar() {
        let e = parse("items[0]").unwrap_err();
        assert!(e.message.contains("'$'"));
    }

    #[test]
    fn parse_error_unexpected_char() {
        let e = parse("$.items|(0)").unwrap_err();
        assert!(e.message.contains("unexpected"));
    }

    // ---- Executor tests ----

    fn sample_response() -> Value {
        json!({
            "items": [
                {"text": "first", "entry_id": "id-1", "score": 0.9},
                {"text": "second", "entry_id": "id-2", "score": 0.7},
                {"text": "third", "entry_id": "id-3", "score": 0.5},
                {"text": "fourth", "entry_id": "id-4", "score": 0.3},
                {"text": "fifth", "entry_id": "id-5", "score": 0.1},
            ],
            "formatted_text": "Here is the assembled context...",
            "total_tokens": 1234,
            "entity_context": [
                {"name": "Rust", "relevance": "direct_mention"},
                {"name": "gRPC", "relevance": "graph_neighbor"},
                {"name": "tonic", "relevance": "graph_neighbor"},
                {"name": "Axon", "relevance": "direct_mention"},
                {"name": "RocksDB", "relevance": "graph_neighbor"},
                {"name": "Post-Cortex", "relevance": "direct_mention"},
            ],
            "impact": [
                {"entity": "Axon", "depends_on": "gRPC", "relation_type": "DependsOn"}
            ]
        })
    }

    #[test]
    fn exec_field() {
        let p = parse("$.formatted_text").unwrap();
        let v = execute(&p, &sample_response());
        assert_eq!(v, json!("Here is the assembled context..."));
    }

    #[test]
    fn exec_index() {
        let p = parse("$.items[0].text").unwrap();
        let v = execute(&p, &sample_response());
        assert_eq!(v, json!("first"));
    }

    #[test]
    fn exec_negative_index() {
        let p = parse("$.items[-1].text").unwrap();
        let v = execute(&p, &sample_response());
        assert_eq!(v, json!("fifth"));
    }

    #[test]
    fn exec_slice() {
        let p = parse("$.items[0:3].text").unwrap();
        let v = execute(&p, &sample_response());
        assert_eq!(v, json!(["first", "second", "third"]));
    }

    #[test]
    fn exec_wildcard() {
        let p = parse("$.entity_context[*].name").unwrap();
        let v = execute(&p, &sample_response());
        assert_eq!(
            v,
            json!(["Rust", "gRPC", "tonic", "Axon", "RocksDB", "Post-Cortex"])
        );
    }

    #[test]
    fn exec_union() {
        let p = parse("$.items[0,4].entry_id").unwrap();
        let v = execute(&p, &sample_response());
        assert_eq!(v, json!(["id-1", "id-5"]));
    }

    #[test]
    fn exec_empty_match_returns_null() {
        let p = parse("$.nonexistent").unwrap();
        let v = execute(&p, &sample_response());
        assert_eq!(v, Value::Null);
    }

    #[test]
    fn exec_out_of_bounds_returns_null() {
        let p = parse("$.items[99]").unwrap();
        let v = execute(&p, &sample_response());
        assert_eq!(v, Value::Null);
    }

    // ---- apply_select tests ----

    #[test]
    fn select_string_form() {
        let resp = sample_response();
        let v = apply_select(&resp, Some("$.formatted_text"), None).unwrap();
        assert_eq!(v, json!("Here is the assembled context..."));
    }

    #[test]
    fn select_map_form() {
        let resp = sample_response();
        let map = std::collections::HashMap::from([
            ("text".into(), "$.formatted_text".into()),
            ("top3".into(), "$.items[0:3].text".into()),
            ("ids".into(), "$.items[*].entry_id".into()),
            ("neighbours".into(), "$.entity_context[0:5].name".into()),
        ]);
        let v = apply_select(&resp, None, Some(&map)).unwrap();
        assert_eq!(v["text"], json!("Here is the assembled context..."));
        assert_eq!(v["top3"], json!(["first", "second", "third"]));
        assert_eq!(v["ids"], json!(["id-1", "id-2", "id-3", "id-4", "id-5"]));
        assert_eq!(
            v["neighbours"],
            json!(["Rust", "gRPC", "tonic", "Axon", "RocksDB"])
        );
    }

    #[test]
    fn select_map_empty_match_gives_null() {
        let resp = sample_response();
        let map =
            std::collections::HashMap::from([("missing".into(), "$.nonexistent_field".into())]);
        let v = apply_select(&resp, None, Some(&map)).unwrap();
        assert_eq!(v["missing"], Value::Null);
    }

    #[test]
    fn select_invalid_expression_returns_parse_error() {
        let resp = sample_response();
        let e = apply_select(&resp, Some("$..bad"), None).unwrap_err();
        assert!(e.message.contains("recursive descent"));
        assert!(e.byte_offset > 0);
    }

    #[test]
    fn select_none_returns_full_response() {
        let resp = sample_response();
        let v = apply_select(&resp, None, None).unwrap();
        assert_eq!(v, resp);
    }
}
