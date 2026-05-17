//! Integration tests for MCP validation layer
//!
//! Tests that the custom validation functions work correctly for MCP tool parameters

use anyhow::Result;
use post_cortex::daemon::validate::*;

#[test]
fn test_validate_session_id_with_valid_uuid() -> Result<()> {
    let valid_uuid = "60c598e2-d602-4e07-a328-c458006d48c7";
    assert!(validate_session_id(valid_uuid).is_ok());
    println!("✓ Valid UUID accepted: {}", valid_uuid);
    Ok(())
}

#[test]
fn test_validate_session_id_with_invalid_uuid() -> Result<()> {
    let invalid_uuid = "not-a-uuid";
    let result = validate_session_id(invalid_uuid);

    assert!(result.is_err());
    let error = result.unwrap_err();
    assert_eq!(error.parameter_path, Some("session_id".to_string()));
    assert!(error.expected_type.as_ref().unwrap().contains("UUID"));
    assert!(
        error.hint.as_ref().unwrap().contains("create")
            || error.hint.as_ref().unwrap().contains("semantic")
    );

    println!("✓ Invalid UUID rejected with helpful error");
    println!("  Error: {}", error.message);
    println!("  Hint: {}", error.hint.unwrap());

    Ok(())
}

#[test]
fn test_validate_workspace_id_with_valid_uuid() -> Result<()> {
    let valid_uuid = "f1d2e3a4-b5c6-7d8e-9f0a-1b2c3d4e5f6a"; // Fixed: valid UUID
    assert!(validate_workspace_id(valid_uuid).is_ok());
    println!("✓ Valid workspace UUID accepted");
    Ok(())
}

#[test]
fn test_validate_workspace_id_with_invalid_uuid() -> Result<()> {
    let invalid_uuid = "12345"; // Common mistake: number as string
    let result = validate_workspace_id(invalid_uuid);

    assert!(result.is_err());
    let error = result.unwrap_err();
    assert_eq!(error.parameter_path, Some("workspace_id".to_string()));
    assert!(error.hint.as_ref().unwrap().contains("list"));

    println!("✓ Invalid workspace UUID rejected");
    println!("  Hint guides user to use 'list' action");

    Ok(())
}

#[test]
fn test_validate_interaction_type_with_valid_types() -> Result<()> {
    let valid_types = vec![
        "qa",
        "decision_made",
        "problem_solved",
        "code_change",
        "requirement_added",
        "concept_defined",
    ];

    for interaction_type in valid_types {
        assert!(validate_interaction_type(interaction_type).is_ok());
    }

    println!("✓ All 6 valid interaction_type values accepted");
    Ok(())
}

#[test]
fn test_validate_interaction_type_with_invalid_type() -> Result<()> {
    let invalid_type = "made_decision"; // Common mistake: wrong word order
    let result = validate_interaction_type(invalid_type);

    assert!(result.is_err());
    let error = result.unwrap_err();
    assert_eq!(error.parameter_path, Some("interaction_type".to_string()));
    assert!(
        error
            .expected_type
            .as_ref()
            .unwrap()
            .contains("qa, decision_made")
    );
    assert!(error.hint.as_ref().unwrap().contains("decision_made"));

    println!("✓ Invalid interaction_type rejected with correction hint");
    println!("  Error suggests correct value in hint");

    Ok(())
}

#[test]
fn test_validate_scope_with_valid_scopes() -> Result<()> {
    let valid_scopes = vec!["session", "workspace", "global"];

    for scope in valid_scopes {
        assert!(validate_scope(scope).is_ok());
    }

    println!("✓ All 3 valid scope values accepted");
    Ok(())
}

#[test]
fn test_validate_scope_with_invalid_scope() -> Result<()> {
    let invalid_scope = "all"; // Common mistake: wrong scope name
    let result = validate_scope(invalid_scope);

    assert!(result.is_err());
    let error = result.unwrap_err();
    assert_eq!(error.parameter_path, Some("scope".to_string()));
    assert!(
        error
            .expected_type
            .as_ref()
            .unwrap()
            .contains("session, workspace, global")
    );
    assert!(error.hint.as_ref().unwrap().contains("global"));

    println!("✓ Invalid scope rejected with valid options");
    Ok(())
}

#[test]
fn test_validate_session_action_with_valid_actions() -> Result<()> {
    let valid_actions = vec!["create", "list"];

    for action in valid_actions {
        assert!(validate_session_action(action).is_ok());
    }

    println!("✓ All valid session actions accepted");
    Ok(())
}

#[test]
fn test_validate_session_action_with_invalid_action() -> Result<()> {
    let invalid_action = "delete"; // Sessions don't have delete action
    let result = validate_session_action(invalid_action);

    assert!(result.is_err());
    let error = result.unwrap_err();
    assert_eq!(error.parameter_path, Some("action".to_string()));
    assert!(
        error.hint.as_ref().unwrap().contains("create")
            && error.hint.as_ref().unwrap().contains("list")
    );

    println!("✓ Invalid session action rejected");
    println!("  Hint lists valid actions: create, list");

    Ok(())
}

#[test]
fn test_validate_workspace_action_with_valid_actions() -> Result<()> {
    let valid_actions = vec![
        "create",
        "list",
        "get",
        "delete",
        "add_session",
        "remove_session",
    ];

    for action in valid_actions {
        assert!(validate_workspace_action(action).is_ok());
    }

    println!("✓ All 6 valid workspace actions accepted");
    Ok(())
}

#[test]
fn test_validate_workspace_action_with_invalid_action() -> Result<()> {
    let invalid_action = "update"; // No update action
    let result = validate_workspace_action(invalid_action);

    assert!(result.is_err());
    let error = result.unwrap_err();
    assert!(
        error.hint.as_ref().unwrap().contains("create")
            && error.hint.as_ref().unwrap().contains("add_session")
    );

    println!("✓ Invalid workspace action rejected");
    println!("  Hint lists all 6 valid actions");

    Ok(())
}

#[test]
fn test_validate_session_role_with_valid_roles() -> Result<()> {
    let valid_roles = vec!["primary", "related", "dependency", "shared"];

    for role in valid_roles {
        assert!(validate_session_role(role).is_ok());
    }

    println!("✓ All 4 valid session roles accepted");
    Ok(())
}

#[test]
fn test_validate_session_role_with_invalid_role() -> Result<()> {
    let invalid_role = "admin"; // Common mistake: non-existent role
    let result = validate_session_role(invalid_role);

    assert!(result.is_err());
    let error = result.unwrap_err();
    assert_eq!(error.parameter_path, Some("role".to_string()));
    assert!(error.hint.as_ref().unwrap().contains("primary"));

    println!("✓ Invalid session role rejected");
    println!("  Hint shows all 4 valid roles");

    Ok(())
}

#[test]
fn test_validate_limits_with_valid_values() -> Result<()> {
    // Test default value
    assert_eq!(validate_limits(None, 10, 100).unwrap(), 10);

    // Test within bounds
    assert_eq!(validate_limits(Some(5), 10, 100).unwrap(), 5);
    assert_eq!(validate_limits(Some(50), 10, 100).unwrap(), 50);
    assert_eq!(validate_limits(Some(100), 10, 100).unwrap(), 100);

    println!("✓ Limits validated correctly (default, min, max)");
    Ok(())
}

#[test]
fn test_validate_limits_exceeds_maximum() -> Result<()> {
    let result = validate_limits(Some(200), 10, 100);

    assert!(result.is_err());
    let error = result.unwrap_err();
    assert_eq!(error.parameter_path, Some("limit".to_string()));
    assert!(error.message.contains("exceeds maximum"));
    assert!(error.hint.as_ref().unwrap().contains("1 and 100"));

    println!("✓ Limit exceeding maximum rejected");
    println!("  Hint shows valid range: 1 to 100");

    Ok(())
}

#[test]
fn test_validate_limits_zero_value() -> Result<()> {
    let result = validate_limits(Some(0), 10, 100);

    assert!(result.is_err());
    let error = result.unwrap_err();
    assert!(error.message.contains("at least 1"));

    println!("✓ Limit of 0 rejected (minimum is 1)");

    Ok(())
}

#[test]
fn test_validation_errors_include_all_fields() -> Result<()> {
    let result = validate_session_id("invalid");

    assert!(result.is_err());
    let error = result.unwrap_err();

    // Check all error fields are present
    assert!(error.parameter_path.is_some());
    assert!(error.expected_type.is_some());
    assert!(error.received_value.is_some());
    assert!(error.hint.is_some());

    // Check fields are not empty
    assert!(!error.message.is_empty());
    assert!(!error.parameter_path.unwrap().is_empty());
    assert!(!error.expected_type.unwrap().is_empty());
    assert!(!error.hint.unwrap().is_empty());

    println!("✓ Validation errors include all required fields:");
    println!("  - message");
    println!("  - parameter_path");
    println!("  - expected_type");
    println!("  - received_value");
    println!("  - hint");

    Ok(())
}

#[test]
fn test_common_mistakes_have_helpful_hints() -> Result<()> {
    // Mistake 1: Wrong interaction_type word order
    let result1 = validate_interaction_type("made_decision");
    assert!(result1.is_err());
    let error1 = result1.unwrap_err();
    assert!(error1.hint.as_ref().unwrap().contains("decision_made"));

    // Mistake 2: Number as UUID
    let result2 = validate_session_id("12345");
    assert!(result2.is_err());
    let error2 = result2.unwrap_err();
    assert!(error2.hint.as_ref().unwrap().contains("create"));

    // Mistake 3: Invalid action
    let result3 = validate_session_action("delete");
    assert!(result3.is_err());
    let error3 = result3.unwrap_err();
    assert!(
        error3.hint.as_ref().unwrap().contains("create")
            && error3.hint.as_ref().unwrap().contains("list")
    );

    println!("✓ Common mistakes have helpful, corrective hints");

    Ok(())
}

#[test]
fn test_validation_integration_scenario() -> Result<()> {
    // Simulate a common scenario: agent makes multiple validation mistakes

    // Mistake 1: Invalid UUID format
    let uuid_result = validate_session_id("abc");
    assert!(uuid_result.is_err());
    let uuid_error = uuid_result.unwrap_err();
    println!("Step 1: Agent learns UUID format from error");
    println!("  Error: {}", uuid_error.message);

    // Mistake 2: Wrong interaction_type
    let type_result = validate_interaction_type("DecisionMade");
    assert!(type_result.is_err());
    let type_error = type_result.unwrap_err();
    println!("Step 2: Agent learns correct case for interaction_type");
    println!("  Error: {}", type_error.message);

    // Mistake 3: Invalid scope
    let scope_result = validate_scope("all");
    assert!(scope_result.is_err());
    let scope_error = scope_result.unwrap_err();
    println!("Step 3: Agent learns valid scope values");
    println!("  Error: {}", scope_error.message);

    // Now agent gets it right
    assert!(validate_session_id("60c598e2-d602-4e07-a328-c458006d48c7").is_ok());
    assert!(validate_interaction_type("decision_made").is_ok());
    assert!(validate_scope("global").is_ok());

    println!("✓ Agent self-corrects after validation errors");

    Ok(())
}
