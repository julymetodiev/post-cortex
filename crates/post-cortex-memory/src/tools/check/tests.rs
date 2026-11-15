// Copyright (c) 2025, 2026 Julius ML
// MIT License

use super::*;

#[test]
fn test_validate_session_id_valid() {
    assert!(session_id("60c598e2-d602-4e07-a328-c458006d48c7").is_ok());
}

#[test]
fn test_validate_session_id_invalid() {
    let result = session_id("invalid");
    assert!(result.is_err());
    let error = result.unwrap_err();
    assert_eq!(error.details.parameter, Some("session_id".to_string()));
    assert!(error.details.hint.is_some());
}

#[test]
fn test_validate_workspace_id_valid() {
    assert!(workspace_id("60c598e2-d602-4e07-a328-c458006d48c7").is_ok());
}

#[test]
fn test_validate_workspace_id_invalid() {
    let result = workspace_id("not-a-uuid");
    assert!(result.is_err());
    let error = result.unwrap_err();
    assert_eq!(error.details.parameter, Some("workspace_id".to_string()));
}

#[test]
fn test_validate_scope_valid() {
    assert!(scope("session").is_ok());
    assert!(scope("workspace").is_ok());
    assert!(scope("global").is_ok());
}

#[test]
fn test_validate_scope_invalid() {
    let result = scope("invalid_scope");
    assert!(result.is_err());
    let error = result.unwrap_err();
    assert_eq!(error.details.parameter, Some("scope".to_string()));
}

#[test]
fn test_validate_session_role_valid() {
    assert!(role("primary").is_ok());
    assert!(role("related").is_ok());
    assert!(role("dependency").is_ok());
    assert!(role("shared").is_ok());
}

#[test]
fn test_validate_session_role_invalid() {
    let result = role("admin");
    assert!(result.is_err());
    let error = result.unwrap_err();
    assert_eq!(error.details.parameter, Some("role".to_string()));
    assert!(error.details.hint.unwrap().contains("primary"));
}

#[test]
fn test_validate_limits_within_bounds() {
    assert_eq!(limit("limit", Some(5), 10, 100).unwrap(), 5);
    assert_eq!(limit("limit", Some(10), 10, 100).unwrap(), 10);
    assert_eq!(limit("limit", Some(100), 10, 100).unwrap(), 100);
}

#[test]
fn test_validate_limits_use_default() {
    assert_eq!(limit("limit", None, 10, 100).unwrap(), 10);
    assert_eq!(limit("limit", None, 50, 100).unwrap(), 50);
}

#[test]
fn test_validate_limits_exceeds_maximum() {
    let result = limit("limit", Some(200), 10, 100);
    assert!(result.is_err());
    let error = result.unwrap_err();
    assert_eq!(error.details.parameter, Some("limit".to_string()));
    assert!(error.message.contains("exceeds maximum"));
}

#[test]
fn test_validate_limits_zero() {
    let result = limit("limit", Some(0), 10, 100);
    assert!(result.is_err());
    let error = result.unwrap_err();
    assert!(error.message.contains("at least 1"));
}

#[test]
fn test_validate_recency_bias_valid() {
    // Valid values
    assert_eq!(recency_bias(Some(0.0)).unwrap(), Some(0.0));
    assert_eq!(recency_bias(Some(0.5)).unwrap(), Some(0.5));
    assert_eq!(recency_bias(Some(1.0)).unwrap(), Some(1.0));
    assert_eq!(recency_bias(Some(10.0)).unwrap(), Some(10.0));
}

#[test]
fn test_validate_recency_bias_none() {
    assert_eq!(recency_bias(None).unwrap(), None);
}

#[test]
fn test_validate_recency_bias_negative() {
    let result = recency_bias(Some(-1.0));
    assert!(result.is_err());
    let error = result.unwrap_err();
    assert_eq!(error.details.parameter, Some("recency_bias".to_string()));
    assert!(error.message.contains("out of range"));
}

#[test]
fn test_validate_recency_bias_nan() {
    let result = recency_bias(Some(f32::NAN));
    assert!(result.is_err());
    let error = result.unwrap_err();
    assert_eq!(error.details.parameter, Some("recency_bias".to_string()));
    assert!(error.message.contains("Invalid recency_bias value"));
}

#[test]
fn test_validate_recency_bias_infinity() {
    let result = recency_bias(Some(f32::INFINITY));
    assert!(result.is_err());
    let error = result.unwrap_err();
    assert!(error.message.contains("NaN or Infinity not allowed"));
}

#[test]
fn test_validate_recency_bias_exceeds_maximum() {
    let result = recency_bias(Some(100.0));
    assert!(result.is_err());
    let error = result.unwrap_err();
    assert!(error.message.contains("out of range"));
    assert_eq!(error.details.parameter, Some("recency_bias".to_string()));
}

/// Suggestions follow the argument and the problem, never the text of the
/// error (review W5 item 9).
#[test]
fn suggestions_follow_the_argument_and_the_problem() {
    let uuid = suggestions("entry_id", Problem::BadUuid, Some(&"x".into()));
    assert!(uuid[0].contains("36-character UUID"), "{uuid:?}");
    assert!(uuid[2].contains("has 1 characters"), "{uuid:?}");
    let missing = suggestions("content", Problem::Missing, None);
    assert!(
        missing.iter().any(|s| s.contains("key-value pairs")),
        "{missing:?}"
    );
    // A message that merely mentions "UUID" gives no UUID advice.
    let fallback = suggestions("scope", Problem::NotOneOf, None);
    assert!(
        fallback[0].starts_with("Review the error message"),
        "{fallback:?}"
    );
}
