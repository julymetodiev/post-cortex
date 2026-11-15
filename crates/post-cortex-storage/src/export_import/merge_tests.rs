// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Restored messages keep free positions in the target's stream.

use std::collections::HashSet;

use chrono::Utc;
use post_cortex_core::core::message::{Message, MessageRole};
use uuid::Uuid;

use crate::export_import::place_restored_messages;

fn archived(session_id: Uuid, seq: u64, turn: &str) -> Message {
    Message {
        session_id,
        seq,
        turn_id: turn.to_string(),
        role: MessageRole::User,
        author: None,
        source_session: None,
        timestamp: Utc::now(),
        text: format!("turn {turn}"),
        tools: vec![],
        files: vec![],
        parent: None,
        blocks: Vec::new(),
    }
}

/// Known and repeated turns are left out; a position at or past the
/// next one is kept, an earlier one goes to the end.
#[test]
fn restored_messages_keep_free_positions() {
    let (from, to) = (Uuid::new_v4(), Uuid::new_v4());
    let known: HashSet<String> = ["b".to_string()].into_iter().collect();
    let messages = vec![
        archived(from, 9, "e"),
        archived(from, 1, "b"),
        archived(from, 0, "a"),
        archived(from, 2, "c"),
        archived(from, 3, "c"),
    ];
    let (placed, next) = place_restored_messages(to, messages, 2, &known);
    let got: Vec<(u64, u64, &str)> = placed
        .iter()
        .map(|(f, m)| (*f, m.seq, m.turn_id.as_str()))
        .collect();
    assert_eq!(got, [(0, 2, "a"), (2, 3, "c"), (9, 9, "e")]);
    assert_eq!(next, 10);
    assert!(placed.iter().all(|(_, m)| m.session_id == to));
}
