// Copyright (c) 2025, 2026 Julius ML
// MIT License

use super::*;
use serde::Deserialize;
use serde_json::json;
use std::collections::HashMap;

#[test]
fn test_coerce_number_to_string() {
    let value = json!({"session_id": 123});
    let result: HashMap<String, String> = coerce_and_validate(value).unwrap();
    assert_eq!(result.get("session_id"), Some(&"123".to_string()));
}

/// Mirrors the shape of the update_conversation_context request.
#[derive(Debug, Deserialize)]
struct UpdateReq {
    session_id: String,
    content: Option<HashMap<String, String>>,
    updates: Option<Vec<UpdateItem>>,
    dry_run: Option<bool>,
    code_reference: Option<Value>,
}

#[derive(Debug, Deserialize)]
struct UpdateItem {
    interaction_type: String,
    content: HashMap<String, String>,
}

/// Mirrors the shape of the semantic_search request.
#[derive(Debug, Deserialize)]
struct SearchReq {
    query: String,
    scope_id: Option<String>,
    limit: Option<usize>,
    recency_bias: Option<f32>,
    interaction_type: Option<Vec<String>>,
}

#[test]
fn test_coerce_content_values_to_strings() {
    // Use an arbitrary float that doesn't approximate `f64::consts::PI`.
    let value = json!({
        "session_id": "s",
        "content": {
            "score": 2.71,
            "count": 42,
            "enabled": true,
            "metadata": {"key": "value"},
            "tags": ["tag1", "tag2"],
            "text": "plain"
        }
    });
    let req: UpdateReq = coerce_and_validate(value).unwrap();
    let content = req.content.unwrap();
    assert_eq!(content["score"], "2.71");
    assert_eq!(content["count"], "42");
    assert_eq!(content["enabled"], "true");
    assert_eq!(content["metadata"], "{\"key\":\"value\"}");
    assert_eq!(content["tags"], "[\"tag1\",\"tag2\"]");
    assert_eq!(content["text"], "plain");
}

#[test]
fn test_coerce_keeps_typed_fields_intact() {
    // `dry_run` (bool), `code_reference` (object) and `updates` (array)
    // must survive the slow path untouched; only content values change.
    let value = json!({
        "session_id": 7,
        "dry_run": true,
        "code_reference": {"file": "main.rs", "line": 3},
        "updates": [
            {"interaction_type": "qa", "content": {"question": "q", "n": 1}},
            {"interaction_type": "code_change", "content": {"ok": false}}
        ]
    });
    let req: UpdateReq = coerce_and_validate(value).unwrap();
    assert_eq!(req.session_id, "7");
    assert_eq!(req.dry_run, Some(true));
    assert_eq!(
        req.code_reference,
        Some(json!({"file": "main.rs", "line": 3}))
    );
    let updates = req.updates.unwrap();
    assert_eq!(updates.len(), 2);
    assert_eq!(updates[0].interaction_type, "qa");
    assert_eq!(updates[0].content["n"], "1");
    assert_eq!(updates[1].content["ok"], "false");
}

#[test]
fn test_coerce_does_not_stringify_numeric_and_array_params() {
    // A numeric `scope_id` forces the slow path; `limit`,
    // `recency_bias` and the `interaction_type` array must stay typed.
    let value = json!({
        "query": "auth",
        "scope_id": 42,
        "limit": 5,
        "recency_bias": 0.5,
        "interaction_type": ["qa", "decision_made"]
    });
    let req: SearchReq = coerce_and_validate(value).unwrap();
    assert_eq!(req.query, "auth");
    assert_eq!(req.scope_id.as_deref(), Some("42"));
    assert_eq!(req.limit, Some(5));
    assert_eq!(req.recency_bias, Some(0.5));
    assert_eq!(
        req.interaction_type,
        Some(vec!["qa".to_string(), "decision_made".to_string()])
    );
}

#[test]
fn test_coerce_query_parameters_map() {
    let value = json!({"parameters": {"limit": 10, "topic": "x"}});
    #[derive(Deserialize)]
    struct QueryReq {
        parameters: HashMap<String, String>,
    }
    let req: QueryReq = coerce_and_validate(value).unwrap();
    assert_eq!(req.parameters["limit"], "10");
    assert_eq!(req.parameters["topic"], "x");
}

#[test]
fn test_coerce_unrelated_field_still_errors() {
    // Non-`_id`, non-content fields are not coerced.
    let value = json!({"query": 123});
    let result: Result<SearchReq, _> = coerce_and_validate(value);
    assert!(result.is_err());
}

#[test]
fn test_fast_path_string_passes_through() {
    let value = json!({"name": "test"});
    let result: HashMap<String, String> = coerce_and_validate(value).unwrap();
    assert_eq!(result.get("name"), Some(&"test".to_string()));
}

/// A failed argument is named from the typed error: the path serde
/// reports, or the required field the schema lists and the call lacks.
#[test]
fn a_failure_names_the_argument_at_fault() {
    use crate::tools::args::SessionRequest;
    let hints = [FieldHint {
        field: "action",
        expected: "one of: create",
        hint: "Use create",
    }];
    let missing = arguments::<SessionRequest>(json!({"name": "x"}), &hints).unwrap_err();
    assert_eq!(missing.details.parameter.as_deref(), Some("action"));
    assert_eq!(missing.details.hint.as_deref(), Some("Use create"));
    assert!(
        missing.message.contains("missing field `action`"),
        "{}",
        missing.message
    );
    let wrong =
        arguments::<SessionRequest>(json!({"action": "list", "seq": "x"}), &hints).unwrap_err();
    assert_eq!(wrong.details.parameter.as_deref(), Some("seq"));
    assert_eq!(wrong.details.received, Some(json!("x")));
    assert!(wrong.details.hint.is_none());
}
