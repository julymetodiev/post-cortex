// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The conversations of [`DetailViews`]: a scope's message streams, a page
//! at a time, and pages of one stream with the records drawn from each
//! turn.

use std::cmp::Reverse;
use std::sync::Arc;

use post_cortex_core::core::conversation;
use post_cortex_core::core::message::{self, MessageCursor, MessageOrder};
use uuid::Uuid;

use crate::capabilities::scope::{self, Target};
use crate::capabilities::views::ViewsError;
use crate::capabilities::views::details::{self, AUTHOR_SCAN, DetailViews, on_index};
use crate::capabilities::views::streams;
use crate::graph_views::GraphScope;
use crate::graph_views::detail::{
    ConversationPage, ConversationSummary, Conversations, PageAt, PageCursor, neighbours,
    page_after, turn,
};

/// Messages read from the start of a listed conversation for its first
/// question.
const FIRST_SCAN: usize = 20;

impl DetailViews {
    /// A page of the sessions of `target` with captured messages, most
    /// recently active first, after `cursor`, at most `limit` (default
    /// 100), with each stream's stats and authors from its summary (one
    /// read, no message). With `speaker` (W25), only an agent's
    /// conversations: the streams whose newest [`AUTHOR_SCAN`] messages it
    /// wrote in, each with who asked its first question and how it stands.
    ///
    /// # Errors
    /// The scope or a message stream could not be read.
    pub async fn conversations(
        &self,
        target: Target,
        limit: Option<usize>,
        cursor: Option<PageCursor<Uuid>>,
        speaker: Option<&str>,
    ) -> Result<Conversations, ViewsError> {
        let max = details::limit(limit);
        let scope = self.scope(target).await?;
        let ids: Vec<Uuid> = scope.sessions.iter().map(|s| s.id()).collect();
        let stats = streams::stats(&self.messages, ids).await?;
        // A stream without a newest message has no turns to show.
        let mut active: Vec<_> = scope
            .sessions
            .iter()
            .zip(stats)
            .filter_map(|(session, stats)| {
                let at = stats.last_at?;
                Some((
                    PageCursor {
                        at,
                        key: session.id(),
                    },
                    session,
                    stats,
                ))
            })
            .collect();
        if let Some(speaker) = speaker {
            let ids: Vec<Uuid> = active.iter().map(|(_, s, _)| s.id()).collect();
            let newest = streams::newest(&self.messages, ids, AUTHOR_SCAN).await?;
            active = active
                .into_iter()
                .zip(newest)
                .filter(|(_, newest)| conversation::spoke(newest, speaker))
                .map(|(item, _)| item)
                .collect();
        }
        active.sort_by_key(|(at, _, _)| Reverse(*at));
        let total = active.len();
        let (page, next) = page_after(active, |(at, _, _)| *at, cursor.as_ref(), max);
        // Turns are read only for a speaker's conversations: how each stands
        // and who asked first. The authors come with the stats.
        let (newest, oldest) = match speaker {
            Some(_) => {
                let page_ids: Vec<Uuid> = page.iter().map(|(_, s, _)| s.id()).collect();
                (
                    streams::newest(&self.messages, page_ids.clone(), AUTHOR_SCAN).await?,
                    streams::oldest(&self.messages, page_ids, FIRST_SCAN).await?,
                )
            }
            None => (vec![Vec::new(); page.len()], vec![Vec::new(); page.len()]),
        };
        let conversations = page
            .into_iter()
            .zip(newest)
            .zip(oldest)
            .map(|(((_, session, stats), newest), oldest)| {
                let state = speaker.and_then(|s| conversation::state(&newest, s));
                let first = speaker.and_then(|s| conversation::first_prompt(&oldest, s));
                let (asker, first_question) = match first {
                    Some(m) => (m.author.clone(), Some(m.text.clone())),
                    None => (None, None),
                };
                ConversationSummary {
                    asker,
                    first_question,
                    state,
                    session_id: session.id(),
                    name: session.name().unwrap_or_default(),
                    messages: stats.count,
                    first_at: stats.first_at,
                    last_at: stats.last_at,
                    authors: stats.authors,
                    derived_records: session
                        .records()
                        .iter()
                        .filter(|u| {
                            u.derived_from
                                .iter()
                                .any(|s| message::seq_of_content_id(s).is_some())
                        })
                        .count(),
                }
            })
            .collect();
        Ok(Conversations {
            conversations,
            total,
            next,
        })
    }

    /// A page of one session's stream at `at` (default 100 messages), each
    /// turn with the records drawn from it.
    ///
    /// # Errors
    /// The session does not exist or its stream could not be read.
    pub async fn conversation(
        &self,
        session_id: Uuid,
        at: PageAt,
        limit: Option<usize>,
    ) -> Result<ConversationPage, ViewsError> {
        let max = details::limit(limit);
        let session = scope::load(&self.views.rt, session_id).await?;
        let stats = self.messages.stats(session_id).await?;
        // One extra message says whether the page has a neighbour on its
        // far side.
        let (cursor, order) = match at {
            PageAt::From(seq) => (MessageCursor::From(seq), MessageOrder::Asc),
            PageAt::Before(seq) => (MessageCursor::Before(seq), MessageOrder::Desc),
        };
        let mut messages = self
            .messages
            .page(session_id, cursor, max + 1, order)
            .await?;
        let more = messages.len() > max;
        messages.truncate(max);
        if order == MessageOrder::Desc {
            messages.reverse();
        }
        let (next, prev) = neighbours(
            at,
            messages.first().map(|m| m.seq),
            messages.last().map(|m| m.seq),
            more,
            &stats,
        );
        let name = session.name().unwrap_or_default();
        let scope = GraphScope {
            graph: Arc::clone(session.entity_graph_shared()),
            sessions: vec![session],
        };
        let drawn = on_index(scope, |index| index.drawn_from()).await?;
        Ok(ConversationPage {
            session_id,
            name,
            turns: messages.into_iter().map(|m| turn(m, &drawn)).collect(),
            next,
            prev,
            total: stats.count,
        })
    }
}
