// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The events the memory module answers (ARCHITECTURE section 7): a session
//! starts with the user and project cards in front of it, an agent is told
//! what memory knows about a file once, on its first read of it or else
//! before its first edit, and a finished turn is captured into
//! the session's message stream. Every one fails open: memory never gets in
//! the way of the session.

use pcx_module_api::json::object_from_wire;
use pcx_module_api::{
    Event, EventKind, FailurePolicy, HookSpecificOutput, Outcome, Scope, Subscription, event_kind,
};
use post_cortex_core::core::card::{Card, CardScope};
use post_cortex_core::core::context_update::RelationType;
use post_cortex_core::core::message::{MessageRole, NewMessage};
use post_cortex_core::services::{AppendMessagesRequest, GetCardRequest};
use serde::Deserialize;
use uuid::Uuid;

use crate::Memory;
use crate::capabilities::{Moment, Telling};
use crate::graph_views::{FileView, ImpactItem, RecordView, ViewRequest};
use crate::tools::Failure;

/// Records per list in a file's memory.
const PER_LIST: usize = 3;
/// Dependents named in a file's memory.
const DEPENDENTS: usize = 10;
/// Hops of impact analysis for a file's dependents.
const FILE_DEPTH: usize = 2;
/// Characters of a record's description in a file's memory.
const DESCRIPTION_CHARS: usize = 200;

/// The events the module subscribes to, each fail-open within its
/// deadline.
pub fn subscriptions() -> Vec<Subscription> {
    [
        (EventKind::SessionStart, 5_000),
        (EventKind::PreTool, 3_000),
        (EventKind::PostTool, 3_000),
        (EventKind::TurnEnd, 30_000),
    ]
    .into_iter()
    .map(|(kind, deadline_ms)| Subscription {
        kind: kind.into(),
        deadline_ms,
        policy: FailurePolicy::FailOpen.into(),
        memory: None,
    })
    .collect()
}

/// Answer `event`.
///
/// # Errors
/// The scope lacks the workspace or session the event needs, the event is
/// not one the module answers, or memory failed.
pub async fn answer(memory: &Memory, scope: &Scope, event: &Event) -> Result<Outcome, Failure> {
    match event_kind(&event.hook_event_name) {
        Some(EventKind::SessionStart) => cards(memory, scope, event).await,
        Some(EventKind::PreTool) => file(memory, scope, event, Moment::Edit).await,
        Some(EventKind::PostTool) => file(memory, scope, event, Moment::Read).await,
        Some(EventKind::TurnEnd) => capture(memory, scope, event).await,
        _ => Err(Failure::invalid(format!(
            "the memory module does not answer {}",
            event.hook_event_name
        ))
        .parameter("hook_event_name")),
    }
}

/// The user card and the project card, or how to write the project card.
async fn cards(memory: &Memory, scope: &Scope, event: &Event) -> Result<Outcome, Failure> {
    let workspace_id = id(scope.workspace_id.as_deref(), "workspace_id")?;
    let card = |scope| async move {
        memory
            .cards
            .get(GetCardRequest { scope, history: 0 })
            .await
            .map(|r| r.card)
            .map_err(Failure::from)
    };
    let person = post_cortex_core::core::card::person(scope.person.as_deref())
        .map_err(|e| Failure::invalid(e.to_string()))?;
    let user = card(CardScope::user_of(person)).await?;
    let project = card(CardScope::Workspace(workspace_id)).await?;
    Ok(context(
        event,
        render_cards(user.as_ref(), project.as_ref(), workspace_id),
    ))
}

/// The cards as the session reads them.
fn render_cards(user: Option<&Card>, project: Option<&Card>, workspace_id: Uuid) -> String {
    let mut parts = Vec::new();
    for (card, title) in [(user, "User card"), (project, "Project card")] {
        if let Some(card) = card
            && !card.text.trim().is_empty()
        {
            parts.push(format!(
                "### {title} (v{})\n{}",
                card.version,
                card.text.trim()
            ));
        }
    }
    if project.is_none_or(|card| card.text.trim().is_empty()) {
        parts.push(format!(
            "No project card yet. Once you know the stack, conventions and current goals, \
             write a short one (max 2 KB): manage_workspace action update_card, \
             workspace_id {workspace_id}, expected_version 0."
        ));
    }
    format!("## post-cortex cards\n\n{}", parts.join("\n\n"))
}

/// What memory knows about the file a tool read (PostToolUse) or is about
/// to change (PreToolUse): problems solved there, decisions about it, what
/// depends on it and the code changed with it, told once per agent session
/// (the event's `session_id`) and file. A read adds it to what the agent
/// reads; an edit of a file not told yet is blocked once with it as the
/// reason, so the agent reads it before anything is written. Nothing known,
/// a file told already, or a tool without a file adds nothing.
async fn file(
    memory: &Memory,
    scope: &Scope,
    event: &Event,
    moment: Moment,
) -> Result<Outcome, Failure> {
    let Some(path) = event
        .tool_input
        .clone()
        .map(object_from_wire)
        .and_then(|input| input.get("file_path")?.as_str().map(str::to_string))
    else {
        return Ok(Outcome::default());
    };
    let workspace_id = id(scope.workspace_id.as_deref(), "workspace_id")?;
    let request = ViewRequest::File {
        path: path.clone(),
        depth: FILE_DEPTH,
        limit: PER_LIST,
    };
    let telling = memory
        .telling
        .tell(workspace_id, &event.session_id, request, moment)
        .await?;
    Ok(match telling {
        Telling::Nothing => Outcome::default(),
        Telling::Add(view) => {
            render_file(&view, &path).map_or_else(Outcome::default, |text| context(event, text))
        }
        Telling::Hold(view) => {
            render_file(&view, &path).map_or_else(Outcome::default, |text| Outcome {
                decision: BLOCK.to_string(),
                reason: format!("{text}\n\n{HELD}"),
                hook_specific_output: None,
            })
        }
    })
}

/// The decision that blocks a tool call.
const BLOCK: &str = "block";

/// What a held edit says after the file's memory.
const HELD: &str = "This is said once a session for this file, before its first edit: \
     check your change against it, then send the edit again (or a better one) and it goes through.";

/// A file's memory as the agent reads it; none when nothing is known.
fn render_file(view: &FileView, path: &str) -> Option<String> {
    let list = |title: &str, records: &[RecordView]| {
        (!records.is_empty()).then(|| {
            let lines: Vec<String> = records.iter().take(PER_LIST).map(record_line).collect();
            format!("{title}:\n{}", lines.join("\n"))
        })
    };
    let mut parts: Vec<String> = [
        list("Problems solved here before", &view.problems),
        list("Decisions and requirements about it", &view.decisions),
    ]
    .into_iter()
    .flatten()
    .collect();
    if !view.dependents.is_empty() {
        let lines: Vec<String> = view
            .dependents
            .iter()
            .take(DEPENDENTS)
            .map(dependent_line)
            .collect();
        parts.push(format!("Depends on it:\n{}", lines.join("\n")));
    }
    if !view.changed_with.is_empty() {
        let names: Vec<&str> = view
            .changed_with
            .iter()
            .take(PER_LIST)
            .map(|n| n.name.as_str())
            .collect();
        parts.push(format!("Changed with it: {}", names.join(", ")));
    }
    (!parts.is_empty()).then(|| format!("post-cortex memory for {path}:\n\n{}", parts.join("\n\n")))
}

/// One dependent of a file's memory, with the relation that makes it one:
/// `- B (depends on A)`; `A required_by B` reads `B (requires A)`.
fn dependent_line(d: &ImpactItem) -> String {
    let how = if d.relation_type == RelationType::RequiredBy.as_str() {
        "requires".to_string()
    } else {
        d.relation_type.replace('_', " ")
    };
    format!("- {} ({how} {})", d.entity, d.via)
}

/// One record of a file's memory.
fn record_line(record: &RecordView) -> String {
    let mut text = record.title.clone();
    let description = record.description.trim();
    if !description.is_empty() {
        text.push_str(": ");
        text.extend(description.chars().take(DESCRIPTION_CHARS));
    }
    if record.validity.valid_to.is_some() {
        let reason = record.validity.reason.map_or("ended", |r| r.as_str());
        text.push_str(&format!(" (no longer holds: {reason})"));
    }
    format!(
        "- {} {text} [entry {}]",
        record.timestamp.format("%Y-%m-%d"),
        record.id
    )
}

/// The turns a client captured (the event's `payload.messages`) appended to
/// the scope's session; turns already stored are skipped.
async fn capture(memory: &Memory, scope: &Scope, event: &Event) -> Result<Outcome, Failure> {
    let session_id = id(scope.session_id.as_deref(), "session_id")?;
    let payload = event.payload.clone().map(object_from_wire);
    let Some(messages) = payload.and_then(|mut p| p.remove("messages")) else {
        return Ok(Outcome::default());
    };
    let messages: Vec<CapturedMessage> = serde_json::from_value(messages).map_err(|e| {
        Failure::invalid(format!("captured messages: {e}")).parameter("payload.messages")
    })?;
    let now = chrono::Utc::now();
    memory
        .messages
        .append(AppendMessagesRequest {
            session_id,
            messages: messages.into_iter().map(|m| m.into_new(now)).collect(),
        })
        .await?;
    Ok(Outcome::default())
}

/// One captured turn as a client sends it; see [`NewMessage`].
#[derive(Debug, Deserialize)]
pub struct CapturedMessage {
    turn_id: String,
    role: MessageRole,
    #[serde(default)]
    author: Option<String>,
    #[serde(default)]
    source_session: Option<String>,
    /// The time of the request when the client sends none.
    #[serde(default)]
    timestamp: Option<chrono::DateTime<chrono::Utc>>,
    text: String,
    #[serde(default)]
    tools: Vec<String>,
    #[serde(default)]
    files: Vec<String>,
}

impl CapturedMessage {
    /// The turn to store, received at `now`.
    pub fn into_new(self, now: chrono::DateTime<chrono::Utc>) -> NewMessage {
        NewMessage {
            turn_id: self.turn_id,
            role: self.role,
            author: self.author,
            source_session: self.source_session,
            timestamp: self.timestamp.unwrap_or(now),
            text: self.text,
            tools: self.tools,
            files: self.files,
            parent: None,
            blocks: Vec::new(),
        }
    }
}

/// The outcome that adds `text` to the model's context.
fn context(event: &Event, text: String) -> Outcome {
    Outcome {
        hook_specific_output: Some(HookSpecificOutput {
            hook_event_name: event.hook_event_name.clone(),
            additional_context: text,
        }),
        ..Outcome::default()
    }
}

/// The id the scope names as `field`.
fn id(value: Option<&str>, field: &str) -> Result<Uuid, Failure> {
    let value = value.ok_or_else(|| {
        Failure::invalid(format!("the event needs the scope's {field}")).parameter(field)
    })?;
    Uuid::parse_str(value).map_err(|e| {
        Failure::invalid(format!("{field}: invalid UUID: {e}"))
            .parameter(field)
            .expected("UUID")
            .received(value)
    })
}

#[cfg(test)]
mod tests;
