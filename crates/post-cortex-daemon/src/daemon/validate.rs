// Copyright (c) 2025 Julius ML
// MIT License

//! Custom validation layer for MCP tool parameters.
//!
//! This module provides business logic validation for tool parameters,
//! complementing the type coercion layer in `coerce.rs`.
//!
//! Validations include:
//! - UUID format validation
//! - Enum value validation (interaction_type, scope, etc.)
//! - Numeric limit validation
//! - Business rule validation

use crate::daemon::coerce::CoercionError;
use post_cortex_core::core::context_update::CodeReference;
use uuid::Uuid;

/// Validate a session ID is a valid UUID and return the parsed value.
///
/// # Arguments
///
/// * `session_id` - The session ID string to validate
///
/// # Returns
///
/// * `Ok(Uuid)` - The parsed UUID if valid
/// * `Err(CoercionError)` with helpful message if invalid
///
/// # Example
///
/// ```rust,ignore
/// use post_cortex::daemon::validate::validate_session_id;
/// use uuid::Uuid;
///
/// // Valid UUID
/// assert!(validate_session_id("60c598e2-d602-4e07-a328-c458006d48c7").is_ok());
///
/// // Invalid UUID
/// assert!(validate_session_id("invalid").is_err());
/// ```
pub fn validate_session_id(session_id: &str) -> Result<Uuid, CoercionError> {
    Uuid::parse_str(session_id).map_err(|_| CoercionError::new(
        &format!("Invalid UUID format: '{}'", session_id),
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "Invalid UUID"),
        Some(session_id.into()),
    )
    .with_parameter_path("session_id".to_string())
    .with_expected_type("UUID format: xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx (36 chars with hyphens)")
    .with_hint("Create a session first using the 'session' tool with action='create', or search for existing sessions using 'semantic_search'"))
}

/// Validate a workspace ID is a valid UUID and return the parsed value.
///
/// # Arguments
///
/// * `workspace_id` - The workspace ID string to validate
///
/// # Returns
///
/// * `Ok(Uuid)` - The parsed UUID if valid
/// * `Err(CoercionError)` with helpful message if invalid
pub fn validate_workspace_id(workspace_id: &str) -> Result<Uuid, CoercionError> {
    Uuid::parse_str(workspace_id).map_err(|_| CoercionError::new(
        &format!("Invalid workspace ID format: '{}'", workspace_id),
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "Invalid UUID"),
        Some(workspace_id.into()),
    )
    .with_parameter_path("workspace_id".to_string())
    .with_expected_type("UUID format: xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx (36 chars with hyphens)")
    .with_hint("Use the 'manage_workspace' tool with action='list' to see available workspaces, or create one with action='create'"))
}

/// Valid interaction type values
pub const VALID_INTERACTION_TYPES: &[&str] = &[
    "qa",
    "decision_made",
    "problem_solved",
    "code_change",
    "requirement_added",
    "concept_defined",
];

/// Validate an interaction type is one of the valid values.
///
/// # Arguments
///
/// * `interaction_type` - The interaction type string to validate
///
/// # Valid Values
///
/// - `qa`: Questions and answers about codebase
/// - `decision_made`: Architectural decisions and trade-offs
/// - `problem_solved`: Bug fixes and technical solutions
/// - `code_change`: Code modifications and refactoring
/// - `requirement_added`: New requirements or constraints
/// - `concept_defined`: Technical concepts and patterns explained
///
/// # Returns
///
/// * `Ok(())` if the interaction type is valid
/// * `Err(CoercionError)` with helpful message if invalid
pub fn validate_interaction_type(interaction_type: &str) -> Result<(), CoercionError> {
    if VALID_INTERACTION_TYPES.contains(&interaction_type.to_lowercase().as_str()) {
        Ok(())
    } else {
        Err(CoercionError::new(
            &format!("Invalid interaction_type: '{}'", interaction_type),
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "Invalid type"),
            Some(interaction_type.into()),
        )
        .with_parameter_path("interaction_type".to_string())
        .with_expected_type(&format!("one of: {}", VALID_INTERACTION_TYPES.join(", ")))
        .with_hint("Use exact lowercase term with underscores. Valid types: qa, decision_made, problem_solved, code_change, requirement_added, concept_defined"))
    }
}

/// Validate a scope value for semantic search.
///
/// # Arguments
///
/// * `scope` - The scope string to validate
///
/// # Valid Values
///
/// - `session`: Search within a specific session (requires scope_id)
/// - `workspace`: Search within a workspace (requires scope_id)
/// - `global`: Search across all data (default)
///
/// # Returns
///
/// * `Ok(())` if the scope is valid
/// * `Err(CoercionError)` with helpful message if invalid
pub fn validate_scope(scope: &str) -> Result<(), CoercionError> {
    const VALID_SCOPES: &[&str] = &["session", "workspace", "global"];

    if VALID_SCOPES.contains(&scope.to_lowercase().as_str()) {
        Ok(())
    } else {
        Err(CoercionError::new(
            &format!("Invalid scope: '{}'", scope),
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "Invalid scope"),
            Some(scope.into()),
        )
        .with_parameter_path("scope".to_string())
        .with_expected_type(&format!("one of: {}", VALID_SCOPES.join(", ")))
        .with_hint("Valid scopes: 'session' (requires scope_id), 'workspace' (requires scope_id), 'global' (default, no scope_id needed)"))
    }
}

/// Validate a session action.
///
/// # Arguments
///
/// * `action` - The action string to validate
///
/// # Valid Values
///
/// - `create`: Create a new session
/// - `list`: List all sessions
///
/// # Returns
///
/// * `Ok(())` if the action is valid
/// * `Err(CoercionError)` with helpful message if invalid
pub fn validate_session_action(action: &str) -> Result<(), CoercionError> {
    const VALID_ACTIONS: &[&str] = &[
        "create",
        "list",
        "load",
        "search",
        "update_metadata",
        "delete",
    ];

    if VALID_ACTIONS.contains(&action.to_lowercase().as_str()) {
        Ok(())
    } else {
        Err(CoercionError::new(
            &format!("Invalid action: '{}'", action),
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "Invalid action"),
            Some(action.into()),
        )
        .with_parameter_path("action".to_string())
        .with_expected_type(&format!("one of: {}", VALID_ACTIONS.join(", ")))
        .with_hint("Valid actions: create (name, description), list, load (session_id), search (query), update_metadata (session_id + name/description), delete (session_id)"))
    }
}

/// Validate a workspace action.
///
/// # Arguments
///
/// * `action` - The action string to validate
///
/// # Valid Values
///
/// - `create`: Create a new workspace
/// - `list`: List all workspaces
/// - `get`: Get workspace details
/// - `delete`: Delete a workspace
/// - `add_session`: Add a session to a workspace
/// - `remove_session`: Remove a session from a workspace
///
/// # Returns
///
/// * `Ok(())` if the action is valid
/// * `Err(CoercionError)` with helpful message if invalid
pub fn validate_workspace_action(action: &str) -> Result<(), CoercionError> {
    const VALID_ACTIONS: &[&str] = &[
        "create",
        "list",
        "get",
        "delete",
        "add_session",
        "remove_session",
        "get_card",
        "update_card",
    ];

    if VALID_ACTIONS.contains(&action.to_lowercase().as_str()) {
        Ok(())
    } else {
        Err(CoercionError::new(
            &format!("Invalid action: '{}'", action),
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "Invalid action"),
            Some(action.into()),
        )
        .with_parameter_path("action".to_string())
        .with_expected_type(&format!("one of: {}", VALID_ACTIONS.join(", ")))
        .with_hint("Valid actions: create (with name/description), list, get (workspace_id), delete (workspace_id), add_session (workspace_id, session_id, role), remove_session (workspace_id, session_id)"))
    }
}

/// Validate a session role for workspace membership.
///
/// # Arguments
///
/// * `role` - The role string to validate
///
/// # Valid Values
///
/// - `primary`: Primary session for the workspace
/// - `related`: Related session
/// - `dependency`: Dependency session
/// - `shared`: Shared session
///
/// # Returns
///
/// * `Ok(())` if the role is valid
/// * `Err(CoercionError)` with helpful message if invalid
pub fn validate_session_role(role: &str) -> Result<(), CoercionError> {
    const VALID_ROLES: &[&str] = &["primary", "related", "dependency", "shared"];

    if VALID_ROLES.contains(&role.to_lowercase().as_str()) {
        Ok(())
    } else {
        Err(CoercionError::new(
            &format!("Invalid role: '{}'", role),
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "Invalid role"),
            Some(role.into()),
        )
        .with_parameter_path("role".to_string())
        .with_expected_type(&format!("one of: {}", VALID_ROLES.join(", ")))
        .with_hint("Valid session roles: primary (main session), related (related context), dependency (required context), shared (shared context)"))
    }
}

/// Validate a recency_bias parameter is within acceptable bounds.
///
/// The recency_bias parameter controls temporal decay in semantic search. Invalid values
/// can cause exponential growth (negatives), underflow to zero (extreme values), or
/// corrupt rankings (NaN, Infinity).
///
/// # Arguments
///
/// * `recency_bias` - The recency_bias value to validate (None means disabled)
///
/// # Returns
///
/// * `Ok(Some(value))` - The validated recency_bias value
/// * `Ok(None)` - If recency_bias was None (disabled)
/// * `Err(CoercionError)` with helpful message if invalid
///
/// # Valid Range
///
/// - `[0.0, 10.0]` - 0.0 disables decay, 10.0 is maximum practical decay rate
/// - Rejects: negative values, NaN, Infinity, NegInfinity
///
/// # Example
///
/// ```rust,ignore
/// use post_cortex::daemon::validate::validate_recency_bias;
///
/// // Valid values
/// assert_eq!(validate_recency_bias(Some(0.0)).unwrap(), Some(0.0));
/// assert_eq!(validate_recency_bias(Some(0.5)).unwrap(), Some(0.5));
/// assert_eq!(validate_recency_bias(Some(10.0)).unwrap(), Some(10.0));
/// assert_eq!(validate_recency_bias(None).unwrap(), None);
///
/// // Invalid values
/// assert!(validate_recency_bias(Some(-1.0)).is_err());
/// assert!(validate_recency_bias(Some(f32::NAN)).is_err());
/// assert!(validate_recency_bias(Some(f32::INFINITY)).is_err());
/// ```
pub fn validate_recency_bias(recency_bias: Option<f32>) -> Result<Option<f32>, CoercionError> {
    const MAX_RECENCY_BIAS: f32 = 10.0;
    const MIN_RECENCY_BIAS: f32 = 0.0;

    if let Some(value) = recency_bias {
        if value.is_nan() || value.is_infinite() {
            return Err(CoercionError::new(
                "Invalid recency_bias value",
                std::io::Error::new(std::io::ErrorKind::InvalidInput, "NaN or Infinity not allowed"),
                Some(serde_json::Value::String(value.to_string())),
            )
            .with_parameter_path("recency_bias".to_string())
            .with_expected_type("finite f32 between 0.0 and 10.0")
            .with_hint("Use a finite value between 0.0 (disabled) and 10.0 (aggressive decay). Recommended: 0.0-1.0 for most use cases."));
        }
        if !(MIN_RECENCY_BIAS..=MAX_RECENCY_BIAS).contains(&value) {
            return Err(CoercionError::new(
                "recency_bias out of range",
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "Value must be between 0.0 and 10.0",
                ),
                Some(serde_json::Value::String(value.to_string())),
            )
            .with_parameter_path("recency_bias".to_string())
            .with_expected_type("f32 in range [0.0, 10.0]")
            .with_hint(&format!(
                "Use recency_bias between {} and {}, or omit for default (0.0 = disabled)",
                MIN_RECENCY_BIAS, MAX_RECENCY_BIAS
            )));
        }
        Ok(Some(value))
    } else {
        Ok(None)
    }
}

/// Validate a numeric limit is within acceptable bounds.
///
/// This function validates that limit parameters are within safe ranges to prevent
/// excessive resource consumption while maintaining flexibility for legitimate use cases.
///
/// # Arguments
///
/// * `limit` - The limit value to validate (None means use default)
/// * `default` - The default limit value (used when limit is None)
/// * `max` - The maximum allowed limit value
///
/// # Returns
///
/// * `Ok(limit_value)` - The limit to use (either the provided value or default)
/// * `Err(CoercionError)` with helpful message if limit exceeds maximum or is zero
///
/// # Example
///
/// ```rust,ignore
/// use post_cortex::daemon::validate::validate_limits;
///
/// // Valid limit
/// assert_eq!(validate_limits(Some(5), 10, 100).unwrap(), 5);
///
/// // Use default
/// assert_eq!(validate_limits(None, 10, 100).unwrap(), 10);
///
/// // Exceeds maximum
/// assert!(validate_limits(Some(200), 10, 100).is_err());
/// ```
pub fn validate_limits(
    limit: Option<usize>,
    default: usize,
    max: usize,
) -> Result<usize, CoercionError> {
    let value = limit.unwrap_or(default);
    if value > max {
        Err(CoercionError::new(
            &format!("Limit {} exceeds maximum of {}", value, max),
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "Limit too large"),
            Some(value.into()),
        )
        .with_parameter_path("limit".to_string())
        .with_expected_type(&format!("number between 1 and {}", max))
        .with_hint(&format!(
            "Use limit between 1 and {}, or omit for default ({})",
            max, default
        )))
    } else if value == 0 {
        Err(CoercionError::new(
            "Limit must be at least 1",
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "Limit too small"),
            Some(value.into()),
        )
        .with_parameter_path("limit".to_string())
        .with_expected_type(&format!("number between 1 and {}", max))
        .with_hint(&format!(
            "Use limit between 1 and {}, or omit for default ({})",
            max, default
        )))
    } else {
        Ok(value)
    }
}

/// Parse an optional `code_reference` parameter.
///
/// Accepts the forms the tool schema documents, plus the canonical struct:
/// - `null` / absent → `None`
/// - `"src/main.rs"`, `"src/main.rs:42"`, `"src/main.rs:10-20"`
/// - `{"file": "src/main.rs", "line": 42, "function": "process_data"}`
///   (`file_path`, `start_line`, `end_line`, `code_snippet`,
///   `change_description`, `commit_hash`, `branch` are honoured too)
///
/// Anything else is rejected instead of being silently dropped.
pub fn validate_code_reference(
    value: Option<&serde_json::Value>,
    parameter_path: &str,
) -> Result<Option<CodeReference>, CoercionError> {
    use serde_json::Value;

    let invalid = |why: &str, received: &Value| {
        CoercionError::new(
            &format!("Invalid code_reference: {why}"),
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "invalid code_reference"),
            Some(received.clone()),
        )
        .with_parameter_path(parameter_path.to_string())
        .with_expected_type(
            "\"path[:line[-end]]\" string or {\"file\": ..., \"line\": ..., \"function\": ...} object",
        )
        .with_hint("Examples: \"src/main.rs:42\" or {\"file\": \"src/main.rs\", \"line\": 42}")
    };

    let reference = |file_path: String, start: u32, end: u32| CodeReference {
        file_path,
        start_line: start,
        end_line: end.max(start),
        code_snippet: String::new(),
        commit_hash: None,
        branch: None,
        change_description: String::new(),
    };

    match value {
        None | Some(Value::Null) => Ok(None),
        Some(v @ Value::String(s)) => {
            let s = s.trim();
            // Only treat a trailing `:N` / `:N-M` as a line spec, so paths
            // containing ':' (e.g. Windows drives) still parse.
            let (path, lines) = match s.rsplit_once(':') {
                Some((p, spec))
                    if !p.is_empty()
                        && !spec.is_empty()
                        && spec.chars().all(|c| c.is_ascii_digit() || c == '-') =>
                {
                    (p, Some(spec))
                }
                _ => (s, None),
            };
            if path.is_empty() {
                return Err(invalid("empty file path", v));
            }
            let (start, end) = match lines {
                None => (0, 0),
                Some(spec) => {
                    let mut parts = spec.splitn(2, '-');
                    let start = parts.next().and_then(|p| p.parse::<u32>().ok());
                    let end = match parts.next() {
                        Some(p) => p.parse::<u32>().ok(),
                        None => start,
                    };
                    match (start, end) {
                        (Some(s), Some(e)) => (s, e),
                        _ => return Err(invalid("malformed line range", v)),
                    }
                }
            };
            Ok(Some(reference(path.to_string(), start, end)))
        }
        Some(v @ Value::Object(map)) => {
            let str_field = |k: &str| map.get(k).and_then(Value::as_str).map(str::to_string);
            let line_field = |k: &str| -> Result<Option<u32>, CoercionError> {
                match map.get(k) {
                    None | Some(Value::Null) => Ok(None),
                    Some(n) => n
                        .as_u64()
                        .and_then(|n| u32::try_from(n).ok())
                        .map(Some)
                        .ok_or_else(|| {
                            invalid(&format!("'{k}' must be a non-negative integer"), v)
                        }),
                }
            };

            let file_path = str_field("file_path")
                .or_else(|| str_field("file"))
                .filter(|p| !p.trim().is_empty())
                .ok_or_else(|| invalid("missing 'file' (or 'file_path')", v))?;
            let start = line_field("start_line")?
                .or(line_field("line")?)
                .unwrap_or(0);
            let end = line_field("end_line")?.unwrap_or(start);

            let mut code_ref = reference(file_path, start, end);
            code_ref.code_snippet = str_field("code_snippet").unwrap_or_default();
            code_ref.commit_hash = str_field("commit_hash");
            code_ref.branch = str_field("branch");
            code_ref.change_description = str_field("change_description")
                .or_else(|| str_field("description"))
                .or_else(|| str_field("function").map(|f| format!("in {f}")))
                .unwrap_or_default();
            Ok(Some(code_ref))
        }
        Some(v) => Err(invalid("expected a string or an object", v)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_code_reference_string_forms() {
        use serde_json::json;
        let r = validate_code_reference(Some(&json!("src/main.rs:42")), "code_reference")
            .unwrap()
            .unwrap();
        assert_eq!(
            (r.file_path.as_str(), r.start_line, r.end_line),
            ("src/main.rs", 42, 42)
        );

        let r = validate_code_reference(Some(&json!("src/lib.rs:10-20")), "code_reference")
            .unwrap()
            .unwrap();
        assert_eq!((r.start_line, r.end_line), (10, 20));

        let r = validate_code_reference(Some(&json!("C:\\x\\y.rs")), "code_reference")
            .unwrap()
            .unwrap();
        assert_eq!((r.file_path.as_str(), r.start_line), ("C:\\x\\y.rs", 0));

        assert!(
            validate_code_reference(None, "code_reference")
                .unwrap()
                .is_none()
        );
        assert!(
            validate_code_reference(Some(&json!(null)), "code_reference")
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn test_code_reference_object_forms() {
        use serde_json::json;
        let r = validate_code_reference(
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
        let r = validate_code_reference(Some(&canonical), "code_reference")
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
            let err = validate_code_reference(Some(&bad), "updates[0].code_reference")
                .expect_err(&format!("{bad} should be rejected"));
            assert_eq!(
                err.parameter_path.as_deref(),
                Some("updates[0].code_reference")
            );
        }
    }

    #[test]
    fn test_validate_session_id_valid() {
        assert!(validate_session_id("60c598e2-d602-4e07-a328-c458006d48c7").is_ok());
    }

    #[test]
    fn test_validate_session_id_invalid() {
        let result = validate_session_id("invalid");
        assert!(result.is_err());
        let error = result.unwrap_err();
        assert_eq!(error.parameter_path, Some("session_id".to_string()));
        assert!(error.hint.is_some());
    }

    #[test]
    fn test_validate_workspace_id_valid() {
        assert!(validate_workspace_id("60c598e2-d602-4e07-a328-c458006d48c7").is_ok());
    }

    #[test]
    fn test_validate_workspace_id_invalid() {
        let result = validate_workspace_id("not-a-uuid");
        assert!(result.is_err());
        let error = result.unwrap_err();
        assert_eq!(error.parameter_path, Some("workspace_id".to_string()));
    }

    #[test]
    fn test_validate_interaction_type_valid() {
        assert!(validate_interaction_type("qa").is_ok());
        assert!(validate_interaction_type("decision_made").is_ok());
        assert!(validate_interaction_type("problem_solved").is_ok());
        assert!(validate_interaction_type("code_change").is_ok());
        assert!(validate_interaction_type("requirement_added").is_ok());
        assert!(validate_interaction_type("concept_defined").is_ok());
    }

    #[test]
    fn test_validate_interaction_type_invalid() {
        let result = validate_interaction_type("made_decision");
        assert!(result.is_err());
        let error = result.unwrap_err();
        assert_eq!(error.parameter_path, Some("interaction_type".to_string()));
        assert!(error.hint.unwrap().contains("decision_made"));
    }

    #[test]
    fn test_validate_scope_valid() {
        assert!(validate_scope("session").is_ok());
        assert!(validate_scope("workspace").is_ok());
        assert!(validate_scope("global").is_ok());
    }

    #[test]
    fn test_validate_scope_invalid() {
        let result = validate_scope("invalid_scope");
        assert!(result.is_err());
        let error = result.unwrap_err();
        assert_eq!(error.parameter_path, Some("scope".to_string()));
    }

    #[test]
    fn test_validate_session_action_valid() {
        assert!(validate_session_action("create").is_ok());
        assert!(validate_session_action("list").is_ok());
    }

    #[test]
    fn test_validate_session_action_invalid() {
        // "delete" was added to VALID_ACTIONS in a later refactor; the
        // test previously asserted that "delete" was rejected, which
        // became the long-standing baseline failure noted in TODO.md.
        // Use a genuinely invalid action here.
        let result = validate_session_action("nuke_everything");
        assert!(result.is_err());
        let error = result.unwrap_err();
        let hint = error.hint.as_ref().unwrap();
        assert!(hint.contains("create") && hint.contains("list"));
    }

    #[test]
    fn test_validate_workspace_action_valid() {
        assert!(validate_workspace_action("create").is_ok());
        assert!(validate_workspace_action("list").is_ok());
        assert!(validate_workspace_action("get").is_ok());
        assert!(validate_workspace_action("delete").is_ok());
        assert!(validate_workspace_action("add_session").is_ok());
        assert!(validate_workspace_action("remove_session").is_ok());
    }

    #[test]
    fn test_validate_workspace_action_invalid() {
        let result = validate_workspace_action("invalid_action");
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_session_role_valid() {
        assert!(validate_session_role("primary").is_ok());
        assert!(validate_session_role("related").is_ok());
        assert!(validate_session_role("dependency").is_ok());
        assert!(validate_session_role("shared").is_ok());
    }

    #[test]
    fn test_validate_session_role_invalid() {
        let result = validate_session_role("admin");
        assert!(result.is_err());
        let error = result.unwrap_err();
        assert_eq!(error.parameter_path, Some("role".to_string()));
        assert!(error.hint.unwrap().contains("primary"));
    }

    #[test]
    fn test_validate_limits_within_bounds() {
        assert_eq!(validate_limits(Some(5), 10, 100).unwrap(), 5);
        assert_eq!(validate_limits(Some(10), 10, 100).unwrap(), 10);
        assert_eq!(validate_limits(Some(100), 10, 100).unwrap(), 100);
    }

    #[test]
    fn test_validate_limits_use_default() {
        assert_eq!(validate_limits(None, 10, 100).unwrap(), 10);
        assert_eq!(validate_limits(None, 50, 100).unwrap(), 50);
    }

    #[test]
    fn test_validate_limits_exceeds_maximum() {
        let result = validate_limits(Some(200), 10, 100);
        assert!(result.is_err());
        let error = result.unwrap_err();
        assert_eq!(error.parameter_path, Some("limit".to_string()));
        assert!(error.message.contains("exceeds maximum"));
    }

    #[test]
    fn test_validate_limits_zero() {
        let result = validate_limits(Some(0), 10, 100);
        assert!(result.is_err());
        let error = result.unwrap_err();
        assert!(error.message.contains("at least 1"));
    }

    #[test]
    fn test_validate_recency_bias_valid() {
        // Valid values
        assert_eq!(validate_recency_bias(Some(0.0)).unwrap(), Some(0.0));
        assert_eq!(validate_recency_bias(Some(0.5)).unwrap(), Some(0.5));
        assert_eq!(validate_recency_bias(Some(1.0)).unwrap(), Some(1.0));
        assert_eq!(validate_recency_bias(Some(10.0)).unwrap(), Some(10.0));
    }

    #[test]
    fn test_validate_recency_bias_none() {
        assert_eq!(validate_recency_bias(None).unwrap(), None);
    }

    #[test]
    fn test_validate_recency_bias_negative() {
        let result = validate_recency_bias(Some(-1.0));
        assert!(result.is_err());
        let error = result.unwrap_err();
        assert_eq!(error.parameter_path, Some("recency_bias".to_string()));
        assert!(error.message.contains("out of range"));
    }

    #[test]
    fn test_validate_recency_bias_nan() {
        let result = validate_recency_bias(Some(f32::NAN));
        assert!(result.is_err());
        let error = result.unwrap_err();
        assert_eq!(error.parameter_path, Some("recency_bias".to_string()));
        assert!(error.message.contains("Invalid recency_bias value"));
    }

    #[test]
    fn test_validate_recency_bias_infinity() {
        let result = validate_recency_bias(Some(f32::INFINITY));
        assert!(result.is_err());
        let error = result.unwrap_err();
        assert!(error.message.contains("NaN or Infinity not allowed"));
    }

    #[test]
    fn test_validate_recency_bias_exceeds_maximum() {
        let result = validate_recency_bias(Some(100.0));
        assert!(result.is_err());
        let error = result.unwrap_err();
        assert!(error.message.contains("out of range"));
        assert_eq!(error.parameter_path, Some("recency_bias".to_string()));
    }
}
