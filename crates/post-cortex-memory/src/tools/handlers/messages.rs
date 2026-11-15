// Copyright (c) 2026 Julius ML
// MIT License

//! Tool `manage_messages`: a session's message stream, for an agent that
//! keeps its turns whole (ARCHITECTURE section 7, the agent): append turns
//! with their parent and blocks, read the stream back a page at a time.

use post_cortex_core::core::message::{MessageBlock, NewMessage};
use post_cortex_core::services::AppendMessagesRequest;
use serde_json::Value;

use crate::Memory;
use crate::capabilities::MessageError;
use crate::tools::args::{ManageMessagesRequest, MessageInput};
use crate::tools::handlers::Outcome;
use crate::tools::parse::{FieldHint, arguments};
use crate::tools::responses::{MessagePage, MessagesAppended};
use crate::tools::{Failure, check, reply};

const TOOL: &str = "manage_messages";

const ACTION_HINT: &str =
    "Valid actions: append (session_id, messages), page (session_id, optional from, limit)";

const HINTS: &[FieldHint] = &[FieldHint {
    field: "action",
    expected: "one of: append, page",
    hint: ACTION_HINT,
}];

/// `author` writes turns that name no author.
pub async fn handle(memory: &Memory, params: Value, author: Option<String>) -> Outcome {
    let req: ManageMessagesRequest = arguments(params, HINTS)?;
    let action = crate::tools::handlers::action(TOOL, &req.action, ACTION_HINT)?;
    let session_id = check::session_id(&req.session_id)?;
    match action.as_str() {
        "append" => {
            let given = req.messages.ok_or_else(|| {
                Failure::invalid("append needs messages: the turns, in order").parameter("messages")
            })?;
            let now = chrono::Utc::now();
            let messages = given
                .into_iter()
                .enumerate()
                .map(|(i, m)| new_message(i, m, author.clone(), now))
                .collect::<Result<Vec<_>, Failure>>()?;
            let stored = memory
                .messages
                .append(AppendMessagesRequest {
                    session_id,
                    messages,
                })
                .await
                .map_err(Failure::from)?;
            reply::with(
                format!(
                    "Appended {} turn(s) to session {session_id} ({} skipped)",
                    stored.appended.len(),
                    stored.skipped
                ),
                &MessagesAppended {
                    appended: stored.appended,
                    skipped: stored.skipped,
                },
            )
        }
        "page" => {
            let from = req.from.unwrap_or(0);
            let page = memory
                .messages
                .stream(session_id, from, req.limit)
                .await
                .map_err(|e| match e {
                    MessageError::InvalidArgument(_) => Failure::from(e).parameter("limit"),
                    e => Failure::from(e),
                })?;
            reply::with(
                format!(
                    "{} turn(s) of session {session_id} from position {from}",
                    page.messages.len()
                ),
                &MessagePage {
                    messages: page.messages,
                    next_from: page.next_from,
                },
            )
        }
        other => Err(crate::tools::handlers::unhandled(TOOL, other)),
    }
}

/// Turn `index` of an append as memory stores it.
fn new_message(
    index: usize,
    input: MessageInput,
    author: Option<String>,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<NewMessage, Failure> {
    let at = |field: &str| format!("messages[{index}].{field}");
    let role = input.role.parse().map_err(|_| {
        Failure::invalid(format!(
            "{}: user or assistant, not {}",
            at("role"),
            input.role
        ))
        .parameter("messages")
    })?;
    let blocks = input
        .blocks
        .unwrap_or_default()
        .into_iter()
        .enumerate()
        .map(|(n, block)| {
            serde_json::from_value::<MessageBlock>(block).map_err(|e| {
                Failure::invalid(format!("{}: {e}", at(&format!("blocks[{n}]"))))
                    .parameter("messages")
            })
        })
        .collect::<Result<Vec<_>, Failure>>()?;
    Ok(NewMessage {
        turn_id: input.turn_id,
        role,
        author: crate::tools::handlers::author(input.author.as_deref(), author),
        source_session: None,
        timestamp: now,
        text: input.text.unwrap_or_default(),
        tools: input.tools.unwrap_or_default(),
        files: Vec::new(),
        parent: input.parent,
        blocks,
    })
}

#[cfg(test)]
mod tests;
