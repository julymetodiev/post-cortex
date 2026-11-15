// Copyright (c) 2026 Julius ML
// MIT License

//! The JSONPath subset grammar: segments and the parser.

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
    pub segments: Vec<PathSegment>,
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
                // Field characters are ASCII, so the range is on char boundaries.
                let field_name = input
                    .get(start..pos)
                    .ok_or(ParseError {
                        byte_offset: start,
                        message: "field name is not on a character boundary".into(),
                    })?
                    .to_string();
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
                let content = read_bracket_content(input, &mut pos)?;
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
fn read_bracket_content(input: &str, pos: &mut usize) -> Result<String, ParseError> {
    let bytes = input.as_bytes();
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
    // `]` and `[` are ASCII, so the range ends on a char boundary.
    input
        .get(start..*pos)
        .map(|content| content.trim().to_string())
        .ok_or(ParseError {
            byte_offset: start,
            message: "bracket content is not on a character boundary".into(),
        })
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
