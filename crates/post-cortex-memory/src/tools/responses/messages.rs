// Copyright (c) 2026 Julius ML
// MIT License

//! Response shapes of `manage_messages`.

use post_cortex_core::core::message::Message;
use serde::Serialize;

/// What an append stored.
#[derive(Debug, Serialize)]
pub struct MessagesAppended {
    /// Positions of the turns stored, in order.
    pub appended: Vec<u64>,
    /// Turns not stored: already stored, repeated, or empty.
    pub skipped: usize,
}

/// A page of a stream, oldest first.
#[derive(Debug, Serialize)]
pub struct MessagePage {
    /// The turns.
    pub messages: Vec<Message>,
    /// Where the next page starts; none on the last.
    pub next_from: Option<u64>,
}
