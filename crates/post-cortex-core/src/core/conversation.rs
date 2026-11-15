// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! An agent's conversation (ARCHITECTURE section 7, the app's chat; W25):
//! a session's message stream in which the agent wrote as its speaker
//! (the agent, the chief, a team's member). Who asked and the first
//! question come from its first prompt; how it stands from its newest turn:
//! a prompt with no end of the agent after it runs, an end that failed
//! failed, else it is idle. A prompt is a person's turn: a user message
//! with text, not written by the speaker (whose tool results are user
//! messages too). Pure.

use crate::core::message::{Message, MessageBlock, MessageRole};

/// How an agent's conversation stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConversationState {
    /// Its newest turn ended.
    Idle,
    /// Its newest prompt has no end yet.
    Running,
    /// Its newest turn ended failing.
    Failed,
}

fn by(message: &Message, speaker: &str) -> bool {
    message.author.as_deref() == Some(speaker)
}

/// Whether `message` is a person's prompt to `speaker`.
#[must_use]
pub fn is_prompt(message: &Message, speaker: &str) -> bool {
    message.role == MessageRole::User
        && !by(message, speaker)
        && (!message.text.trim().is_empty()
            || message
                .blocks
                .iter()
                .any(|b| matches!(b, MessageBlock::Text { .. })))
}

/// The end `message` says, when it is an end of `speaker`'s turn: why it
/// stopped.
fn end_of<'a>(message: &'a Message, speaker: &str) -> Option<&'a str> {
    if !by(message, speaker) {
        return None;
    }
    message.blocks.iter().find_map(|b| match b {
        MessageBlock::End { stop, .. } => Some(stop.as_str()),
        _ => None,
    })
}

/// Whether `speaker` wrote in `messages`.
#[must_use]
pub fn spoke(messages: &[Message], speaker: &str) -> bool {
    messages.iter().any(|m| by(m, speaker))
}

/// How `speaker`'s conversation stands, from its newest `messages` (any
/// order); none when they hold no prompt of it.
#[must_use]
pub fn state(messages: &[Message], speaker: &str) -> Option<ConversationState> {
    let prompt = messages
        .iter()
        .filter(|m| is_prompt(m, speaker))
        .map(|m| m.seq)
        .max()?;
    let end = messages
        .iter()
        .filter_map(|m| end_of(m, speaker).map(|stop| (m.seq, stop)))
        .max_by_key(|(seq, _)| *seq);
    Some(match end {
        Some((seq, _)) if seq < prompt => ConversationState::Running,
        None => ConversationState::Running,
        Some((_, "failed")) => ConversationState::Failed,
        Some(_) => ConversationState::Idle,
    })
}

/// The first prompt to `speaker` among its oldest `messages` (any order).
#[must_use]
pub fn first_prompt<'a>(messages: &'a [Message], speaker: &str) -> Option<&'a Message> {
    messages
        .iter()
        .filter(|m| is_prompt(m, speaker))
        .min_by_key(|m| m.seq)
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use uuid::Uuid;

    use super::*;

    fn message(seq: u64, role: MessageRole, author: &str, blocks: Vec<MessageBlock>) -> Message {
        Message {
            session_id: Uuid::nil(),
            seq,
            turn_id: format!("t{seq}"),
            role,
            author: Some(author.to_string()),
            source_session: None,
            timestamp: Utc::now(),
            text: String::new(),
            tools: Vec::new(),
            files: Vec::new(),
            parent: None,
            blocks,
        }
    }

    fn said(text: &str) -> Vec<MessageBlock> {
        vec![MessageBlock::Text { text: text.into() }]
    }

    fn end(stop: &str) -> Vec<MessageBlock> {
        vec![MessageBlock::End {
            stop: stop.into(),
            detail: String::new(),
        }]
    }

    #[test]
    fn a_conversation_runs_until_its_turn_ends() {
        let asked = message(0, MessageRole::User, "julius", said("what is left?"));
        let tool = message(
            2,
            MessageRole::User,
            "chief",
            vec![MessageBlock::ToolResult {
                tool_use_id: "c".into(),
                content: "x".into(),
                is_error: false,
            }],
        );
        let answer = message(1, MessageRole::Assistant, "chief", said("looking"));
        let mut messages = vec![asked.clone(), answer, tool];
        assert_eq!(state(&messages, "chief"), Some(ConversationState::Running));
        assert_eq!(first_prompt(&messages, "chief").map(|m| m.seq), Some(0));
        messages.push(message(3, MessageRole::Assistant, "chief", end("end_turn")));
        assert_eq!(state(&messages, "chief"), Some(ConversationState::Idle));
        messages.push(message(4, MessageRole::User, "julius", said("and now?")));
        assert_eq!(state(&messages, "chief"), Some(ConversationState::Running));
        messages.push(message(5, MessageRole::Assistant, "chief", end("failed")));
        assert_eq!(state(&messages, "chief"), Some(ConversationState::Failed));
        assert!(spoke(&messages, "chief"));
        assert!(!spoke(&messages, "reviewer"));
        assert_eq!(state(&[], "chief"), None);
    }
}
