// Copyright (c) 2025 Julius ML
// MIT License

//! Type coercion and validation for MCP tool parameters.
//!
//! This module provides flexible type coercion to help AI agents interact
//! with MCP tools correctly, even when they pass parameters with slightly
//! incorrect types (e.g., numbers instead of strings).

use serde::Deserialize;
use serde_json::Value;

/// Valid interaction type values accepted by MCP tools.
pub use crate::daemon::validate::VALID_INTERACTION_TYPES;

/// Coerce and validate a JSON value into the target type.
///
/// This function attempts to deserialize the value directly first (fast path,
/// borrowing the value — no clone). If that fails, it applies targeted type
/// coercion rules to fix common type mismatches before deserializing again.
///
/// # Type Coercion Rules
///
/// Coercion is deliberately narrow so typed fields (`limit`, `dry_run`,
/// `updates: [...]`, `select_map`, ...) are never rewritten:
///
/// - `content` maps (top-level and inside each `updates[]` item) and the
///   query tool's `parameters` map are `HashMap<String, String>`, so every
///   non-string value in them becomes a
///   string: numbers/booleans via `to_string`, objects/arrays as compact JSON.
/// - Top-level `*_id` fields (`session_id`, `workspace_id`, `scope_id`,
///   `entry_id`) are always strings, so a number/boolean there becomes its
///   string form.
///
/// # Example
///
/// ```rust,ignore
/// use serde_json::json;
///
/// let value = json!({
///     "session_id": 123,                 // Number instead of string
///     "content": {"score": 42, "ok": true}, // Non-string content values
///     "limit": 5                          // Left as a number
/// });
///
/// let req: Request = coerce_and_validate(value)?;
/// assert_eq!(req.session_id, "123");
/// ```
pub fn coerce_and_validate<T: for<'de> Deserialize<'de>>(value: Value) -> Result<T, CoercionError> {
    // Fast path: deserialize straight from the borrowed value.
    if let Ok(result) = T::deserialize(&value) {
        return Ok(result);
    }

    // Slow path: apply coercions and try again
    let coerced = apply_coercions(value)?;
    T::deserialize(coerced)
        .map_err(|e| CoercionError::new("Failed to deserialize parameter(s)", e, None))
}

/// Apply the targeted coercion rules (see [`coerce_and_validate`]) in place.
fn apply_coercions(mut value: Value) -> Result<Value, CoercionError> {
    let Some(obj) = value.as_object_mut() else {
        return Ok(value);
    };

    for (key, val) in obj.iter_mut() {
        match key.as_str() {
            // Both are `HashMap<String, String>` in the request types.
            "content" | "parameters" => coerce_string_map(val)?,
            "updates" => {
                if let Value::Array(items) = val {
                    for item in items {
                        if let Some(content) = item.get_mut("content") {
                            coerce_string_map(content)?;
                        }
                    }
                }
            }
            k if k.ends_with("_id") => coerce_scalar_to_string(val),
            _ => {}
        }
    }
    Ok(value)
}

/// Stringify every non-string value of a `HashMap<String, String>`-shaped object.
fn coerce_string_map(val: &mut Value) -> Result<(), CoercionError> {
    if let Value::Object(map) = val {
        for v in map.values_mut() {
            coerce_to_string(v)?;
        }
    }
    Ok(())
}

/// Number/Boolean → String; everything else is left untouched.
fn coerce_scalar_to_string(val: &mut Value) {
    match val {
        Value::Number(n) => *val = Value::String(n.to_string()),
        Value::Bool(b) => *val = Value::String(b.to_string()),
        _ => {}
    }
}

/// Number/Boolean → String, Object/Array → compact JSON string. Strings and
/// nulls pass through unchanged.
fn coerce_to_string(val: &mut Value) -> Result<(), CoercionError> {
    match val {
        Value::Number(_) | Value::Bool(_) => coerce_scalar_to_string(val),
        Value::Object(_) | Value::Array(_) => {
            let json = serde_json::to_string(val).map_err(|e| {
                CoercionError::new(
                    "Failed to serialize nested value to JSON string",
                    e,
                    Some(val.clone()),
                )
            })?;
            *val = Value::String(json);
        }
        Value::String(_) | Value::Null => {}
    }
    Ok(())
}

/// Generate recovery suggestions based on error message patterns.
///
/// Analyzes common error patterns and provides actionable suggestions
/// for AI agents to fix their requests.
///
/// # Examples
///
/// ```rust,ignore
/// let error_msg = "invalid type: string 'abc', expected u32";
/// let suggestions = generate_recovery_suggestions(error_msg);
/// // Returns: ["Ensure the value is a valid number", "Check for typos in the value"]
/// ```
pub fn generate_recovery_suggestions(
    error_message: &str,
    parameter_path: Option<&str>,
    received_value: Option<&Value>,
) -> Vec<String> {
    let mut suggestions = Vec::new();

    // Pattern: UUID validation errors
    if error_message.contains("UUID") || error_message.contains("36-character") {
        suggestions.push("Ensure the session_id is a valid 36-character UUID (e.g., '60c598e2-d602-4e07-a328-c458006d48c7')".to_string());
        suggestions.push("Create a new session using the 'session' tool with action='create' to get a valid UUID".to_string());

        if let Some(Value::String(s)) = received_value
            && s.len() != 36
        {
            suggestions.push(format!("Your session_id '{}' has {} characters, but UUIDs require exactly 36 characters with hyphens.", s, s.len()));
        }
    }

    // Pattern: interaction_type validation
    if error_message.contains("interaction_type")
        || error_message.contains("Unknown interaction type")
    {
        suggestions.push(format!(
            "Valid interaction_type values are: {}",
            VALID_INTERACTION_TYPES.join(", ")
        ));
        suggestions.push("Use lowercase with underscores, not CamelCase or spaces".to_string());
        suggestions
            .push("Examples: ✅ 'decision_made' ❌ 'DecisionMade' ❌ 'made decision'".to_string());
    }

    // Pattern: content field errors
    if error_message.contains("content") && error_message.contains("required") {
        suggestions.push(
            "For single update mode, provide both 'interaction_type' and 'content' parameters"
                .to_string(),
        );
        suggestions.push("For bulk updates, use 'updates' array instead".to_string());
        suggestions.push("Content must be an object with key-value pairs".to_string());
    }

    // Pattern: Type coercion errors
    if error_message.contains("invalid type") || error_message.contains("expected") {
        if let Some(path) = parameter_path {
            suggestions.push(format!("Parameter '{}' has an incorrect type", path));
        }

        // Check if value is a number when string expected
        if let Some(Value::Number(n)) = received_value {
            suggestions.push(format!("Convert the number {} to a string", n));
        }

        // Check if value is boolean when string expected
        if let Some(Value::Bool(b)) = received_value {
            suggestions.push(format!("Convert the boolean {} to a string ('{}')", b, b));
        }
    }

    // Pattern: Missing required parameters
    if error_message.contains("required") || error_message.contains("missing") {
        suggestions
            .push("Check that all required parameters are included in your request".to_string());
        suggestions.push(
            "Review the tool schema to see which parameters are required vs optional".to_string(),
        );
    }

    // Pattern: Session not found
    if error_message.contains("Session not found")
        || error_message.contains("session does not exist")
    {
        suggestions
            .push("Create a new session using the 'session' tool with action='create'".to_string());
        suggestions.push("Or use semantic_search to find existing sessions".to_string());
    }

    // Pattern: Array/structure errors
    if error_message.contains("updates")
        && (error_message.contains("array") || error_message.contains("expected length"))
    {
        suggestions
            .push("When using bulk mode, 'updates' must be an array of update objects".to_string());
        suggestions.push(
            "Each update in the array must have 'interaction_type' and 'content' fields"
                .to_string(),
        );
    }

    // General fallback suggestions
    if suggestions.is_empty() {
        suggestions
            .push("Review the error message and check your parameter types and values".to_string());
        suggestions
            .push("Use dry_run=true to validate your request without making changes".to_string());
        suggestions
            .push("Check the tool documentation for the correct parameter format".to_string());
    }

    suggestions
}

/// Structured error type for coercion failures.
///
/// Provides rich error information to help AI agents understand
/// what went wrong and how to fix it.
#[derive(Debug, Clone)]
pub struct CoercionError {
    /// Human-readable error message
    pub message: String,
    /// Path to the parameter that failed (e.g., "session_id", "updates\[0\].interaction_type")
    pub parameter_path: Option<String>,
    /// Expected type description (e.g., "UUID string", "one of: qa, decision_made, ...")
    pub expected_type: Option<String>,
    /// The actual value that was received
    pub received_value: Option<Value>,
    /// Actionable hint for fixing the error
    pub hint: Option<String>,
}

impl CoercionError {
    /// Create a new coercion error.
    pub fn new(
        message: &str,
        source_error: impl std::error::Error,
        received_value: Option<Value>,
    ) -> Self {
        Self {
            message: format!("{}: {}", message, source_error),
            parameter_path: None,
            expected_type: None,
            received_value,
            hint: None,
        }
    }

    /// Set the parameter path for this error.
    pub fn with_parameter_path(mut self, path: String) -> Self {
        self.parameter_path = Some(path);
        self
    }

    /// Set the expected type description for this error.
    pub fn with_expected_type(mut self, type_desc: &str) -> Self {
        self.expected_type = Some(type_desc.to_string());
        self
    }

    /// Set a hint for fixing this error.
    pub fn with_hint(mut self, hint: &str) -> Self {
        self.hint = Some(hint.to_string());
        self
    }

    /// Convert this error to an MCP error response.
    ///
    /// Creates a structured JSON error that agents can parse and understand.
    /// Includes automatically generated recovery suggestions.
    pub fn to_mcp_error(&self) -> rmcp::model::ErrorData {
        let mut details = serde_json::json!({
            "message": self.message,
        });

        if let Some(path) = &self.parameter_path {
            details["parameter"] = serde_json::json!(path);
        }

        if let Some(expected) = &self.expected_type {
            details["expectedType"] = serde_json::json!(expected);
        }

        if let Some(received) = &self.received_value {
            details["receivedValue"] = received.clone();
        }

        if let Some(hint) = &self.hint {
            details["hint"] = serde_json::json!(hint);
        }

        // Generate and include recovery suggestions
        let suggestions = generate_recovery_suggestions(
            &self.message,
            self.parameter_path.as_deref(),
            self.received_value.as_ref(),
        );

        if !suggestions.is_empty() {
            details["suggestions"] = serde_json::json!(suggestions);
        }

        rmcp::model::ErrorData::invalid_params(
            serde_json::to_string(&details).unwrap_or_default(),
            None,
        )
    }
}

impl std::fmt::Display for CoercionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)?;
        if let Some(path) = &self.parameter_path {
            write!(f, " (parameter: {})", path)?;
        }
        if let Some(hint) = &self.hint {
            write!(f, "\nHint: {}", hint)?;
        }
        Ok(())
    }
}

impl std::error::Error for CoercionError {}

#[cfg(test)]
mod tests {
    use super::*;
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

    #[test]
    fn test_coercion_error_with_path() {
        let error = CoercionError::new(
            "Test error",
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "invalid"),
            Some(json!(123)),
        )
        .with_parameter_path("session_id".to_string())
        .with_expected_type("UUID string")
        .with_hint("Create a session first");

        assert_eq!(error.parameter_path, Some("session_id".to_string()));
        assert_eq!(error.expected_type, Some("UUID string".to_string()));
        assert_eq!(error.hint, Some("Create a session first".to_string()));
    }

    #[test]
    fn test_to_mcp_error_format() {
        let error = CoercionError::new(
            "Invalid parameter",
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "test"),
            Some(json!(123)),
        )
        .with_parameter_path("session_id".to_string())
        .with_expected_type("UUID string")
        .with_hint("Use session tool to create");

        let mcp_error = error.to_mcp_error();
        // The to_mcp_error() function serializes the error details into the message
        // The second parameter to invalid_params (data) is None in our implementation
        // So we verify the message contains our structured data
        let error_message = mcp_error.message;
        assert!(error_message.contains("session_id"));

        // Parse the message as JSON to verify structure
        let error_data: serde_json::Value = serde_json::from_str(&error_message).unwrap();
        assert_eq!(error_data["parameter"], "session_id");
        assert_eq!(error_data["expectedType"], "UUID string");
        assert_eq!(error_data["receivedValue"], 123);
        assert_eq!(error_data["hint"], "Use session tool to create");
    }

    #[test]
    fn test_recovery_suggestions_uuid_error() {
        let suggestions = generate_recovery_suggestions(
            "Invalid UUID format",
            Some("session_id"),
            Some(&json!("abc")),
        );

        assert!(suggestions.iter().any(|s| s.contains("36-character UUID")));
        assert!(suggestions.iter().any(|s| s.contains("'session' tool")));
    }

    #[test]
    fn test_recovery_suggestions_interaction_type_error() {
        let suggestions = generate_recovery_suggestions(
            "Unknown interaction type",
            Some("interaction_type"),
            Some(&json!("made_decision")),
        );

        assert!(suggestions.iter().any(|s| s.contains("decision_made")));
        assert!(
            suggestions
                .iter()
                .any(|s| s.contains("lowercase with underscores"))
        );
    }

    #[test]
    fn test_recovery_suggestions_type_error() {
        let suggestions = generate_recovery_suggestions(
            "invalid type: integer `123`, expected a string",
            Some("session_id"),
            Some(&json!(123)),
        );

        assert!(
            suggestions
                .iter()
                .any(|s| s.contains("Convert the number 123"))
        );
    }

    #[test]
    fn test_recovery_suggestions_content_required() {
        let suggestions =
            generate_recovery_suggestions("content is required", Some("content"), None);

        assert!(suggestions.iter().any(|s| s.contains("interaction_type")));
        assert!(suggestions.iter().any(|s| s.contains("bulk updates")));
    }

    #[test]
    fn test_recovery_suggestions_session_not_found() {
        let suggestions = generate_recovery_suggestions("Session not found", None, None);

        assert!(suggestions.iter().any(|s| s.contains("'session' tool")));
        assert!(suggestions.iter().any(|s| s.contains("semantic_search")));
    }

    #[test]
    fn test_recovery_suggestions_includes_suggestions_in_mcp_error() {
        let error = CoercionError::new(
            "Invalid UUID format",
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "invalid"),
            Some(json!("short-id")),
        )
        .with_parameter_path("session_id".to_string());

        let mcp_error = error.to_mcp_error();
        let error_data: serde_json::Value = serde_json::from_str(&mcp_error.message).unwrap();

        // Verify suggestions array exists
        assert!(error_data["suggestions"].is_array());
        let suggestions = error_data["suggestions"].as_array().unwrap();
        assert!(!suggestions.is_empty());
    }

    #[test]
    fn test_recovery_suggestions_general_fallback() {
        let suggestions = generate_recovery_suggestions("Some unknown error", None, None);

        assert!(suggestions.iter().any(|s| s.contains("dry_run")));
        assert!(suggestions.iter().any(|s| s.contains("parameter types")));
    }
}
