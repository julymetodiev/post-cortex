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
/// ```rust
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
/// ```rust
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
        if value < MIN_RECENCY_BIAS || value > MAX_RECENCY_BIAS {
            return Err(CoercionError::new(
                "recency_bias out of range",
                std::io::Error::new(std::io::ErrorKind::InvalidInput, "Value must be between 0.0 and 10.0"),
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
/// ```rust
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

#[cfg(test)]
mod tests {
    use super::*;

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
        let result = validate_session_action("delete");
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
