// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The headers of a request (docs/modules/http.md, requests): a name is a
//! token, a value one line, and the authentication, the host, the cookies
//! and the hop-by-hop headers are only the module's.

/// The headers only the module sets.
const RESERVED: [&str; 13] = [
    "authorization",
    "proxy-authorization",
    "cookie",
    "host",
    "connection",
    "keep-alive",
    "proxy-connection",
    "transfer-encoding",
    "te",
    "trailer",
    "upgrade",
    "content-length",
    "content-type",
];

/// Check header name `name` (in lower case): a token.
///
/// # Errors
/// Why it is none.
pub fn header_name(name: &str) -> Result<(), String> {
    let token = |b: u8| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b);
    if name.is_empty() || !name.bytes().all(token) {
        return Err(format!("{name:?} is no header name"));
    }
    Ok(())
}

/// Whether header `name` (in lower case) is only the module's.
#[must_use]
pub fn reserved(name: &str) -> bool {
    RESERVED.contains(&name) || name.starts_with("proxy-")
}

/// Check header `name` (in lower case) with `value`: a name, not one only
/// the module sets, a value of one line.
///
/// # Errors
/// Why it may not be sent.
pub fn header(name: &str, value: &str) -> Result<(), String> {
    header_name(name)?;
    if reserved(name) {
        return Err(format!("{name} is set by the module"));
    }
    if value.contains(['\r', '\n', '\0']) {
        return Err(format!("{name}'s value is one line"));
    }
    Ok(())
}
