// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use serde_json::json;

use super::*;

#[test]
fn a_failure_by_its_type_else_its_status() {
    let of =
        |status, kind: &str| failure(Some(status), &json!({"type": kind, "message": "m"})).kind;
    assert_eq!(of(429, "rate_limit_error"), ModelFailureKind::RateLimited);
    assert_eq!(of(529, "overloaded_error"), ModelFailureKind::Overloaded);
    assert_eq!(
        of(401, "authentication_error"),
        ModelFailureKind::Unauthorized
    );
    assert_eq!(of(402, "billing_error"), ModelFailureKind::QuotaExhausted);
    assert_eq!(
        of(413, "request_too_large"),
        ModelFailureKind::ContextTooLong
    );
    assert_eq!(of(400, "invalid_request_error"), ModelFailureKind::Invalid);
    assert_eq!(of(503, "something_new"), ModelFailureKind::Overloaded);
    assert_eq!(
        failure(None, &json!("gone")).kind,
        ModelFailureKind::Unavailable
    );
}
