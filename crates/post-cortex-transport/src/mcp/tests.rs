// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use super::*;

use pcx_module_api::ModuleErrorKind;

fn text(result: &CallToolResult) -> String {
    result
        .content
        .iter()
        .filter_map(|c| c.as_text().map(|t| t.text.clone()))
        .collect()
}

#[test]
fn an_output_is_its_text_then_its_results() {
    let result = call_result(Ok(ToolOutput {
        text: "Created".to_string(),
        data: Some(serde_json::json!({"id": 1})),
        failed: false,
        meta: None,
    }));
    assert_eq!(result.is_error, Some(false));
    assert_eq!(text(&result), "Created\n\nResults:\n{\"id\":1}");
}

#[test]
fn a_failed_output_keeps_its_results() {
    let result = call_result(Ok(ToolOutput {
        text: "1 failed".to_string(),
        data: Some(serde_json::json!([{"ok": false}])),
        failed: true,
        meta: None,
    }));
    assert_eq!(result.is_error, Some(true));
    assert!(text(&result).contains("Results:"));
}

#[test]
fn an_error_is_a_tool_error_with_its_details() {
    let error = ModuleError::new(ModuleErrorKind::InvalidArgument, "bad action")
        .with_details(serde_json::json!({"kind": "invalid_argument"}));
    let result = call_result(Err(error));
    assert_eq!(result.is_error, Some(true));
    assert_eq!(
        text(&result),
        "bad action\n\nDetails:\n{\"kind\":\"invalid_argument\"}"
    );
    let bare = call_result(Err(ModuleError::new(
        ModuleErrorKind::DeadlineExceeded,
        "late",
    )));
    assert_eq!(text(&bare), "late");
}
