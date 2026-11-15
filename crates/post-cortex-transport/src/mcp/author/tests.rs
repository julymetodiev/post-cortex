// Copyright (c) 2025, 2026 Julius ML
// MIT License

use super::*;

fn parts(author: Option<&str>) -> axum::http::request::Parts {
    let mut request = axum::http::Request::builder().uri("/mcp");
    if let Some(author) = author {
        request = request.header("X-PCX-Author", author);
    }
    request.body(()).unwrap().into_parts().0
}

/// The header wins; a client name that only names its SDK is no author.
#[test]
fn default_author_prefers_the_header_and_skips_sdk_names() {
    let with = parts(Some("claude-code"));
    let without = parts(None);
    assert_eq!(
        default_author(Some(&with), Some("rmcp")).as_deref(),
        Some("claude-code")
    );
    assert_eq!(
        default_author(Some(&with), Some("Zed")).as_deref(),
        Some("claude-code")
    );
    assert_eq!(
        default_author(Some(&without), Some("Zed")).as_deref(),
        Some("Zed")
    );
    assert_eq!(default_author(Some(&without), Some("rmcp")), None);
    assert_eq!(default_author(None, Some("MCP")), None);
    assert_eq!(default_author(Some(&parts(Some("  "))), None), None);
}
