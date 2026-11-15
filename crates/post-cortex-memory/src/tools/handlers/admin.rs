// Copyright (c) 2025, 2026 Julius ML
// MIT License

//! Tool `admin`: health, vectorize_session, vectorize_stats,
//! create_checkpoint.

use crate::Memory;
use serde_json::Value;

use crate::tools::Failure;
use crate::tools::args::AdminRequest;
use crate::tools::check;
use crate::tools::handlers::Outcome;
use crate::tools::parse::{FieldHint, arguments};
use crate::tools::reply;
use crate::tools::responses::Vectorized;
use crate::tools::responses::{Checkpoint, HealthPayload};

const TOOL: &str = "admin";

const HINTS: &[FieldHint] = &[FieldHint {
    field: "action",
    expected: "one of: health, vectorize_session, vectorize_stats, create_checkpoint",
    hint: "",
}];

pub async fn handle(memory: &Memory, params: Value) -> Outcome {
    let req: AdminRequest = arguments(params, HINTS)?;
    let action = req.action.to_lowercase();
    match action.as_str() {
        "health" => {
            let health = memory.admin.health();
            let message = format!(
                "PCX healthy={} version={} active_sessions={}",
                health.healthy, health.version, health.active_sessions
            );
            reply::with(message, &HealthPayload::from(health))
        }
        "vectorize_session" => {
            let session_id = check::session_id(required_session(&req, &action)?)?;
            let vectors_created = memory
                .admin
                .vectorize_session(session_id)
                .await
                .map_err(Failure::from)?;
            reply::with(
                format!("Vectorized session {session_id} ({vectors_created} vectors created)"),
                &Vectorized {
                    session_id: session_id.to_string(),
                    vectors_created,
                },
            )
        }
        "vectorize_stats" => {
            let stats = memory
                .admin
                .vectorization_stats()
                .await
                .map_err(Failure::from)?;
            reply::with("Vectorization stats", &stats)
        }
        "create_checkpoint" => {
            let session_id = check::session_id(required_session(&req, &action)?)?;
            let checkpoint_id = memory.sessions.checkpoint(session_id).await.map_err(|e| {
                let message = format!("Failed to load session: {e}");
                Failure::from(e).with_message(message)
            })?;
            reply::with(
                "Checkpoint created successfully",
                &Checkpoint {
                    checkpoint_id: checkpoint_id.to_string(),
                },
            )
        }
        other => Err(crate::tools::handlers::unhandled(TOOL, other)),
    }
}

fn required_session<'a>(req: &'a AdminRequest, action: &str) -> Result<&'a str, Failure> {
    req.session_id.as_deref().ok_or_else(|| {
        Failure::invalid(format!("{action} requires session_id")).parameter("session_id")
    })
}
