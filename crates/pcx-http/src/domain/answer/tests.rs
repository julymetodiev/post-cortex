// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use pcx_expr::schema::fields;
use serde_json::json;

use super::*;

#[test]
fn a_status_answers_or_fails_as_a_retry_reads_it() {
    assert!(answers(204, &[]) && !answers(404, &[]) && answers(404, &[404]));
    assert!(!answers(200, &[404]));
    assert_eq!(failure_kind(401), ModuleErrorKind::Denied);
    assert_eq!(failure_kind(404), ModuleErrorKind::NotFound);
    assert_eq!(failure_kind(429), ModuleErrorKind::Unavailable);
    assert_eq!(failure_kind(503), ModuleErrorKind::Unavailable);
    assert_eq!(failure_kind(422), ModuleErrorKind::FailedPrecondition);
    assert_eq!(failure_kind(304), ModuleErrorKind::FailedPrecondition);
}

#[test]
fn a_failure_never_shows_a_secret() {
    let message = failure_message(
        401,
        "Unauthorized",
        br#"{"error":"bad token tok_123"}"#,
        &["tok_123".into()],
    );
    assert_eq!(
        message,
        r#"HTTP 401 Unauthorized: {"error":"bad token ***"}"#
    );
    let long = failure_message(
        500,
        "Internal Server Error",
        "é".repeat(600).as_bytes(),
        &[],
    );
    assert_eq!(
        long.chars().count(),
        "HTTP 500 Internal Server Error: ".len() + 512
    );
}

#[test]
fn a_body_is_read_by_its_media_type() {
    let (body, json) = read("application/json; charset=utf-8", br#"{"a":1}"#.to_vec());
    assert!(matches!(body, Body::Json(_)));
    assert_eq!(json, Some(json!({"a": 1})));
    let (body, json) = read("application/json", b"not json".to_vec());
    assert_eq!((body, json), (Body::Text("not json".into()), None));
    assert_eq!(
        read("text/html", b"<p>".to_vec()).0,
        Body::Text("<p>".into())
    );
    assert_eq!(read("image/png", vec![137, 80]).0, Body::Raw(vec![137, 80]));
    assert_eq!(read("", vec![0xff]).0, Body::Raw(vec![0xff]));
}

#[test]
fn cookies_are_never_shown() {
    let shown = shown_headers(vec![
        ("Set-Cookie".into(), "s=1".into()),
        ("ETag".into(), "x".into()),
    ]);
    assert_eq!(shown, [("etag".to_string(), "x".to_string())]);
}

#[test]
fn a_body_is_held_to_what_is_expected() {
    let expect = Expect {
        fields: fields([("ok", "bool", true), ("ts", "string", false)]).unwrap(),
        list: false,
    };
    assert!(fits(&expect, Some(&json!({"ok": true, "extra": 1}))).is_ok());
    let missing = fits(&expect, Some(&json!({"ts": "1"}))).unwrap_err();
    assert_eq!(
        (missing.kind, missing.field.as_str()),
        (ModuleErrorKind::FailedPrecondition, "body")
    );
    assert!(fits(&expect, Some(&json!({"ok": "yes"}))).is_err());
    assert!(fits(&expect, None).is_err());
    let list = Expect {
        list: true,
        ..expect
    };
    assert!(fits(&list, Some(&json!([{"ok": true}, {"ok": false}]))).is_ok());
    assert_eq!(
        fits(&list, Some(&json!([{"ok": true}, {}])))
            .unwrap_err()
            .field,
        "body[1]"
    );
    assert!(fits(&list, Some(&json!({"ok": true}))).is_err());
}

#[test]
fn a_null_of_a_body_is_a_null_value() {
    use pbjson_types::value::Kind;
    let (body, _) = read(
        "application/json",
        br#"{"a": null, "b": [null, 1]}"#.to_vec(),
    );
    let Body::Json(v) = body else {
        panic!("not JSON: {body:?}");
    };
    let Some(Kind::StructValue(s)) = v.kind else {
        panic!("not an object");
    };
    assert_eq!(s.fields["a"].kind, Some(Kind::NullValue(0)));
    let Some(Kind::ListValue(l)) = &s.fields["b"].kind else {
        panic!("not a list");
    };
    assert_eq!(l.values[0].kind, Some(Kind::NullValue(0)));
    assert_eq!(l.values[1].kind, Some(Kind::NumberValue(1.0)));
}
