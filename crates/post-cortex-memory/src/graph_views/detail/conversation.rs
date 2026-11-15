// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Captured turns with the records drawn from them, the summary of a
//! session's stream and a page of it.

use post_cortex_core::core::conversation::ConversationState;
use std::collections::HashMap;

use chrono::{DateTime, Utc};
use post_cortex_core::core::message::{self, Message, MessageStats};
use uuid::Uuid;

use crate::graph_views::Index;
use crate::graph_views::detail::types::PageCursor;

/// A captured turn with the records drawn from it.
#[derive(Clone, Debug)]
pub struct TurnDetail {
    /// The turn.
    pub message: Message,
    /// Records drawn from it (their `derived_from` names it), sorted.
    pub records: Vec<Uuid>,
}

/// One session's message stream.
#[derive(Clone, Debug)]
pub struct ConversationSummary {
    /// The session.
    pub session_id: Uuid,
    /// Its display name (empty when unnamed).
    pub name: String,
    /// Messages in the stream.
    pub messages: u64,
    /// The first message's time.
    pub first_at: Option<DateTime<Utc>>,
    /// The latest message's time.
    pub last_at: Option<DateTime<Utc>>,
    /// Authors of its newest messages (see `Views::conversations`), sorted.
    pub authors: Vec<String>,
    /// Records drawn from its messages.
    pub derived_records: usize,
    /// For an agent's conversation (listed by its speaker, W25): who asked
    /// its first question; none when its oldest messages hold no prompt.
    pub asker: Option<String>,
    /// That first question.
    pub first_question: Option<String>,
    /// How it stands; none when not listed by a speaker, or its newest
    /// messages hold no prompt.
    pub state: Option<ConversationState>,
}

/// A page of a scope's streams, most recently active first (then by
/// session, descending).
#[derive(Clone, Debug)]
pub struct Conversations {
    /// At most the limit.
    pub conversations: Vec<ConversationSummary>,
    /// Streams over all pages.
    pub total: usize,
    /// Where the next page starts; `None` on the last.
    pub next: Option<PageCursor<Uuid>>,
}

/// Where a page of a stream starts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PageAt {
    /// Messages from this position on.
    From(u64),
    /// Messages just before this position (`u64::MAX`: the newest page).
    Before(u64),
}

/// A page of one session's stream, in stream order.
#[derive(Clone, Debug)]
pub struct ConversationPage {
    /// The session.
    pub session_id: Uuid,
    /// Its display name (empty when unnamed).
    pub name: String,
    /// In stream order.
    pub turns: Vec<TurnDetail>,
    /// `PageAt::From` of the next (newer) page; `None` on the last.
    pub next: Option<u64>,
    /// `PageAt::Before` of the previous (older) page; `None` on the first.
    pub prev: Option<u64>,
    /// Messages the whole stream holds.
    pub total: u64,
}

/// (session, `message:<seq>`) -> the records drawn from that turn, sorted.
pub type DrawnFrom = HashMap<(Uuid, String), Vec<Uuid>>;

impl Index<'_> {
    /// Which records each captured turn of the scope was drawn into.
    pub fn drawn_from(&self) -> DrawnFrom {
        let mut out: DrawnFrom = HashMap::new();
        for (id, (session_id, u)) in &self.records {
            for source in &u.derived_from {
                out.entry((*session_id, source.clone()))
                    .or_default()
                    .push(*id);
            }
        }
        for ids in out.values_mut() {
            ids.sort();
        }
        out
    }
}

/// `message` with the records `drawn` says were drawn from it.
pub fn turn(message: Message, drawn: &DrawnFrom) -> TurnDetail {
    let records = drawn
        .get(&(message.session_id, message::content_id(message.seq)))
        .cloned()
        .unwrap_or_default();
    TurnDetail { message, records }
}

/// The cursors of the pages on either side of a page at `at`: `(next,
/// prev)`. `first` and `last` are the positions the page holds (`None` when
/// empty: it sits at its cursor), `more` whether the store had a message
/// past the page's far end. Gaps left by retention are skipped, so a page is
/// full while the stream has messages on that side.
pub fn neighbours(
    at: PageAt,
    first: Option<u64>,
    last: Option<u64>,
    more: bool,
    stats: &MessageStats,
) -> (Option<u64>, Option<u64>) {
    // The positions the page spans, [low, high).
    let (low, high) = match (first, last) {
        (Some(first), Some(last)) => (first, last.saturating_add(1)),
        _ => {
            let (PageAt::From(seq) | PageAt::Before(seq)) = at;
            (seq, seq)
        }
    };
    match at {
        PageAt::From(_) => (
            more.then_some(high),
            stats.first_seq.filter(|&f| f < low).map(|_| low),
        ),
        PageAt::Before(_) => (
            stats.last_seq.filter(|&l| l >= high).map(|_| high),
            more.then_some(low),
        ),
    }
}
