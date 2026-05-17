// Copyright (c) 2025, 2026 Julius ML
// MIT License

//! Tool 4: get_structured_summary — full or sectioned summary.

use crate::ConversationMemorySystem;
use crate::daemon::coerce::coerce_and_validate;
use crate::daemon::validate::{validate_limits, validate_session_id};
use rmcp::{
    handler::server::wrapper::Parameters,
    model::{CallToolResult, Content, ErrorData as McpError},
};
use std::sync::Arc;

use super::GetStructuredSummaryRequest;

pub(super) async fn handle(
    _memory_system: &Arc<ConversationMemorySystem>,
    params: Parameters<serde_json::Value>,
) -> Result<CallToolResult, McpError> {
    let req: GetStructuredSummaryRequest = coerce_and_validate(params.0)
        .map_err(|e| {
            if e.message.contains("session_id") {
                e.clone()
                    .with_parameter_path("session_id".to_string())
                    .with_expected_type("UUID string (36 chars with hyphens)")
                    .with_hint("Provide a valid session UUID to get its summary")
                    .to_mcp_error()
            } else if e.message.contains("include") {
                e.clone()
                    .with_parameter_path("include".to_string())
                    .with_expected_type("array of: decisions, insights, entities, questions, all")
                    .with_hint("Specify which sections to include, or omit for all. Valid values: decisions, insights, entities, questions, all")
                    .to_mcp_error()
            } else {
                e.to_mcp_error()
            }
        })?;

    validate_session_id(&req.session_id).map_err(|e| e.to_mcp_error())?;

    let validated_decisions_limit =
        validate_limits(req.decisions_limit, 10, 100).map_err(|e| {
            e.with_parameter_path("decisions_limit".to_string())
                .to_mcp_error()
        })?;
    let validated_entities_limit = validate_limits(req.entities_limit, 20, 200).map_err(|e| {
        e.with_parameter_path("entities_limit".to_string())
            .to_mcp_error()
    })?;
    let validated_questions_limit =
        validate_limits(req.questions_limit, 5, 50).map_err(|e| {
            e.with_parameter_path("questions_limit".to_string())
                .to_mcp_error()
        })?;
    let validated_concepts_limit = validate_limits(req.concepts_limit, 10, 50).map_err(|e| {
        e.with_parameter_path("concepts_limit".to_string())
            .to_mcp_error()
    })?;

    let include = req
        .include
        .as_ref()
        .map(|v| v.iter().map(|s| s.to_lowercase()).collect::<Vec<_>>());

    let include_all = include.is_none()
        || include
            .as_ref()
            .is_some_and(|v| v.contains(&"all".to_string()));
    let include_decisions = include_all
        || include
            .as_ref()
            .is_some_and(|v| v.contains(&"decisions".to_string()));
    let include_insights = include_all
        || include
            .as_ref()
            .is_some_and(|v| v.contains(&"insights".to_string()));
    let include_entities = include_all
        || include
            .as_ref()
            .is_some_and(|v| v.contains(&"entities".to_string()));

    let mut result_parts = Vec::new();

    if include_all {
        match crate::tools::mcp::get_structured_summary(
            req.session_id.clone(),
            Some(validated_decisions_limit),
            Some(validated_entities_limit),
            Some(validated_questions_limit),
            Some(validated_concepts_limit),
            req.min_confidence,
            req.compact,
        )
        .await
        {
            Ok(result) => {
                result_parts.push(result.message);
                if let Some(data) = result.data {
                    result_parts.push(format!(
                        "\n\nStructured Data:\n{}",
                        serde_json::to_string_pretty(&data).unwrap_or_default()
                    ));
                }
            }
            Err(e) => return Err(McpError::internal_error(e.to_string(), None)),
        }
    } else {
        if include_decisions {
            match crate::tools::mcp::get_key_decisions(req.session_id.clone()).await {
                Ok(result) => result_parts.push(format!("## Decisions\n{}", result.message)),
                Err(e) => result_parts.push(format!("## Decisions\nError: {}", e)),
            }
        }

        if include_insights {
            match crate::tools::mcp::get_key_insights(
                req.session_id.clone(),
                Some(validated_decisions_limit),
            )
            .await
            {
                Ok(result) => result_parts.push(format!("## Insights\n{}", result.message)),
                Err(e) => result_parts.push(format!("## Insights\nError: {}", e)),
            }
        }

        if include_entities {
            match crate::tools::mcp::get_entity_importance_analysis(
                req.session_id.clone(),
                req.entities_limit,
                req.min_confidence,
            )
            .await
            {
                Ok(result) => result_parts.push(format!("## Entities\n{}", result.message)),
                Err(e) => result_parts.push(format!("## Entities\nError: {}", e)),
            }
        }
    }

    Ok(CallToolResult::success(vec![Content::text(
        result_parts.join("\n\n"),
    )]))
}
