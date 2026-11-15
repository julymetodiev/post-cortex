// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Watching a scope's records as they are stored: the records written after
//! a time first, then each record change the change log announces for the
//! scope's sessions, every record in full; when asked (W25), each message
//! appended to the scope's sessions too, as it was stored (an agent's turn
//! that a person did not start, followed in the app: its prompt starts it,
//! its end block ends it).
//!
//! A watcher that falls behind the change log loses changes; it then
//! re-reads the scope and sends the records it has not sent (and, with
//! `changes`, removals of those it knew), so written records are never
//! missed. Forgets that happened while it lagged are not replayed.

use std::collections::{HashMap, HashSet};
use std::pin::Pin;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use futures::Stream;
use tokio::sync::{broadcast, mpsc};
use uuid::Uuid;

use post_cortex_core::core::message::{MessageCursor, MessageOrder};

use crate::capabilities::messages::Messages;
use crate::capabilities::scope::{self, Target};
use crate::capabilities::views::ViewsError;
use crate::capabilities::workspaces::Workspaces;
use crate::graph_views::detail::{RecordDetail, TurnDetail, turn};
use crate::graph_views::{GraphScope, Index};
use crate::memory_system::{
    Change, MemoryRuntime, MessagesChange, MessagesChangeKind, RecordEvent, RecordEventKind,
};

/// Changes taken from the log and answered together, so one session index
/// serves a burst of writes.
const BATCH: usize = 256;

/// Events a watch buffers for its client.
const BUFFERED: usize = 16;

/// What happened, as a watch tells it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActivityKind {
    /// A record was written, forgotten or removed.
    Record(RecordEventKind),
    /// A message was appended (W25).
    Message,
}

/// A record change, or a message appended, as the app shows it.
#[derive(Clone, Debug)]
pub struct ActivityEvent {
    /// The record as it is now; `None` when removed, and for a message.
    pub record: Option<RecordDetail>,
    /// What happened.
    pub kind: ActivityKind,
    /// The record; `None` for a message.
    pub record_id: Option<Uuid>,
    /// Its session (`None` for a removal found by a re-read).
    pub session_id: Option<Uuid>,
    /// When: the record's time when written, its retirement when forgotten,
    /// the deletion when removed; a message's own time.
    pub at: DateTime<Utc>,
    /// The message appended, as stored.
    pub message: Option<TurnDetail>,
}

/// What a watch sends.
#[derive(Clone, Copy, Debug, Default)]
pub struct WatchRequest {
    /// Also send the records written after this, first (`None`: only
    /// records stored from the call on).
    pub since: Option<DateTime<Utc>>,
    /// End after the first event.
    pub first_only: bool,
    /// Also send forgotten and removed records.
    pub changes: bool,
    /// Also send each message appended to the scope's sessions (W25).
    pub messages: bool,
}

/// The events of a watch, until the client goes away or the scope can no
/// longer be read.
pub type ActivityStream = Pin<Box<dyn Stream<Item = Result<ActivityEvent, ViewsError>> + Send>>;

fn event(
    record: Option<RecordDetail>,
    kind: RecordEventKind,
    record_id: Uuid,
    session_id: Option<Uuid>,
    at: DateTime<Utc>,
) -> ActivityEvent {
    ActivityEvent {
        record,
        kind: ActivityKind::Record(kind),
        record_id: Some(record_id),
        session_id,
        at,
        message: None,
    }
}

/// Every record id of `scope`.
fn ids(scope: &GraphScope) -> HashSet<Uuid> {
    scope
        .sessions
        .iter()
        .flat_map(|s| s.records().iter().map(|u| u.id))
        .collect()
}

/// Written events for the records of `scope` not in `known`, oldest first,
/// added to `known`; with `since`, only those written after it.
fn unseen(
    scope: &GraphScope,
    known: &mut HashSet<Uuid>,
    since: Option<DateTime<Utc>>,
) -> Vec<ActivityEvent> {
    let mut new: Vec<(DateTime<Utc>, Uuid, Uuid)> = scope
        .sessions
        .iter()
        .flat_map(|s| {
            let session = s.id();
            s.records()
                .iter()
                .map(move |u| (u.timestamp, u.id, session))
        })
        .filter(|(at, id, _)| !known.contains(id) && since.is_none_or(|s| *at > s))
        .collect();
    if new.is_empty() {
        return Vec::new();
    }
    new.sort();
    let index = Index::of(scope);
    new.into_iter()
        .filter_map(|(at, id, session)| {
            known.insert(id);
            let record = index.record_detail(id)?;
            Some(event(
                Some(record),
                RecordEventKind::Written,
                id,
                Some(session),
                at,
            ))
        })
        .collect()
}

/// What a watch reads the scope's changes from.
pub struct Sources {
    /// The runtime: sessions and their records.
    pub rt: Arc<MemoryRuntime>,
    /// The workspaces: who is in the scope.
    pub workspaces: Workspaces,
    /// The message streams.
    pub messages: Messages,
}

/// State of one running watch.
struct Watcher {
    sources: Sources,
    target: Target,
    request: WatchRequest,
    /// Records of the scope the client has, or need not be told about.
    known: HashSet<Uuid>,
}

impl Watcher {
    /// The sessions the watch admits now (`None`: all).
    fn members(&self) -> Result<Option<HashSet<Uuid>>, ViewsError> {
        match self.target {
            Target::Session(id) => Ok(Some(HashSet::from([id]))),
            Target::Workspace(id) => self
                .sources
                .workspaces
                .get(id)
                .map(|w| Some(w.get_all_sessions().into_iter().map(|(s, _)| s).collect()))
                .map_err(|e| ViewsError::WorkspaceNotFound(e.to_string())),
            Target::Global => Ok(None),
        }
    }

    /// What a batch of record changes tells the client, in order.
    async fn answer(&mut self, batch: Vec<RecordEvent>) -> Result<Vec<ActivityEvent>, ViewsError> {
        let members = self.members()?;
        let changes = self.request.changes;
        let mut wanted: Vec<(RecordEventKind, RecordEvent)> = Vec::new();
        for e in batch {
            if members.as_ref().is_some_and(|m| !m.contains(&e.session_id)) {
                continue;
            }
            let sent = match e.kind {
                RecordEventKind::Written => self.known.insert(e.entry_id).then_some(e.kind),
                // A record stored already retired is news, as a written
                // one, to a client that only takes written records.
                RecordEventKind::Forgotten => match (self.known.insert(e.entry_id), changes) {
                    (_, true) => Some(e.kind),
                    (true, false) => Some(RecordEventKind::Written),
                    (false, false) => None,
                },
                RecordEventKind::Removed => {
                    (self.known.remove(&e.entry_id) && changes).then_some(e.kind)
                }
            };
            if let Some(kind) = sent {
                wanted.push((kind, e));
            }
        }
        // One index per session over the records the changes name; the
        // session's own graph resolves their entity names.
        let mut records: HashMap<Uuid, RecordDetail> = HashMap::new();
        let sessions: HashSet<Uuid> = wanted
            .iter()
            .filter(|(kind, _)| *kind != RecordEventKind::Removed)
            .map(|(_, e)| e.session_id)
            .collect();
        for session_id in sessions {
            let session = match scope::load(&self.sources.rt, session_id).await {
                Ok(session) => session,
                // The session watched is gone: the watch ends.
                Err(e) if self.target == Target::Session(session_id) => return Err(e.into()),
                // A member deleted meanwhile has nothing to show.
                Err(_) => continue,
            };
            let scope = GraphScope {
                graph: Arc::clone(session.entity_graph_shared()),
                sessions: vec![session],
            };
            let index = Index::of(&scope);
            for (_, e) in wanted.iter().filter(|(_, e)| e.session_id == session_id) {
                if let Some(record) = index.record_detail(e.entry_id) {
                    records.insert(e.entry_id, record);
                }
            }
        }
        Ok(wanted
            .into_iter()
            .filter_map(|(kind, e)| {
                let record = records.remove(&e.entry_id);
                // A record removed again before it could be read is not news.
                if record.is_none() && kind != RecordEventKind::Removed {
                    return None;
                }
                Some(event(
                    record,
                    kind,
                    e.entry_id,
                    Some(e.session_id),
                    e.timestamp,
                ))
            })
            .collect())
    }

    /// The messages `changes` appended to the scope's sessions, in order.
    async fn appended(
        &self,
        changes: Vec<MessagesChange>,
    ) -> Result<Vec<ActivityEvent>, ViewsError> {
        let members = self.members()?;
        let mut events = Vec::new();
        for change in changes {
            if change.kind != MessagesChangeKind::Appended
                || members
                    .as_ref()
                    .is_some_and(|m| !m.contains(&change.session_id))
            {
                continue;
            }
            let (Some(first), Some(last)) = (change.seqs.iter().min(), change.seqs.iter().max())
            else {
                continue;
            };
            let count = usize::try_from(last - first + 1).unwrap_or(usize::MAX);
            let stored = self
                .sources
                .messages
                .page(
                    change.session_id,
                    MessageCursor::From(*first),
                    count,
                    MessageOrder::Asc,
                )
                .await?;
            events.extend(
                stored
                    .into_iter()
                    .filter(|m| change.seqs.contains(&m.seq))
                    .map(|m| ActivityEvent {
                        record: None,
                        kind: ActivityKind::Message,
                        record_id: None,
                        session_id: Some(m.session_id),
                        at: m.timestamp,
                        message: Some(turn(m, &Default::default())),
                    }),
            );
        }
        Ok(events)
    }

    /// After lost changes: the scope's records not sent yet, then (with
    /// `changes`) removals of those gone, whose session is not known here.
    async fn resync(&mut self) -> Result<Vec<ActivityEvent>, ViewsError> {
        let scope = scope::resolve(&self.sources.rt, self.target).await?.scope;
        let mut events = unseen(&scope, &mut self.known, None);
        let current = ids(&scope);
        let gone: Vec<Uuid> = self.known.difference(&current).copied().collect();
        let now = Utc::now();
        for id in gone {
            self.known.remove(&id);
            if self.request.changes {
                events.push(event(None, RecordEventKind::Removed, id, None, now));
            }
        }
        Ok(events)
    }
}

/// What the change log gave.
enum Batch {
    Changes(Vec<RecordEvent>, Vec<MessagesChange>),
    /// Changes were lost.
    Lagged,
    /// The memory instance is gone.
    Closed,
}

/// The next record changes of the log (and, when `messages`, message
/// changes), up to [`BATCH`] of those already there (other changes are
/// skipped).
async fn next_batch(changes: &mut broadcast::Receiver<Change>, messages: bool) -> Batch {
    let (mut records, mut appended) = (Vec::new(), Vec::new());
    loop {
        let change = if records.is_empty() && appended.is_empty() {
            match changes.recv().await {
                Ok(change) => change,
                Err(broadcast::error::RecvError::Closed) => return Batch::Closed,
                Err(broadcast::error::RecvError::Lagged(_)) => return Batch::Lagged,
            }
        } else {
            match changes.try_recv() {
                Ok(change) => change,
                Err(broadcast::error::TryRecvError::Lagged(_)) => return Batch::Lagged,
                Err(_) => return Batch::Changes(records, appended),
            }
        };
        match change {
            Change::Record(record) => records.push(record),
            Change::Messages(change) if messages => appended.push(change),
            _ => continue,
        }
        if records.len() + appended.len() == BATCH {
            return Batch::Changes(records, appended);
        }
    }
}

/// Watch `target` (see the module docs): `changes` was subscribed before
/// `scope` (the target resolved) was read, so nothing written in between is
/// missed; the records written after `since` are sent first.
pub fn watch(
    sources: Sources,
    target: Target,
    request: WatchRequest,
    since: DateTime<Utc>,
    scope: GraphScope,
    mut changes: broadcast::Receiver<Change>,
) -> ActivityStream {
    let (tx, rx) = mpsc::channel::<Result<ActivityEvent, ViewsError>>(BUFFERED);
    tokio::spawn(async move {
        let mut known = HashSet::new();
        let mut out = unseen(&scope, &mut known, Some(since));
        known.extend(ids(&scope));
        drop(scope);
        let mut watcher = Watcher {
            sources,
            target,
            request,
            known,
        };
        loop {
            for e in out.drain(..) {
                if tx.send(Ok(e)).await.is_err() || request.first_only {
                    return;
                }
            }
            let batch = tokio::select! {
                _ = tx.closed() => return,
                batch = next_batch(&mut changes, request.messages) => batch,
            };
            let answered = match batch {
                Batch::Changes(records, appended) => {
                    match (
                        watcher.answer(records).await,
                        watcher.appended(appended).await,
                    ) {
                        (Ok(mut records), Ok(messages)) => {
                            records.extend(messages);
                            Ok(records)
                        }
                        (Err(e), _) | (_, Err(e)) => Err(e),
                    }
                }
                Batch::Closed => return,
                Batch::Lagged => {
                    tracing::debug!("activity watcher lagged; re-reading its scope");
                    // Skip what is left of the lost stretch: the re-read
                    // covers it, and changes from here on are kept.
                    changes = changes.resubscribe();
                    watcher.resync().await
                }
            };
            match answered {
                Ok(events) => out = events,
                Err(e) => {
                    if tx.send(Err(e)).await.is_err() {
                        tracing::debug!("activity watcher closed before its error was sent");
                    }
                    return;
                }
            }
        }
    });
    Box::pin(futures::stream::unfold(rx, |mut rx| async move {
        rx.recv().await.map(|item| (item, rx))
    }))
}

#[cfg(test)]
mod tests;
