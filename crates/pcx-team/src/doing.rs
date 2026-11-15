// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! What the members are doing (W25): each member's work on a task as
//! `team/work` keeps it, with the task's title and column from the board
//! and whether a turn of it runs now.

use futures::{StreamExt, TryStreamExt, stream};

use crate::progress::Busy;
use pcx_documents::{Documents, Item, Typed};
use pcx_module_api::ModuleError;
use post_cortex_proto::team::WorkEntry;

use crate::domain::{Kind, Work};
use crate::ports::Board;
use crate::service::Service;

/// How many headings are read at once.
const AT_ONCE: usize = 8;

/// Entries a list gives when it names no limit.
const LISTED: usize = 50;

/// Entries a list gives at most.
const MAX_LISTED: usize = 500;

/// Which of the members' work a list holds.
#[derive(Default)]
pub struct WorkFilter<'a> {
    /// Only this team's members; none: every member.
    pub team: Option<&'a str>,
    /// Only the work on this task; none: on every task.
    pub task: Option<&'a str>,
    /// Only the work a turn of which runs now.
    pub busy_only: bool,
    /// At most this many, the most recent first; 0: 50.
    pub limit: u32,
}

/// The members' work over its ports.
pub struct Doing<'a> {
    /// The team's documents.
    pub docs: &'a dyn Documents,
    /// The board.
    pub board: &'a dyn Board,
}

impl Doing<'_> {
    /// The work `filter` names, the most recent first, at most its limit;
    /// `busy` holds the turns that run now. Only what the filter names is
    /// read, and headings only for what stays: the work grows with every
    /// task a member touched.
    ///
    /// # Errors
    /// Memory failed.
    pub async fn list(
        &self,
        filter: &WorkFilter<'_>,
        busy: &[Busy],
    ) -> Result<Vec<WorkEntry>, ModuleError> {
        let limit = match filter.limit {
            0 => LISTED,
            n => usize::try_from(n).map_or(MAX_LISTED, |n| n.min(MAX_LISTED)),
        };
        let members: Option<Vec<String>> = match filter.team {
            Some(name) => Some(
                Service::new(self.docs)
                    .rosters(Some(name))
                    .await?
                    .into_iter()
                    .flat_map(|r| r.agents)
                    .filter_map(|record| record.agent.map(|a| a.name))
                    .collect(),
            ),
            None => None,
        };
        let keep = |item: &Item<Work>| {
            let work = &item.data;
            members.as_ref().is_none_or(|m| m.contains(&work.agent))
                && filter.task.is_none_or(|task| work.task == task)
                && (!filter.busy_only
                    || busy
                        .iter()
                        .any(|b| b.task == work.task && b.member == work.agent))
        };
        // Each read names what it needs (docs/host.md, pages in a module):
        // the turns that run, the task, the team's members; else the newest.
        let named = |agent: &str, task: &str| Work {
            agent: agent.to_string(),
            task: task.to_string(),
            ..Work::default()
        };
        let examples: Option<Vec<Work>> = if filter.busy_only {
            Some(busy.iter().map(|b| named(&b.member, &b.task)).collect())
        } else if let Some(task) = filter.task {
            Some(vec![named("", task)])
        } else {
            members
                .as_ref()
                .map(|m| m.iter().map(|agent| named(agent, "")).collect())
        };
        let works = Typed::new(self.docs, Kind::Work.document());
        let found: Vec<Item<Work>> = match examples {
            None => works.after(None, None, limit, keep).await?.0,
            Some(examples) => {
                let mut found = Vec::new();
                for example in &examples {
                    let fields: Vec<&str> = [("agent", &example.agent), ("task", &example.task)]
                        .into_iter()
                        .filter(|(_, value)| !value.is_empty())
                        .map(|(field, _)| field)
                        .collect();
                    let matching = Some((example, fields.as_slice()));
                    found.extend(works.after(matching, None, limit, keep).await?.0);
                }
                let at = |w: &Item<Work>| w.document.updated_at.map(|t| (t.seconds, t.nanos));
                found.sort_by_key(|w| std::cmp::Reverse(at(w)));
                found.truncate(limit);
                found
            }
        };
        // The headings at once, not one after another: a board polls the
        // list.
        let board = self.board;
        let tasks: Vec<String> = found.iter().map(|work| work.data.task.clone()).collect();
        let headings = stream::iter(tasks)
            .map(|task| async move { board.heading(&task).await })
            .buffered(AT_ONCE)
            .try_collect::<Vec<_>>()
            .await?;
        let mut entries = Vec::with_capacity(found.len());
        for (work, heading) in found.into_iter().zip(headings) {
            let now = busy
                .iter()
                .find(|b| b.task == work.data.task && b.member == work.data.agent);
            let (title, column) = heading.map_or_else(Default::default, |h| (h.title, h.column));
            entries.push(WorkEntry {
                agent: work.data.agent,
                task: work.data.task,
                title,
                column,
                session_id: work.data.session_id,
                turns: work.data.turns,
                updated_at: work.document.updated_at,
                busy: now.is_some(),
                quiet_seconds: now.map_or(0, |b| b.quiet_seconds),
                received: now.map_or(0, |b| b.received),
            });
        }
        Ok(entries)
    }
}

#[cfg(test)]
mod tests;
