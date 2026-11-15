// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Placing restored messages into a stream: the positions both adapters'
//! `restore_messages` give the turns of an imported archive.

use post_cortex_core::core::message::Message;
use std::collections::HashSet;
use uuid::Uuid;

/// Positions for messages restored into a stream whose next position is
/// `next` and which stores the turns `known`: by archived `seq`, a turn
/// stored already or repeated is left out, the others keep their `seq` when
/// it is at or past the next position (never used in this stream) and go
/// to the end otherwise. Returns `(archived seq, message as stored)` and the
/// stream's new next position.
pub(crate) fn place_restored_messages(
    session_id: Uuid,
    mut messages: Vec<Message>,
    mut next: u64,
    known: &HashSet<String>,
) -> (Vec<(u64, Message)>, u64) {
    messages.sort_by_key(|m| m.seq);
    let mut seen = HashSet::new();
    let mut placed = Vec::new();
    for mut message in messages {
        if known.contains(&message.turn_id) || !seen.insert(message.turn_id.clone()) {
            continue;
        }
        let from = message.seq;
        message.seq = from.max(next);
        message.session_id = session_id;
        next = message.seq.saturating_add(1);
        placed.push((from, message));
    }
    (placed, next)
}
