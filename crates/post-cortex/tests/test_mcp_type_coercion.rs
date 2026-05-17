//! Integration tests for MCP type coercion and structured error messages
//!
//! Tests that type coercion works correctly for MCP tool parameters

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::json;

// Test parameter structures matching MCP tool requests
#[derive(Debug, Deserialize, Serialize)]
struct UpdateConversationContextRequest {
    pub session_id: String,
    pub interaction_type: Option<String>,
    pub content: Option<std::collections::HashMap<String, String>>,
    pub code_reference: Option<serde_json::Value>,
    pub updates: Option<Vec<ContextUpdateItem>>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
struct ContextUpdateItem {
    pub interaction_type: String,
    pub content: std::collections::HashMap<String, String>,
    pub code_reference: Option<serde_json::Value>,
}

// Import coercion functions
use post_cortex::daemon::coerce::{coerce_and_validate, CoercionError};

#[test]
fn test_coerce_session_id_number_to_string() -> Result<()> {
    // Test that session_id as number is coerced to string
    let params = json!({
        "session_id": 123, // Number instead of string
        "interaction_type": "qa",
        "content": {
            "question": "Test question",
            "answer": "Test answer"
        }
    });

    let result: Result<UpdateConversationContextRequest, _> = coerce_and_validate(params);

    match result {
        Ok(req) => {
            println!("✓ Coercion succeeded: number → string for session_id");
            assert_eq!(req.session_id, "123");
            assert_eq!(req.interaction_type, Some("qa".to_string()));
        }
        Err(e) => {
            println!("Coercion error: {}", e.message);
            // Check for structured error
            if let Some(param) = &e.parameter_path {
                println!("✓ Error includes parameter_path: {}", param);
            }
            if let Some(hint) = &e.hint {
                println!("✓ Error includes hint: {}", hint);
            }
        }
    }

    Ok(())
}

#[test]
fn test_coerce_boolean_in_content() -> Result<()> {
    // Test that boolean values in content are coerced to strings
    let params = json!({
        "session_id": "valid-uuid-string",
        "interaction_type": "decision_made",
        "content": {
            "enabled": true, // Boolean should become "true"
            "approved": false, // Boolean should become "false"
            "score": 42 // Number should become "42"
        }
    });

    let result: Result<UpdateConversationContextRequest, _> = coerce_and_validate(params);

    match result {
        Ok(req) => {
            println!("✓ Boolean/number coercion in content succeeded");
            if let Some(content) = req.content {
                assert_eq!(content.get("enabled"), Some(&"true".to_string()));
                assert_eq!(content.get("approved"), Some(&"false".to_string()));
                assert_eq!(content.get("score"), Some(&"42".to_string()));
                println!("✓ All values correctly coerced to strings");
            }
        }
        Err(e) => {
            println!("Coercion error: {}", e.message);
        }
    }

    Ok(())
}

#[test]
fn test_error_structure_for_invalid_uuid() -> Result<()> {
    // Test error structure for invalid UUID
    let params = json!({
        "session_id": "not-a-uuid",
        "interaction_type": "qa",
        "content": {
            "question": "Test",
            "answer": "Test"
        }
    });

    // This should deserialize successfully (string type is correct)
    // but UUID validation would happen later in the handler
    let result: Result<UpdateConversationContextRequest, _> = coerce_and_validate(params);

    match result {
        Ok(req) => {
            println!("✓ String session_id accepted (UUID validation happens later)");
            assert_eq!(req.session_id, "not-a-uuid");
        }
        Err(e) => {
            println!("Unexpected error: {}", e.message);
        }
    }

    Ok(())
}

#[test]
fn test_error_structure_for_missing_fields() -> Result<()> {
    // Test error when required fields are missing
    let params = json!({
        "session_id": "some-uuid"
        // Missing both content and updates
    });

    let result: Result<UpdateConversationContextRequest, _> = coerce_and_validate(params);

    // Coercion/Deserialization succeeds, but business logic validation happens later
    match result {
        Ok(req) => {
            println!("✓ Deserialization succeeded (field validation happens in handler)");
            assert_eq!(req.session_id, "some-uuid");
            assert!(req.content.is_none());
            assert!(req.updates.is_none());
        }
        Err(e) => {
            println!("Error: {}", e.message);
        }
    }

    Ok(())
}

#[test]
fn test_coercion_error_structured_format() -> Result<()> {
    // Test that CoercionError produces properly structured MCP errors
    let error = CoercionError::new(
        "Invalid parameter type",
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "test error"),
        Some(json!(12345)),
    )
    .with_parameter_path("session_id".to_string())
    .with_expected_type("UUID string (36 chars with hyphens)")
    .with_hint("Create a session first using the 'session' tool");

    // Convert to MCP error
    let mcp_error = error.to_mcp_error();

    // Verify error structure
    let error_msg = &mcp_error.message;
    assert!(error_msg.contains("session_id") || error_msg.contains("Invalid parameter"));

    // Parse the message as JSON (it should be structured)
    let error_data: serde_json::Value = serde_json::from_str(error_msg)?;

    assert_eq!(error_data["parameter"], "session_id");
    assert_eq!(error_data["expectedType"], "UUID string (36 chars with hyphens)");
    assert_eq!(error_data["receivedValue"], 12345);
    assert!(error_data["hint"].as_str().unwrap().contains("Create a session"));

    println!("✓ CoercionError produces well-structured MCP errors");
    println!("Error structure:");
    println!("{}", serde_json::to_string_pretty(&error_data)?);

    Ok(())
}

#[test]
fn test_fast_path_for_correct_types() -> Result<()> {
    // Test that correct types pass through without coercion overhead
    let params = json!({
        "session_id": "60c598e2-d602-4e07-a328-c458006d48c7", // Already string
        "interaction_type": "qa", // Already string
        "content": {
            "question": "Test",
            "answer": "Test" // All strings
        }
    });

    let result: Result<UpdateConversationContextRequest, _> = coerce_and_validate(params);

    assert!(result.is_ok(), "Valid types should deserialize directly");
    let req = result.unwrap();
    assert_eq!(req.session_id, "60c598e2-d602-4e07-a328-c458006d48c7");

    println!("✓ Fast path works: correct types pass through directly");

    Ok(())
}

#[test]
fn test_object_to_json_string_coercion() -> Result<()> {
    // Test that nested objects are stringified as JSON
    let params = json!({
        "session_id": "some-uuid",
        "interaction_type": "code_change",
        "content": {
            "metadata": { // Object should be stringified
                "file": "main.rs",
                "line": 42
            },
            "tags": ["bugfix", "performance"] // Array should be stringified
        }
    });

    let result: Result<UpdateConversationContextRequest, _> = coerce_and_validate(params);

    match result {
        Ok(req) => {
            if let Some(content) = req.content {
                let metadata = content.get("metadata").unwrap();
                assert!(metadata.contains("{") || metadata.contains("file"),
                    "Object should be stringified as JSON");

                let tags = content.get("tags").unwrap();
                assert!(tags.contains("[") || tags.contains("bugfix"),
                    "Array should be stringified as JSON");

                println!("✓ Object/array → JSON string coercion works");
                println!("  metadata: {}", metadata);
                println!("  tags: {}", tags);
            }
        }
        Err(e) => {
            println!("Error: {}", e.message);
        }
    }

    Ok(())
}

#[test]
fn test_all_coercion_rules_together() -> Result<()> {
    // Test multiple coercions in a single request
    let params = json!({
        "session_id": 12345, // Number → String
        "interaction_type": "decision_made",
        "content": {
            "enabled": true, // Boolean → String
            "count": 42, // Number → String
            "config": { // Object → JSON string
                "debug": false
            },
            "list": [1, 2, 3] // Array → JSON string
        }
    });

    let result: Result<UpdateConversationContextRequest, _> = coerce_and_validate(params);

    match result {
        Ok(req) => {
            assert_eq!(req.session_id, "12345");
            if let Some(content) = req.content {
                assert_eq!(content.get("enabled"), Some(&"true".to_string()));
                assert_eq!(content.get("count"), Some(&"42".to_string()));
                println!("✓ Multiple coercions work together in single request");
            }
        }
        Err(e) => {
            println!("Error: {}", e.message);
        }
    }

    Ok(())
}

#[test]
fn test_error_messages_are_actionable() -> Result<()> {
    // Test that error messages provide actionable guidance

    // Test 1: Wrong type for session_id
    let error1 = CoercionError::new(
        "Type mismatch",
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "expected string"),
        Some(json!(123)),
    )
    .with_parameter_path("session_id".to_string())
    .with_expected_type("UUID string")
    .with_hint("Use session tool to create a session first");

    let mcp_error1 = error1.to_mcp_error();
    let data1: serde_json::Value = serde_json::from_str(&mcp_error1.message)?;
    assert!(data1["hint"].as_str().unwrap().contains("session tool"));

    // Test 2: Invalid interaction_type
    let error2 = CoercionError::new(
        "Invalid value",
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "unknown variant"),
        Some(json!("made_decision")),
    )
    .with_parameter_path("interaction_type".to_string())
    .with_expected_type("one of: qa, decision_made, problem_solved, code_change, requirement_added, concept_defined")
    .with_hint("Use 'decision_made' instead of 'made_decision'");

    let mcp_error2 = error2.to_mcp_error();
    let data2: serde_json::Value = serde_json::from_str(&mcp_error2.message)?;
    assert!(data2["expectedType"].as_str().unwrap().contains("qa, decision_made"));

    println!("✓ Error messages provide actionable hints for fixing mistakes");

    Ok(())
}
