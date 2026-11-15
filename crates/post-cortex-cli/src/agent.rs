// Copyright (c) 2026 Julius ML
// MIT License

//! `pcx agent`: one turn of the local daemon's agent (`pcx.agent.v1`
//! `RunTurn`), its answer printed as it comes; its tool calls and the way
//! it ended on stderr. Without a session a new one is created and named.

use post_cortex_proto::AUTHOR_HEADER;
use post_cortex_proto::agent::agent_client::AgentClient;
use post_cortex_proto::agent::{RunTurnRequest, turn_reply::Event};
use std::io::Write;

use crate::daemon_client::DaemonClient;
use crate::local::{Local, env_text};

/// Run one turn of `session` (none: a new one) for `prompt` and print it.
///
/// # Errors
/// The daemon is not running or has no agent, or the turn did not end
/// with an answer.
pub async fn run(prompt: String, session: Option<String>, local: &Local) -> Result<(), String> {
    let daemon = DaemonClient::connect(local).await?;
    let session = match session {
        Some(session) => session,
        None => {
            let created = daemon
                .create_session(Some("pcx agent".into()), None)
                .await?;
            eprintln!("session {}", created.id);
            created.id
        }
    };
    let mut client = AgentClient::connect(local.grpc_url()).await.map_err(|e| {
        format!(
            "cannot reach the daemon's gRPC at {}: {e}",
            local.grpc_url()
        )
    })?;
    // The agent's own settings: no instructions, model, modules or tools
    // of a turn's; the daemon gives a client's turn its scope.
    let mut request = tonic::Request::new(RunTurnRequest {
        session_id: session,
        prompt,
        ..RunTurnRequest::default()
    });
    if let Some(token) = local.auth_token.as_deref().filter(|t| !t.trim().is_empty()) {
        let value = format!("Bearer {}", token.trim())
            .parse()
            .map_err(|_| "the daemon's token is not a valid header value".to_string())?;
        request.metadata_mut().insert("authorization", value);
    }
    if let Some(author) = env_text("PC_AUTHOR")?.filter(|a| !a.trim().is_empty()) {
        let value = author
            .trim()
            .parse()
            .map_err(|_| format!("the author {author:?} is not a valid header value"))?;
        request.metadata_mut().insert(AUTHOR_HEADER, value);
    }
    let mut replies = client
        .run_turn(request)
        .await
        .map_err(|e| format!("the agent refused the turn: {}", e.message()))?
        .into_inner();
    let mut out = std::io::stdout();
    while let Some(reply) = replies
        .message()
        .await
        .map_err(|e| format!("the turn failed: {}", e.message()))?
    {
        match reply.event {
            Some(Event::Text(text)) => {
                print!("{text}");
                out.flush().map_err(|e| e.to_string())?;
            }
            Some(Event::Thinking(_) | Event::Started(_)) | None => {}
            Some(Event::ToolCall(call)) => eprintln!("\n[tool] {}", call.name),
            Some(Event::ToolResult(result)) if result.is_error => {
                eprintln!(
                    "[tool failed] {}",
                    result.content.lines().next().unwrap_or("")
                );
            }
            Some(Event::ToolResult(_)) => {}
            Some(Event::Retrying(retry)) => {
                eprintln!(
                    "[retrying in {} ms] {}: {}",
                    retry.after_ms, retry.kind, retry.message
                );
            }
            Some(Event::End(end)) => {
                println!();
                return match end.stop.as_str() {
                    "end_turn" | "max_tokens" => {
                        eprintln!("[{} after {} model call(s)]", end.stop, end.steps);
                        Ok(())
                    }
                    stop => Err(format!("the turn ended: {stop} {}", end.detail)),
                };
            }
        }
    }
    Err("the turn's stream ended without its end".to_string())
}
