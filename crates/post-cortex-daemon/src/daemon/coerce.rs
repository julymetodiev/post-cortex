// Copyright (c) 2025 Julius ML
// MIT License

//! Type coercion and validation for MCP tool parameters.
//!
//! This module provides flexible type coercion to help AI agents interact
//! with MCP tools correctly, even when they pass parameters with slightly
//! incorrect types (e.g., numbers instead of strings).

use serde::Deserialize;
use serde_json::Value;

pub use crate::daemon::validate::VALID_INTERACTION_TYPES;

/// Coerce and validate a JSON value into the target type.
///
/// This function attempts to deserialize the value directly first (fast path).
/// If that fails, it applies type coercion rules to fix common type mismatches
/// before attempting deserialization again.
///
/// # Type Coercion Rules
///
/// - Number → String: Converts integers and floats to their string representation
/// - Boolean → String: Converts "true"/"false" to strings
/// - Object/Array → JSON String: Serializes nested structures as JSON strings
///   (useful for `content` fields that expect stringified JSON)
///
/// # Example
///
/// ```rust
/// use serde_json::json;
///
/// #[derive(Deserialize)]
/// struct Request {
///     session_id: String,
///     count: String,
/// }
///
/// let value = json!({
///     "session_id": 123,  // Number instead of string
///     "count": 42         // Number instead of string
/// });
///
/// let req: Request = coerce_and_validate(value)?;
/// assert_eq!(req.session_id, "123");
/// assert_eq!(req.count, "42");
/// ```
pub fn coerce_and_validate<T: for<'de> Deserialize<'de>>(
    value: Value,
) -> Result<T, CoercionError> {
    // Fast path: try direct deserialization first
    if let Ok(result) = serde_json::from_value::<T>(value.clone()) {
        return Ok(result);
    }

    // Slow path: apply coercions and try again
    let coerced = apply_coercions(value)?;
    serde_json::from_value(coerced).map_err(|e| {
        CoercionError::new(
            "Failed to deserialize parameter(s)",
            e,
            None,
        )
    })
}

/// Apply type coercion rules to a JSON value.
///
/// This recursively walks through the value and applies coercion rules
/// to convert common type mismatches.
fn apply_coercions(mut value: Value) -> Result<Value, CoercionError> {
    if let Some(obj) = value.as_object_mut() {
        for (_key, val) in obj.iter_mut() {
            *val = coerce_value(val)?;
        }
    }
    Ok(value)
}

/// Coerce a single value according to the coercion rules.
fn coerce_value(val: &Value) -> Result<Value, CoercionError> {
    match val {
        // Number → String
        // Handles cases like: {"session_id": 123} → {"session_id": "123"}
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Ok(Value::String(i.to_string()))
            } else if let Some(f) = n.as_f64() {
                Ok(Value::String(f.to_string()))
            } else {
                Ok(val.clone())
            }
        }

        // Boolean → String
        // Handles cases like: {"enabled": true} → {"enabled": "true"}
        Value::Bool(b) => Ok(Value::String(b.to_string())),

        // Object → JSON String
        // Handles cases where nested objects need to be stringified
        // e.g., {"metadata": {"key": "value"}} → {"metadata": "{\"key\":\"value\"}"}
        Value::Object(obj) => {
            serde_json::to_string(obj)
                .map(Value::String)
                .map_err(|e| CoercionError::new(
                    "Failed to serialize object to JSON string",
                    e,
                    Some(val.clone()),
                ))
        }

        // Array → JSON String
        // Similar to object coercion, stringifies arrays
        Value::Array(arr) => {
            serde_json::to_string(arr)
                .map(Value::String)
                .map_err(|e| CoercionError::new(
                    "Failed to serialize array to JSON string",
                    e,
                    Some(val.clone()),
                ))
        }

        // String, Null, etc. pass through unchanged
        _ => Ok(val.clone()),
    }
}

/// Generate recovery suggestions based on error message patterns.
///
/// Analyzes common error patterns and provides actionable suggestions
/// for AI agents to fix their requests.
///
/// # Examples
///
/// ```rust
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

        if let Some(Value::String(s)) = received_value {
            if s.len() != 36 {
                suggestions.push(format!("Your session_id '{}' has {} characters, but UUIDs require exactly 36 characters with hyphens.", s, s.len()));
            }
        }
    }

    // Pattern: interaction_type validation
    if error_message.contains("interaction_type") || error_message.contains("Unknown interaction type") {
        suggestions.push(format!("Valid interaction_type values are: {}", VALID_INTERACTION_TYPES.join(", ")));
        suggestions.push("Use lowercase with underscores, not CamelCase or spaces".to_string());
        suggestions.push("Examples: ✅ 'decision_made' ❌ 'DecisionMade' ❌ 'made decision'".to_string());
    }

    // Pattern: content field errors
    if error_message.contains("content") && error_message.contains("required") {
        suggestions.push("For single update mode, provide both 'interaction_type' and 'content' parameters".to_string());
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
        suggestions.push("Check that all required parameters are included in your request".to_string());
        suggestions.push("Review the tool schema to see which parameters are required vs optional".to_string());
    }

    // Pattern: Session not found
    if error_message.contains("Session not found") || error_message.contains("session does not exist") {
        suggestions.push("Create a new session using the 'session' tool with action='create'".to_string());
        suggestions.push("Or use semantic_search to find existing sessions".to_string());
    }

    // Pattern: Array/structure errors
    if error_message.contains("updates") && (error_message.contains("array") || error_message.contains("expected length")) {
        suggestions.push("When using bulk mode, 'updates' must be an array of update objects".to_string());
        suggestions.push("Each update in the array must have 'interaction_type' and 'content' fields".to_string());
    }

    // General fallback suggestions
    if suggestions.is_empty() {
        suggestions.push("Review the error message and check your parameter types and values".to_string());
        suggestions.push("Use dry_run=true to validate your request without making changes".to_string());
        suggestions.push("Check the tool documentation for the correct parameter format".to_string());
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
    /// Path to the parameter that failed (e.g., "session_id", "updates[0].interaction_type")
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

    #[test]
    fn test_coerce_float_to_string() {
        let value = json!({"score": 3.14});
        let result: HashMap<String, String> = coerce_and_validate(value).unwrap();
        assert_eq!(result.get("score"), Some(&"3.14".to_string()));
    }

    #[test]
    fn test_coerce_bool_to_string() {
        let value = json!({"enabled": true});
        let result: HashMap<String, String> = coerce_and_validate(value).unwrap();
        assert_eq!(result.get("enabled"), Some(&"true".to_string()));
    }

    #[test]
    fn test_coerce_object_to_json_string() {
        let value = json!({"metadata": {"key": "value"}});
        let result: HashMap<String, String> = coerce_and_validate(value).unwrap();
        assert_eq!(result.get("metadata"), Some(&"{\"key\":\"value\"}".to_string()));
    }

    #[test]
    fn test_coerce_array_to_json_string() {
        let value = json!({"tags": ["tag1", "tag2"]});
        let result: HashMap<String, String> = coerce_and_validate(value).unwrap();
        assert_eq!(result.get("tags"), Some(&"[\"tag1\",\"tag2\"]".to_string()));
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
        assert!(suggestions.iter().any(|s| s.contains("lowercase with underscores")));
    }

    #[test]
    fn test_recovery_suggestions_type_error() {
        let suggestions = generate_recovery_suggestions(
            "invalid type: integer `123`, expected a string",
            Some("session_id"),
            Some(&json!(123)),
        );

        assert!(suggestions.iter().any(|s| s.contains("Convert the number 123")));
    }

    #[test]
    fn test_recovery_suggestions_content_required() {
        let suggestions = generate_recovery_suggestions(
            "content is required",
            Some("content"),
            None,
        );

        assert!(suggestions.iter().any(|s| s.contains("interaction_type")));
        assert!(suggestions.iter().any(|s| s.contains("bulk updates")));
    }

    #[test]
    fn test_recovery_suggestions_session_not_found() {
        let suggestions = generate_recovery_suggestions(
            "Session not found",
            None,
            None,
        );

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
        let suggestions = generate_recovery_suggestions(
            "Some unknown error",
            None,
            None,
        );

        assert!(suggestions.iter().any(|s| s.contains("dry_run")));
        assert!(suggestions.iter().any(|s| s.contains("parameter types")));
    }
}
