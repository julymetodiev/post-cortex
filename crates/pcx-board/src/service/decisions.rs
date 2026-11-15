// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The board's decisions (ARCHITECTURE section 8): a decision on a task,
//! recorded in the board's session; one with a risk holds its task until
//! the team's policy or a person approves it; the workspace's decisions as
//! a person approves them (W24, Approvals).

use pcx_documents::Item;
use pcx_module_api::ModuleError;
use pcx_team::{Act, Stakes};

use crate::domain::{self, Board, Decision, DecisionRules, Kind, Risk, Task};
use crate::ports::{BoardEvent, DecisionRecord};
use crate::service::{Service, broken};
use crate::team::TeamRules;

/// A decision with what it holds, as the approvals show it.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    /// The decision.
    pub decision: Item<Decision>,
    /// Its task's title.
    pub task_title: String,
    /// Its task's board, by id.
    pub board: String,
    /// That board's name.
    pub board_name: String,
}

impl Service<'_> {
    /// The caller's decision on task `id`, recorded in the board's
    /// session; one with a risk holds the task until approved, and keeps
    /// why the team's policy did not approve it.
    ///
    /// # Errors
    /// The call names no author; no such task; an empty what or why.
    pub async fn decide(
        &self,
        id: &str,
        what: &str,
        why: &str,
        risk: Risk,
        stakes: Stakes,
    ) -> Result<Item<Decision>, ModuleError> {
        let author = self.author()?;
        self.authorize(Act::Decide).await?;
        let task = self.task(id).await?;
        let board = self.board(&task.data.board).await?;
        let decision =
            domain::decide(&task.document.id, what, why, author, risk, stakes).map_err(broken)?;
        let text = format!("{}\n\n{}", decision.what, decision.why);
        let mut written = self.create(Kind::Decision, &decision, &text).await?;
        // Inside the trust a person gave the author's team: approved at
        // once; else it keeps why it waits for a person.
        if written.data.pending() {
            let weighed = match TeamRules::new(self.docs).weigh(author, &written).await? {
                Ok(approver) => {
                    domain::approve(written.data.clone(), &approver, None).map_err(broken)?
                }
                Err(hold) => domain::held(written.data.clone(), &hold),
            };
            let (id, version) = (written.document.id.clone(), written.document.version);
            written = self
                .change(Kind::Decision, &id, version, &weighed, &text)
                .await?;
        }
        self.records
            .record_decision(
                &board.data.session_id,
                &DecisionRecord {
                    what: decision.what,
                    why: decision.why,
                    task: task.data.title,
                    board: board.data.name,
                    decided_by: author.to_string(),
                },
            )
            .await?;
        self.events
            .emitted(BoardEvent::DecisionMade {
                decision: written.document.id.clone(),
                task: task.document.id.clone(),
                pending: written.data.pending(),
            })
            .await;
        if let Some(approved_by) = &written.data.approved_by {
            self.events
                .emitted(BoardEvent::DecisionApproved {
                    decision: written.document.id.clone(),
                    task: task.document.id,
                    approved_by: approved_by.clone(),
                })
                .await;
        }
        Ok(written)
    }

    /// Revoke the approval of decision `id` from `version`, as the caller
    /// (a person: the host refuses this to agents): it waits again, and its
    /// task is held.
    ///
    /// # Errors
    /// The call names no author; no such decision; it is not approved; it
    /// changed since `version` (`Conflict`).
    pub async fn revoke(&self, id: &str, version: u64) -> Result<Item<Decision>, ModuleError> {
        self.author()?;
        let current: Item<Decision> = self.get(Kind::Decision, id).await?;
        let revoked = domain::revoke(current.data).map_err(broken)?;
        self.change(
            Kind::Decision,
            id,
            version,
            &revoked,
            &current.document.text,
        )
        .await
    }

    /// Withdraw decision `id`, waiting, as the caller, its author, because
    /// of `why`: it stays withdrawn in its task's thread and holds the
    /// task no more.
    ///
    /// # Errors
    /// The call names no author; no such decision; it waits for no
    /// approval; the caller is not its author (`Denied`); an empty why; it
    /// changed meanwhile (`Conflict`).
    pub async fn withdraw(&self, id: &str, why: &str) -> Result<Item<Decision>, ModuleError> {
        let author = self.author()?;
        let current: Item<Decision> = self.get(Kind::Decision, id).await?;
        let withdrawn = domain::withdraw(current.data, author, why).map_err(broken)?;
        self.change(
            Kind::Decision,
            id,
            current.document.version,
            &withdrawn,
            &current.document.text,
        )
        .await
    }

    /// Decline decision `id` from `version`, as the caller (a person: the
    /// host refuses this to agents), with `note` for the record: it stays
    /// declined in its task's thread, holds the task no more, and who
    /// asked it hears that it was declined.
    ///
    /// # Errors
    /// The call names no author; no such decision; it waits for no
    /// approval; it changed since `version` (`Conflict`).
    pub async fn decline(
        &self,
        id: &str,
        version: u64,
        note: Option<String>,
    ) -> Result<Item<Decision>, ModuleError> {
        let author = self.author()?;
        let current: Item<Decision> = self.get(Kind::Decision, id).await?;
        let declined = domain::decline(current.data, author, note).map_err(broken)?;
        let written = self
            .change(
                Kind::Decision,
                id,
                version,
                &declined,
                &current.document.text,
            )
            .await?;
        self.events
            .emitted(BoardEvent::DecisionDeclined {
                decision: written.document.id.clone(),
                task: written.data.task.clone(),
                declined_by: author.to_string(),
            })
            .await;
        Ok(written)
    }

    /// Approve decision `id` from `version`, as the caller (a person: the
    /// host refuses this to agents), with `note` for the record: a note is
    /// recorded in the board's session too.
    ///
    /// # Errors
    /// The call names no author; no such decision; it waits for no
    /// approval; it changed since `version` (`Conflict`).
    pub async fn approve(
        &self,
        id: &str,
        version: u64,
        note: Option<String>,
    ) -> Result<Item<Decision>, ModuleError> {
        let author = self.author()?;
        let current: Item<Decision> = self.get(Kind::Decision, id).await?;
        let approved = domain::approve(current.data, author, note).map_err(broken)?;
        let written = self
            .change(
                Kind::Decision,
                id,
                version,
                &approved,
                &current.document.text,
            )
            .await?;
        if let Some(note) = &written.data.approval_note {
            let task = self.task(&written.data.task).await?;
            let board = self.board(&task.data.board).await?;
            self.records
                .record_decision(
                    &board.data.session_id,
                    &DecisionRecord {
                        what: format!("Approved: {}", written.data.what),
                        why: note.clone(),
                        task: task.data.title,
                        board: board.data.name,
                        decided_by: author.to_string(),
                    },
                )
                .await?;
        }
        self.events
            .emitted(BoardEvent::DecisionApproved {
                decision: written.document.id.clone(),
                task: written.data.task.clone(),
                approved_by: author.to_string(),
            })
            .await;
        Ok(written)
    }

    /// The workspace's decisions of every board that wait for a person or
    /// were approved (by a person or the policy), none withdrawn or declined, the newest first, each
    /// with its task and board; only the waiting ones (`Some(true)`) or the
    /// approved ones (`Some(false)`) when `waiting` says so.
    ///
    /// # Errors
    /// A decision's task or board is gone; memory failed.
    pub async fn decisions(&self, waiting: Option<bool>) -> Result<Vec<Entry>, ModuleError> {
        let decisions: Vec<Item<Decision>> = self
            .all(Kind::Decision, None)
            .await?
            .into_iter()
            .filter(|d: &Item<Decision>| {
                d.data.risk() != Risk::None
                    && d.data.withdrawn.is_none()
                    && d.data.declined_by.is_none()
            })
            .filter(|d| waiting.is_none_or(|waiting| d.data.pending() == waiting))
            .collect();
        // The tasks and boards are read only for decisions to show.
        if decisions.is_empty() {
            return Ok(Vec::new());
        }
        let tasks: Vec<Item<Task>> = self.all(Kind::Task, None).await?;
        let boards: Vec<Item<Board>> = self.all(Kind::Board, None).await?;
        let find = |id: &str, kind: Kind| {
            ModuleError::new(
                pcx_module_api::ModuleErrorKind::NotFound,
                format!("no {} {id}", kind.name()),
            )
        };
        decisions
            .into_iter()
            .map(|decision| {
                let task = tasks
                    .iter()
                    .find(|t| t.document.id == decision.data.task)
                    .ok_or_else(|| find(&decision.data.task, Kind::Task))?;
                let board = boards
                    .iter()
                    .find(|b| b.document.id == task.data.board)
                    .ok_or_else(|| find(&task.data.board, Kind::Board))?;
                Ok(Entry {
                    task_title: task.data.title.clone(),
                    board: board.document.id.clone(),
                    board_name: board.data.name.clone(),
                    decision,
                })
            })
            .collect()
    }
}
