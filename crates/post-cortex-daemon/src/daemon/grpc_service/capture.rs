// Copyright (c) 2025, 2026 Julius ML
// MIT License

//! gRPC side of ROADMAP Phase 2: conversation capture (AppendMessages) and
//! project / user cards (GetCard, UpdateCard), translated to and from the
//! canonical service.

use chrono::{TimeZone, Utc};
use post_cortex_core::core::card::{Card, CardScope};
use post_cortex_core::core::message::{MessageRole, NewMessage};
use post_cortex_core::services::{self as svc, PostCortexService};
use post_cortex_memory::services::MemoryServiceImpl;
use tonic::Status;

use super::helpers::{parse_uuid, system_error_to_status};
use super::pb;

pub(super) async fn append_messages(
    service: &MemoryServiceImpl,
    req: pb::AppendMessagesRequest,
) -> Result<pb::AppendMessagesResponse, Status> {
    let now = Utc::now();
    let messages = req
        .messages
        .into_iter()
        .map(|m| {
            let role = MessageRole::parse(&m.role.to_lowercase()).ok_or_else(|| {
                Status::invalid_argument(format!(
                    "role '{}': expected 'user' or 'assistant'",
                    m.role
                ))
            })?;
            let timestamp = match m.timestamp_unix_ms {
                Some(ms) => Utc.timestamp_millis_opt(ms).single().ok_or_else(|| {
                    Status::invalid_argument(format!("timestamp {ms} out of range"))
                })?,
                None => now,
            };
            Ok(NewMessage {
                turn_id: m.turn_id,
                role,
                author: m.author,
                source_session: m.source_session,
                timestamp,
                text: m.text,
                tools: m.tools,
                files: m.files,
            })
        })
        .collect::<Result<Vec<_>, Status>>()?;
    let response = service
        .append_messages(svc::AppendMessagesRequest {
            session_id: parse_uuid(&req.session_id)?,
            messages,
        })
        .await
        .map_err(system_error_to_status)?;
    Ok(pb::AppendMessagesResponse {
        appended: response.appended,
        skipped: response.skipped as u32,
        vectorized: response.vectorized as u32,
    })
}

pub(super) async fn get_card(
    service: &MemoryServiceImpl,
    req: pb::GetCardRequest,
) -> Result<pb::GetCardResponse, Status> {
    let response = service
        .get_card(svc::GetCardRequest {
            scope: card_scope(req.workspace_id.as_deref(), req.user)?,
            history: req.history as usize,
        })
        .await
        .map_err(system_error_to_status)?;
    Ok(pb::GetCardResponse {
        card: response.card.as_ref().map(card_info),
        history: response.history.iter().map(card_info).collect(),
    })
}

pub(super) async fn update_card(
    service: &MemoryServiceImpl,
    req: pb::UpdateCardRequest,
) -> Result<pb::UpdateCardResponse, Status> {
    let response = service
        .update_card(svc::UpdateCardRequest {
            scope: card_scope(req.workspace_id.as_deref(), req.user)?,
            text: req.content,
            expected_version: req.expected_version,
            author: req.author,
        })
        .await
        .map_err(system_error_to_status)?;
    Ok(pb::UpdateCardResponse {
        written: response.written,
        card: response.card.as_ref().map(card_info),
    })
}

/// The project card of `workspace_id`, or the user card with `user`.
fn card_scope(workspace_id: Option<&str>, user: bool) -> Result<CardScope, Status> {
    match (workspace_id.filter(|s| !s.is_empty()), user) {
        (Some(_), true) => Err(Status::invalid_argument(
            "set either workspace_id (project card) or user (user card), not both",
        )),
        (Some(id), false) => Ok(CardScope::Workspace(parse_uuid(id)?)),
        (None, true) => Ok(CardScope::User),
        (None, false) => Err(Status::invalid_argument(
            "set workspace_id (project card) or user (user card)",
        )),
    }
}

pub(super) fn card_info(card: &Card) -> pb::CardInfo {
    pb::CardInfo {
        scope: card.scope.key(),
        version: card.version,
        text: card.text.clone(),
        updated_at_unix: card.updated_at.timestamp(),
        author: card.author.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use post_cortex_memory::ConversationMemorySystem;
    use std::sync::Arc;

    async fn service() -> (
        MemoryServiceImpl,
        Arc<ConversationMemorySystem>,
        tempfile::TempDir,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let memory = Arc::new(
            ConversationMemorySystem::new(post_cortex_memory::SystemConfig {
                data_directory: dir.path().to_str().unwrap().to_string(),
                enable_embeddings: false,
                ..Default::default()
            })
            .await
            .unwrap(),
        );
        (MemoryServiceImpl::new(memory.clone()), memory, dir)
    }

    #[tokio::test]
    async fn append_messages_over_grpc() {
        let (svc, memory, _dir) = service().await;
        let sid = memory.create_session(None, None).await.unwrap();
        let turn = |id: &str, role: &str| pb::CapturedMessage {
            turn_id: id.to_string(),
            role: role.to_string(),
            text: format!("turn {id}"),
            timestamp_unix_ms: Some(1_760_000_000_000),
            ..Default::default()
        };
        let request = |messages| pb::AppendMessagesRequest {
            session_id: sid.to_string(),
            messages,
        };

        let bad = append_messages(&svc, request(vec![turn("x", "robot")])).await;
        assert_eq!(bad.unwrap_err().code(), tonic::Code::InvalidArgument);

        let ok = append_messages(
            &svc,
            request(vec![turn("a", "user"), turn("b", "Assistant")]),
        )
        .await
        .unwrap();
        assert_eq!(ok.appended, [0, 1]);
        let again = append_messages(&svc, request(vec![turn("a", "user")]))
            .await
            .unwrap();
        assert_eq!((again.appended.len(), again.skipped), (0, 1));
        let stored = memory
            .storage_actor
            .load_messages(sid, 0..10)
            .await
            .unwrap();
        assert_eq!(stored[0].timestamp.timestamp_millis(), 1_760_000_000_000);

        let unknown = append_messages(
            &svc,
            pb::AppendMessagesRequest {
                session_id: uuid::Uuid::new_v4().to_string(),
                messages: vec![turn("c", "user")],
            },
        )
        .await;
        assert_eq!(unknown.unwrap_err().code(), tonic::Code::NotFound);
    }

    #[tokio::test]
    async fn cards_over_grpc() {
        let (svc, memory, _dir) = service().await;
        let ws = memory
            .workspace_manager
            .create_workspace("g".to_string(), String::new());

        let neither = get_card(&svc, pb::GetCardRequest::default()).await;
        assert_eq!(neither.unwrap_err().code(), tonic::Code::InvalidArgument);

        let update = |content: &str, expected_version| pb::UpdateCardRequest {
            workspace_id: Some(ws.to_string()),
            content: content.to_string(),
            expected_version,
            ..Default::default()
        };
        let first = update_card(&svc, update("## Goals\nship", 0))
            .await
            .unwrap();
        assert!(first.written);
        assert_eq!(
            first.card.as_ref().unwrap().scope,
            format!("workspace:{ws}")
        );
        let stale = update_card(&svc, update("other", 0)).await.unwrap();
        assert!(!stale.written);
        assert_eq!(stale.card.unwrap().version, 1);

        let read = get_card(
            &svc,
            pb::GetCardRequest {
                workspace_id: Some(ws.to_string()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(read.card.unwrap().text, "## Goals\nship");

        let user = get_card(
            &svc,
            pb::GetCardRequest {
                user: true,
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert!(user.card.is_none());
    }
}
