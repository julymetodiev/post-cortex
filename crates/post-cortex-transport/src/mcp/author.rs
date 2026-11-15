// Copyright (c) 2025, 2026 Julius ML
// MIT License

//! Who a write names as its author when the call names none.

use post_cortex_proto::AUTHOR_HEADER;

/// `clientInfo.name` values that name an MCP SDK rather than the
/// application using it.
const SDK_CLIENT_NAMES: &[&str] = &["rmcp", "mcp"];

/// The [`AUTHOR_HEADER`] of an HTTP request, when set.
#[must_use]
pub fn header_author(headers: &axum::http::HeaderMap) -> Option<String> {
    author_value(headers.get(AUTHOR_HEADER).and_then(|v| v.to_str().ok()))
}

/// The author an [`AUTHOR_HEADER`] value names: its text trimmed, none
/// when it is empty or absent (over HTTP and gRPC metadata alike).
#[must_use]
pub fn author_value(raw: Option<&str>) -> Option<String> {
    raw.map(str::trim)
        .filter(|v| !v.is_empty())
        .map(String::from)
}

/// Author of a write that names none: the [`AUTHOR_HEADER`] of the HTTP
/// request, else the MCP client's name unless that only names its SDK.
pub fn default_author(
    parts: Option<&axum::http::request::Parts>,
    client_name: Option<&str>,
) -> Option<String> {
    let client = client_name
        .map(str::trim)
        .filter(|n| !n.is_empty() && !SDK_CLIENT_NAMES.contains(&n.to_ascii_lowercase().as_str()))
        .map(String::from);
    parts.and_then(|p| header_author(&p.headers)).or(client)
}

#[cfg(test)]
mod tests;
