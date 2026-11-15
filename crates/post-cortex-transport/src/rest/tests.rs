// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use std::collections::BTreeSet;

use post_cortex_memory::tools::operations;

use super::*;

/// Every failure kind has its one HTTP status, internal included.
#[test]
fn every_failure_kind_has_its_http_status() {
    for (kind, code) in [
        (Kind::InvalidArgument, StatusCode::BAD_REQUEST),
        (Kind::NotFound, StatusCode::NOT_FOUND),
        (Kind::Conflict, StatusCode::CONFLICT),
        (Kind::Denied, StatusCode::FORBIDDEN),
        (Kind::FailedPrecondition, StatusCode::PRECONDITION_FAILED),
        (Kind::Unavailable, StatusCode::SERVICE_UNAVAILABLE),
        (Kind::Internal, StatusCode::INTERNAL_SERVER_ERROR),
    ] {
        let (status, message) = rest_status(Failure::new(kind, "x"));
        assert_eq!((status, message.as_str()), (code, "x"), "{kind:?}");
    }
}

/// Every REST route of the router is an operation of the table, and every
/// route the table names is served.
#[test]
fn rest_exposes_exactly_the_table() {
    let source = include_str!("../rest.rs");
    let mut served = BTreeSet::new();
    let mut rest = source;
    while let Some(at) = rest.find(".route(") {
        rest = &rest[at + ".route(".len()..];
        let path = rest.trim_start().trim_start_matches('"');
        let path = &path[..path.find('"').unwrap()];
        // The route's arguments: up to its closing parenthesis.
        let mut depth = 1;
        let end = rest
            .char_indices()
            .find_map(|(i, c)| {
                depth += i32::from(c == '(') - i32::from(c == ')');
                (depth == 0).then_some(i)
            })
            .unwrap();
        let call = &rest[..end];
        for method in ["get", "post", "put", "delete"] {
            if call.contains(&format!("{method}(api_")) {
                served.insert(format!("{} {path}", method.to_uppercase()));
            }
        }
    }
    let table: BTreeSet<String> = operations::rest_routes()
        .into_iter()
        .map(String::from)
        .collect();
    assert_eq!(served, table);
}
