// Copyright (c) 2025, 2026 Julius ML
// MIT License

use super::*;

#[test]
fn test_code_reference_string_forms() {
    use serde_json::json;
    let r = code_reference(Some(&json!("src/main.rs:42")), "code_reference")
        .unwrap()
        .unwrap();
    assert_eq!(
        (r.file_path.as_str(), r.start_line, r.end_line),
        ("src/main.rs", 42, 42)
    );

    let r = code_reference(Some(&json!("src/lib.rs:10-20")), "code_reference")
        .unwrap()
        .unwrap();
    assert_eq!((r.start_line, r.end_line), (10, 20));

    let r = code_reference(Some(&json!("C:\\x\\y.rs")), "code_reference")
        .unwrap()
        .unwrap();
    assert_eq!((r.file_path.as_str(), r.start_line), ("C:\\x\\y.rs", 0));

    assert!(code_reference(None, "code_reference").unwrap().is_none());
    assert!(
        code_reference(Some(&json!(null)), "code_reference")
            .unwrap()
            .is_none()
    );
}

#[test]
fn test_code_reference_object_forms() {
    use serde_json::json;
    let r = code_reference(
        Some(&json!({"file": "src/main.rs", "line": 42, "function": "process_data"})),
        "code_reference",
    )
    .unwrap()
    .unwrap();
    assert_eq!(r.file_path, "src/main.rs");
    assert_eq!((r.start_line, r.end_line), (42, 42));
    assert_eq!(r.change_description, "in process_data");

    // Canonical CodeReference shape still round-trips.
    let canonical = json!({
        "file_path": "a.rs", "start_line": 1, "end_line": 3,
        "code_snippet": "fn a() {}", "commit_hash": "abc", "branch": "main",
        "change_description": "added a"
    });
    let r = code_reference(Some(&canonical), "code_reference")
        .unwrap()
        .unwrap();
    assert_eq!((r.start_line, r.end_line), (1, 3));
    assert_eq!(r.commit_hash.as_deref(), Some("abc"));
    assert_eq!(r.change_description, "added a");
}

#[test]
fn test_code_reference_malformed_is_rejected() {
    use serde_json::json;
    for bad in [
        json!(42),
        json!(["a.rs"]),
        json!({"line": 3}),
        json!({"file": "a.rs", "line": "three"}),
        json!(""),
    ] {
        let err = code_reference(Some(&bad), "updates[0].code_reference")
            .expect_err(&format!("{bad} should be rejected"));
        assert_eq!(
            err.details.parameter.as_deref(),
            Some("updates[0].code_reference")
        );
    }
}
