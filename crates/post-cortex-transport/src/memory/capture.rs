// Copyright (c) 2025, 2026 Julius ML
// MIT License

//! gRPC side of conversation capture (AppendMessages, ListMessages: an
//! agent's turns with their blocks) and project / user cards (GetCard,
//! UpdateCard), translated to and from the canonical service.

use chrono::{TimeZone, Utc};
use post_cortex_core::core::card::{Card, CardScope};
use post_cortex_core::core::message::{Message, MessageRole, NewMessage};
use post_cortex_core::services as svc;
use post_cortex_memory::Memory;
use tonic::Status;

use crate::grpc::status;
use crate::memory::helpers::parse_uuid;
use post_cortex_proto::pb;

pub async fn append_messages(
    memory: &Memory,
    req: pb::AppendMessagesRequest,
) -> Result<pb::AppendMessagesResponse, Status> {
    let now = Utc::now();
    let messages = req
        .messages
        .into_iter()
        .map(|m| {
            let role: MessageRole = post_cortex_memory::tools::wire::parse("role", &m.role)
                .map_err(crate::grpc::status)?;
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
                parent: m.parent,
                blocks: m
                    .blocks
                    .into_iter()
                    .map(crate::blocks::from_wire)
                    .collect::<Result<_, Status>>()?,
            })
        })
        .collect::<Result<Vec<_>, Status>>()?;
    let response = memory
        .messages
        .append(svc::AppendMessagesRequest {
            session_id: parse_uuid(&req.session_id)?,
            messages,
        })
        .await
        .map_err(status)?;
    // `vectorized` is deprecated (always 0): its default is sent.
    Ok(pb::AppendMessagesResponse {
        appended: response.appended,
        skipped: response.skipped as u32,
        queued: response.queued as u32,
        ..Default::default()
    })
}

pub async fn list_messages(
    memory: &Memory,
    req: pb::ListMessagesRequest,
) -> Result<pb::MessagePage, Status> {
    let limit = (req.limit != 0).then_some(req.limit);
    let page = memory
        .messages
        .stream(parse_uuid(&req.session_id)?, req.from, limit)
        .await
        .map_err(status)?;
    Ok(pb::MessagePage {
        messages: page
            .messages
            .into_iter()
            .map(stored)
            .collect::<Result<_, Status>>()?,
        next_from: page.next_from,
    })
}

/// A stored turn as the wire carries it.
fn stored(message: Message) -> Result<pb::StoredMessage, Status> {
    Ok(pb::StoredMessage {
        position: message.seq,
        message: Some(pb::CapturedMessage {
            turn_id: message.turn_id,
            role: message.role.as_str().to_string(),
            author: message.author,
            source_session: message.source_session,
            timestamp_unix_ms: Some(message.timestamp.timestamp_millis()),
            text: message.text,
            tools: message.tools,
            files: message.files,
            parent: message.parent,
            blocks: message
                .blocks
                .into_iter()
                .map(crate::blocks::to_wire)
                .collect::<Result<_, Status>>()?,
        }),
    })
}

pub async fn get_card(
    memory: &Memory,
    req: pb::GetCardRequest,
    person: Option<uuid::Uuid>,
) -> Result<pb::GetCardResponse, Status> {
    let response = memory
        .cards
        .get(svc::GetCardRequest {
            scope: card_scope(req.workspace_id.as_deref(), req.user, person)?,
            history: req.history as usize,
        })
        .await
        .map_err(status)?;
    Ok(pb::GetCardResponse {
        card: response.card.as_ref().map(card_info),
        history: response.history.iter().map(card_info).collect(),
    })
}

pub async fn update_card(
    memory: &Memory,
    req: pb::UpdateCardRequest,
    person: Option<uuid::Uuid>,
) -> Result<pb::UpdateCardResponse, Status> {
    let response = memory
        .cards
        .update(svc::UpdateCardRequest {
            scope: card_scope(req.workspace_id.as_deref(), req.user, person)?,
            text: req.content,
            expected_version: req.expected_version,
            author: req.author,
        })
        .await
        .map_err(status)?;
    Ok(pb::UpdateCardResponse {
        written: response.written,
        card: response.card.as_ref().map(card_info),
    })
}

/// The project card of `workspace_id`, or the user card (`person`'s, the
/// host's for none) with `user`.
fn card_scope(
    workspace_id: Option<&str>,
    user: bool,
    person: Option<uuid::Uuid>,
) -> Result<CardScope, Status> {
    match (workspace_id.filter(|s| !s.is_empty()), user) {
        (Some(_), true) => Err(Status::invalid_argument(
            "set either workspace_id (project card) or user (user card), not both",
        )),
        (Some(id), false) => Ok(CardScope::Workspace(parse_uuid(id)?)),
        (None, true) => Ok(CardScope::user_of(person)),
        (None, false) => Err(Status::invalid_argument(
            "set workspace_id (project card) or user (user card)",
        )),
    }
}

pub fn card_info(card: &Card) -> pb::CardInfo {
    pb::CardInfo {
        scope: card.scope.key(),
        version: card.version,
        text: card.text.clone(),
        updated_at_unix: card.updated_at.timestamp(),
        author: card.author.clone(),
    }
}
