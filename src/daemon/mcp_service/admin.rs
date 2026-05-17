// Copyright (c) 2025, 2026 Julius ML
// MIT License

//! Tool 9: admin — health / vectorize_session / vectorize_stats / create_checkpoint.

use crate::ConversationMemorySystem;
use crate::daemon::coerce::coerce_and_validate;
use crate::daemon::validate::validate_session_id;
use crate::tools::mcp::MCPToolResult;
use rmcp::{
    handler::server::wrapper::Parameters,
    model::{CallToolResult, ErrorData as McpError},
};
use std::sync::Arc;

use super::AdminRequest;
use super::mcp_result_to_call_result;

pub(super) async fn handle(
    memory_system: &Arc<ConversationMemorySystem>,
    params: Parameters<serde_json::Value>,
) -> Result<CallToolResult, McpError> {
    let req: AdminRequest = coerce_and_validate(params.0).map_err(|e| {
        e.with_parameter_path("action".to_string())
            .with_expected_type("one of: health, vectorize_session, vectorize_stats, create_checkpoint")
            .to_mcp_error()
    })?;

    match req.action.to_lowercase().as_str() {
        "health" => {
            let health = memory_system.get_system_health();
            let payload = serde_json::json!({
                "healthy": !health.circuit_breaker_open,
                "version": env!("CARGO_PKG_VERSION"),
                "active_sessions": health.active_sessions,
                "total_updates": health.total_requests,
                "embeddings_enabled": memory_system.embeddings_enabled(),
                "circuit_breaker_open": health.circuit_breaker_open,
            });
            Ok(mcp_result_to_call_result(MCPToolResult::success(
                format!(
                    "PCX healthy={} version={} active_sessions={}",
                    !health.circuit_breaker_open,
                    env!("CARGO_PKG_VERSION"),
                    health.active_sessions
                ),
                Some(payload),
            )))
        }
        "vectorize_session" => {
            let sid_raw = req.session_id.as_ref().ok_or_else(|| {
                McpError::invalid_params(
                    "vectorize_session requires session_id".to_string(),
                    Some(serde_json::Value::String("session_id".to_string())),
                )
            })?;
            let session_id = validate_session_id(sid_raw).map_err(|e| e.to_mcp_error())?;

            #[cfg(feature = "embeddings")]
            {
                match memory_system.vectorize_session(session_id).await {
                    Ok(vectors_created) => Ok(mcp_result_to_call_result(MCPToolResult::success(
                        format!(
                            "Vectorized session {} ({} vectors created)",
                            session_id, vectors_created
                        ),
                        Some(serde_json::json!({
                            "session_id": session_id.to_string(),
                            "vectors_created": vectors_created,
                        })),
                    ))),
                    Err(e) => Err(McpError::internal_error(e, None)),
                }
            }
            #[cfg(not(feature = "embeddings"))]
            {
                let _ = session_id;
                Err(McpError::internal_error(
                    "vectorize_session requires the 'embeddings' feature".to_string(),
                    None,
                ))
            }
        }
        "vectorize_stats" => {
            #[cfg(feature = "embeddings")]
            {
                match memory_system.get_vectorization_stats() {
                    Ok(stats) => {
                        let value =
                            serde_json::to_value(&stats).unwrap_or(serde_json::json!({}));
                        Ok(mcp_result_to_call_result(MCPToolResult::success(
                            "Vectorization stats".to_string(),
                            Some(value),
                        )))
                    }
                    Err(e) => Err(McpError::internal_error(e, None)),
                }
            }
            #[cfg(not(feature = "embeddings"))]
            {
                Err(McpError::internal_error(
                    "vectorize_stats requires the 'embeddings' feature".to_string(),
                    None,
                ))
            }
        }
        "create_checkpoint" => {
            let sid_raw = req.session_id.as_ref().ok_or_else(|| {
                McpError::invalid_params(
                    "create_checkpoint requires session_id".to_string(),
                    Some(serde_json::Value::String("session_id".to_string())),
                )
            })?;
            let session_id = validate_session_id(sid_raw).map_err(|e| e.to_mcp_error())?;

            match crate::tools::mcp::create_session_checkpoint(session_id).await {
                Ok(result) => Ok(mcp_result_to_call_result(result)),
                Err(e) => Err(McpError::internal_error(e.to_string(), None)),
            }
        }
        _ => Err(McpError::invalid_params(
            format!(
                "Invalid action '{}'. Use: health | vectorize_session | vectorize_stats | create_checkpoint",
                req.action
            ),
            None,
        )),
    }
}
