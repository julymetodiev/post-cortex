// Copyright (c) 2026 Julius ML
// MIT License

//! The MCP mapping of the host's prompts (ARCHITECTURE section 7,
//! prompts): a module's prompt as an MCP prompt, `prompts/get` arguments
//! as the module's, its filled prompt as the MCP result, and a failure as
//! the JSON-RPC error MCP gives `prompts/get` (it has no `isError`).

use std::collections::HashMap;

use pcx_module_api::{ModuleError, ModuleErrorKind, Prompt, PromptContent, PromptRole};
use rmcp::model::{
    ErrorData as McpError, GetPromptResult, JsonObject, Prompt as McpPrompt,
    PromptArgument as McpArgument, PromptMessage, Role,
};
use serde_json::Value;

/// The MCP prompt of a module's prompt.
pub fn prompt(prompt: Prompt) -> McpPrompt {
    let arguments = prompt
        .arguments
        .into_iter()
        .map(|a| {
            let argument = McpArgument::new(a.name).with_required(a.required);
            if a.description.is_empty() {
                argument
            } else {
                argument.with_description(a.description)
            }
        })
        .collect::<Vec<_>>();
    McpPrompt::new(
        prompt.name,
        (!prompt.description.is_empty()).then_some(prompt.description),
        (!arguments.is_empty()).then_some(arguments),
    )
}

/// The arguments of `prompts/get`, each a string as MCP defines them.
///
/// # Errors
/// `invalid_params` for an argument that is not a string.
pub fn arguments(arguments: Option<JsonObject>) -> Result<HashMap<String, String>, McpError> {
    arguments
        .unwrap_or_default()
        .into_iter()
        .map(|(name, value)| match value {
            Value::String(text) => Ok((name, text)),
            _ => Err(McpError::invalid_params(
                format!("prompt argument {name} is not a string"),
                None,
            )),
        })
        .collect()
}

/// The MCP result of a filled prompt.
///
/// # Errors
/// `internal_error` for a message whose role the module left unset.
pub fn content(content: PromptContent) -> Result<GetPromptResult, McpError> {
    let messages = content
        .messages
        .into_iter()
        .map(|m| {
            let role = match PromptRole::try_from(m.role) {
                Ok(PromptRole::User) => Role::User,
                Ok(PromptRole::Assistant) => Role::Assistant,
                Ok(PromptRole::Unspecified) | Err(_) => {
                    return Err(McpError::internal_error(
                        "the module answered a prompt message without a role",
                        None,
                    ));
                }
            };
            Ok(PromptMessage::new_text(role, m.text))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let result = GetPromptResult::new(messages);
    Ok(if content.description.is_empty() {
        result
    } else {
        result.with_description(content.description)
    })
}

/// The JSON-RPC error of a failed `prompts/get`: the caller's mistake as
/// `invalid_params`, any other failure as `internal_error`, with its
/// message and details.
pub fn error(error: ModuleError) -> McpError {
    match error.kind {
        ModuleErrorKind::NotFound | ModuleErrorKind::InvalidArgument => {
            McpError::invalid_params(error.message, error.details)
        }
        _ => McpError::internal_error(error.message, error.details),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use pcx_module_api::{PromptArgument, PromptMessage as ModuleMessage};
    use serde_json::json;

    #[test]
    fn prompts_and_their_results_map_onto_mcp() {
        let mapped = prompt(Prompt {
            name: "review".to_string(),
            description: String::new(),
            arguments: vec![PromptArgument {
                name: "file".to_string(),
                description: "Which".to_string(),
                required: true,
            }],
        });
        assert_eq!(
            serde_json::to_value(&mapped).unwrap(),
            json!({ "name": "review", "arguments": [
                { "name": "file", "description": "Which", "required": true }
            ] })
        );

        let filled = content(PromptContent {
            description: "Review".to_string(),
            messages: vec![ModuleMessage {
                role: PromptRole::User.into(),
                text: "Read it".to_string(),
            }],
        })
        .unwrap();
        assert_eq!(filled.description.as_deref(), Some("Review"));
        assert_eq!(filled.messages.len(), 1);
        let unset = content(PromptContent {
            description: String::new(),
            messages: vec![ModuleMessage {
                role: PromptRole::Unspecified.into(),
                text: String::new(),
            }],
        });
        assert!(unset.is_err());
    }

    #[test]
    fn arguments_are_strings() {
        let given = json!({ "file": "a.rs" }).as_object().cloned();
        assert_eq!(arguments(given).unwrap()["file"], "a.rs");
        assert!(arguments(json!({ "n": 1 }).as_object().cloned()).is_err());
        assert!(arguments(None).unwrap().is_empty());
    }
}
