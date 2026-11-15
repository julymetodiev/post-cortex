// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! A member's work (ARCHITECTURE section 8): who a change of the board
//! wakes, and one turn of a woken member: its session for the task (kept
//! in `team/work`, which counts its turns), what it is told (its role, skills and the task's
//! thread), the agent's turn in its name (without the board's comment
//! tool), and its answer posted as its comment (a turn without one says
//! how it ended and its last steps instead). And a person's talk with a
//! member (W25): a turn of the conversation's session with the member's
//! role, skills and tools, written as the member.

use pcx_documents::{Documents, Item, Sessions, Typed};
use pcx_module_api::agent::{TurnEnd, TurnEvent, TurnRequest, Turns};
use pcx_module_api::{ModuleError, ModuleErrorKind, Scope};
use tokio::sync::mpsc;

use crate::domain::{Agent, Kind, Work};
use crate::going;
use crate::place;
use crate::ports::{Board, Library, Workplace};
use crate::progress::Progress;
use crate::service::Service;
use crate::steps::{self, Steps};
use crate::wake::{self, Wake};

/// The turn events a member's turn may send before they are read.
const EVENTS: usize = 64;

/// A member's work over its ports, in one call's scope (the member's,
/// as the author of what the turn writes).
pub struct Worker<'a> {
    /// The team's documents.
    pub docs: &'a dyn Documents,
    /// The board.
    pub board: &'a dyn Board,
    /// The skills.
    pub library: &'a dyn Library,
    /// New sessions.
    pub sessions: &'a dyn Sessions,
    /// The agent's turns.
    pub turns: &'a dyn Turns,
    /// Where a member works on a task (W32).
    pub workplace: &'a dyn Workplace,
    /// The call's scope.
    pub scope: &'a Scope,
}

/// `scope` as a turn of `member` carries it: reaching no more than the
/// member does (the workspaces both name; docs/connect.md section 4a, a
/// member's reach).
///
/// # Errors
/// `FailedPrecondition` when the turn's workspace is none of them.
fn held(scope: &Scope, member: &Agent) -> Result<Scope, ModuleError> {
    if member.reach.is_empty() {
        return Ok(scope.clone());
    }
    let held = scope.held_to(&member.reach);
    match &held.workspace_id {
        Some(workspace) if !held.reaches(workspace) => Err(ModuleError::new(
            ModuleErrorKind::FailedPrecondition,
            format!(
                "a turn of {} runs in workspace {workspace}, which it does not reach",
                member.name
            ),
        )),
        _ => Ok(held),
    }
}

impl Worker<'_> {
    /// The members the change version `version` of the board's document
    /// `id` (of `kind`) wakes.
    ///
    /// # Errors
    /// Memory failed.
    pub async fn woken(
        &self,
        kind: &str,
        id: &str,
        version: u64,
    ) -> Result<Vec<Wake>, ModuleError> {
        let Some(change) = self.board.change(kind, id, version).await? else {
            return Ok(Vec::new());
        };
        let members = self.members().await?;
        Ok(wake::woken(&change, &members))
    }

    /// One turn of `wake`'s member on its task; its answer posted as its
    /// comment (without one: how it ended and its last steps), and how the
    /// turn ended answered. The scope's author must be the member.
    ///
    /// # Errors
    /// No such member; memory, the board or the agent failed.
    pub async fn work(&self, wake: &Wake, progress: &Progress) -> Result<TurnEnd, ModuleError> {
        let member = self.member(&wake.member).await?;
        let scope = held(self.scope, &member)?;
        let workspace = self.scope.workspace_id.as_deref().ok_or_else(|| {
            ModuleError::new(
                ModuleErrorKind::FailedPrecondition,
                "a member's turn runs in its task's workspace",
            )
        })?;
        let thread = self.board.thread(&wake.task).await?;
        let work = self
            .session(&member.name, &wake.task, &thread.title)
            .await?;
        let work = self.placed(work, &wake.task, &thread.title).await?;
        let work = going::mark(self.docs, work, wake).await?;
        let mut prompt = wake::prompt(wake.why, &wake.task, workspace, &thread.text());
        if !work.data.worktree.is_empty() {
            prompt = format!(
                "{}\n\n{prompt}",
                place::told(&work.data.worktree, &work.data.branch)
            );
        }
        let request = TurnRequest {
            // Its answer is its comment: the turn does not comment itself.
            withheld: vec![self.board.answer_tool().to_string()],
            ..self
                .as_member(&member, work.data.session_id.clone(), prompt)
                .await?
        };
        // The turn's steps are read as they come, for a turn that gives no
        // answer to say what it did; the sender goes with the turn, which
        // ends the reading.
        let (tx, mut events) = mpsc::channel(EVENTS);
        let turn = async move { self.turns.run(&scope, &request, &tx).await };
        let seen = async {
            let mut seen = Steps::default();
            while let Some(event) = events.recv().await {
                seen.see(&event);
                progress.see(&event);
            }
            seen
        };
        let (end, seen) = tokio::join!(turn, seen);
        let end = end?;
        tracing::info!(member = %member.name, task = %wake.task, steps = end.steps, stop = end.stop.name(), "a member's turn ended");
        let session = work.data.session_id.clone();
        self.took(work).await?;
        let said = if steps::unanswered(&end) {
            steps::said(&member.name, &end, &seen.last(), &session)
        } else {
            end.answer.trim().to_string()
        };
        if !said.is_empty() {
            self.board
                .answer(&wake.task, &said, wake.answers.as_deref())
                .await?;
        }
        Ok(end)
    }

    /// A person's turn with member `name` (W25) in `session`, asking
    /// `prompt`, its events sent to `events`: the member's role, skills and
    /// tools apply as on the board, and its turns are written as it. The
    /// scope's author is the person who asks, who writes the prompt; the
    /// turn's calls run as the member, with no person's role.
    ///
    /// # Errors
    /// No such member, or one that is not an agent of the harness; memory
    /// or the agent failed.
    pub async fn talk(
        &self,
        name: &str,
        session: &str,
        prompt: &str,
        events: &mpsc::Sender<TurnEvent>,
    ) -> Result<TurnEnd, ModuleError> {
        let member = self.member(name).await?;
        if !member.harness {
            return Err(ModuleError::new(
                ModuleErrorKind::FailedPrecondition,
                format!("{name} is not an agent of the harness: it takes part through the board"),
            ));
        }
        let workspace = self.scope.workspace_id.as_deref().ok_or_else(|| {
            ModuleError::new(
                ModuleErrorKind::FailedPrecondition,
                "a member's talk runs in a workspace",
            )
        })?;
        // The person asks; the turn's calls are the member's, under its own
        // authority (no person's role), so what it writes names it.
        let asked = self.as_member(&member, session, prompt).await?;
        let request = TurnRequest {
            asker: self.scope.author.clone(),
            instructions: format!("{}\n\n{}", asked.instructions, wake::talking_in(workspace)),
            ..asked
        };
        let scope = Scope {
            author: Some(member.name.clone()),
            role: None,
            ..held(self.scope, &member)?
        };
        self.turns.run(&scope, &request, events).await
    }

    /// Member `name` of the workspace's teams.
    async fn member(&self, name: &str) -> Result<Agent, ModuleError> {
        self.members()
            .await?
            .into_iter()
            .find(|m| m.name == name)
            .ok_or_else(|| {
                ModuleError::new(
                    ModuleErrorKind::NotFound,
                    format!("no member {name} in the workspace's teams"),
                )
            })
    }

    /// A turn of `member` in `session` for `prompt`: its role and skills as
    /// instructions, its model, modules and tools, written as it.
    async fn as_member(
        &self,
        member: &Agent,
        session: impl Into<String>,
        prompt: impl Into<String>,
    ) -> Result<TurnRequest, ModuleError> {
        let mut skills = Vec::with_capacity(member.skills.len());
        for name in &member.skills {
            match self.library.skill(name).await? {
                Some(text) => skills.push((name.clone(), text)),
                None => {
                    tracing::warn!(member = %member.name, skill = %name, "the library has no such skill")
                }
            }
        }
        Ok(TurnRequest {
            instructions: wake::instructions(member, &skills),
            model: member.model.clone(),
            modules: member.modules.clone(),
            tools: member.tools.clone(),
            speaker: Some(member.name.clone()),
            ..TurnRequest::new(session, prompt)
        })
    }

    /// The harness and other members of the workspace's teams.
    async fn members(&self) -> Result<Vec<Agent>, ModuleError> {
        let rosters = Service::new(self.docs).rosters(None).await?;
        let mut members: Vec<Agent> = Vec::new();
        for agent in rosters
            .into_iter()
            .flat_map(|r| r.agents)
            .filter_map(|record| record.agent)
        {
            if !members.iter().any(|m| m.name == agent.name) {
                members.push(agent);
            }
        }
        Ok(members)
    }

    /// `work` with its worktree: the one kept, else, on a board with a
    /// repository, a new one on the task's branch, which the task links
    /// (kept); none on a board without one.
    async fn placed(
        &self,
        work: Item<Work>,
        task: &str,
        title: &str,
    ) -> Result<Item<Work>, ModuleError> {
        if !work.data.worktree.is_empty() {
            return Ok(work);
        }
        let Some(repository) = self.workplace.repository(task).await? else {
            return Ok(work);
        };
        let (path, branch) = place::place(task, title);
        let worktree = self.workplace.worktree(&repository, &path, &branch).await?;
        self.workplace.link(task, &branch).await?;
        let placed = Work {
            worktree,
            branch,
            ..work.data
        };
        Typed::new(self.docs, Kind::Work.document())
            .change(
                &work.document.id,
                work.document.version,
                &placed,
                &placed.agent,
            )
            .await
    }

    /// Count a turn of `work`.
    async fn took(&self, work: Item<Work>) -> Result<(), ModuleError> {
        let counted = Work {
            turns: work.data.turns.saturating_add(1),
            ..work.data
        };
        Typed::new(self.docs, Kind::Work.document())
            .change(
                &work.document.id,
                work.document.version,
                &counted,
                &counted.agent,
            )
            .await?;
        Ok(())
    }

    /// The work of `member` on `task`, its session the one kept, else a
    /// new one, kept.
    async fn session(
        &self,
        member: &str,
        task: &str,
        title: &str,
    ) -> Result<Item<Work>, ModuleError> {
        let works = Typed::new(self.docs, Kind::Work.document());
        let of = Work {
            agent: member.to_string(),
            task: task.to_string(),
            ..Work::default()
        };
        let found: Vec<Item<Work>> = works.all(Some((&of, &["agent", "task"]))).await?;
        if let Some(work) = found.into_iter().next() {
            return Ok(work);
        }
        let session_id = self
            .sessions
            .create(&format!("{member} on {title}"))
            .await?;
        let work = Work {
            agent: member.to_string(),
            task: task.to_string(),
            session_id,
            turns: 0,
            worktree: String::new(),
            branch: String::new(),
            going: None,
        };
        works.create(&work, &work.agent).await
    }
}

#[cfg(test)]
mod tests;
