// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The board service: the board's operations over its ports, the domain's
//! rules applied to what they read and write (ARCHITECTURE section 8).
//! Between the handlers and the ports.

use pcx_documents::{Documents, Item, Typed};
use pcx_module_api::{ModuleError, ModuleErrorKind, PersonRole};
use pcx_team::Act;
use post_cortex_proto::board::{AddTaskRequest, UpdateTaskRequest};
use prost::{Message, Name};

use crate::domain::{self, Attachment, Board, Broken, Comment, Decision, Kind, Task};
use crate::ports::{BoardEvent, Commits, Events, Records};
use crate::team::TeamRules;

/// A task with its discussion: what a task's detail shows.
#[derive(Debug, Clone, PartialEq)]
pub struct Thread {
    /// The task.
    pub task: Placed,
    /// Its comments, the oldest first.
    pub comments: Vec<Item<Comment>>,
    /// Its decisions, the oldest first.
    pub decisions: Vec<Item<Decision>>,
    /// The tasks related to it.
    pub related: Related,
    /// Why its branch's commits could not be read now; none: the task's
    /// links hold the branch's commits now, or it has no branch.
    pub commits_unread: Option<String>,
}

/// A task added, or the task of its key.
#[derive(Debug, Clone, PartialEq)]
pub struct Added {
    /// The task.
    pub task: Placed,
    /// Whether the board had it already (its key's).
    pub existing: bool,
}

/// A board's tasks and its plans.
#[derive(Debug, Clone, PartialEq)]
pub struct Listing {
    /// The tasks.
    pub tasks: Vec<Placed>,
    /// Its plans, in the tasks' order.
    pub plans: Vec<Placed>,
}

/// A decision or comment of task `task`, to find them by.
fn of_task<T: Default>(task: &str, with: impl FnOnce(&mut T, String)) -> T {
    let mut example = T::default();
    with(&mut example, task.to_string());
    example
}

/// The board's operations in one workspace, as `author` (the caller the
/// host names; none: an unnamed caller) whose person has `role` in the
/// organisation (none: not known).
pub struct Service<'a> {
    docs: &'a dyn Documents,
    records: &'a dyn Records,
    commits: &'a dyn Commits,
    events: &'a dyn Events,
    author: Option<&'a str>,
    role: Option<PersonRole>,
    module: Option<&'a str>,
}

impl<'a> Service<'a> {
    /// The board over its ports.
    pub fn new(
        docs: &'a dyn Documents,
        records: &'a dyn Records,
        commits: &'a dyn Commits,
        events: &'a dyn Events,
        author: Option<&'a str>,
        role: Option<PersonRole>,
    ) -> Self {
        Self {
            docs,
            records,
            commits,
            events,
            author,
            role,
            module: None,
        }
    }

    /// This board called from module `module`, as the host names it (a
    /// module asks decisions in its own name).
    #[must_use]
    pub fn called_from(mut self, module: Option<&'a str>) -> Self {
        self.module = module;
        self
    }

    /// A new task on `board`, at the end of its column, in its plan,
    /// waiting on the tasks named; with a key the board has a task of
    /// already, that task unchanged.
    ///
    /// # Errors
    /// No such board; a column it does not have; an empty title; a key too
    /// long; a plan or a dependency the board's rules refuse; an image
    /// memory does not have.
    pub async fn add_task(&self, task: AddTaskRequest) -> Result<Added, ModuleError> {
        let owner = self.board(&task.board).await?;
        let key = domain::key(task.key.as_deref()).map_err(broken)?;
        if let Some(key) = &key {
            let tasks = self.board_tasks(&owner.document.id).await?;
            if let Some(known) = tasks.iter().find(|t| &t.data.key == key) {
                let talk = self.talk(Some(&known.document.id)).await?;
                return Ok(Added {
                    task: placed(&owner.data, known.clone(), &tasks, &talk),
                    existing: true,
                });
            }
        }
        let title = task.title.trim();
        if title.is_empty() {
            return Err(broken(Broken::Empty("title")));
        }
        let column = domain::column(&owner.data, task.column.as_deref()).map_err(broken)?;
        if domain::closes(&owner.data, &column) {
            self.authorize(Act::Close).await?;
        }
        let tasks = self.board_tasks(&owner.document.id).await?;
        let board_tasks = by_id(&tasks);
        let parent = task
            .parent
            .and_then(named)
            .map(|p| on_board(&p, &board_tasks))
            .transpose()?;
        if let Some(parent) = &parent {
            domain::plan(None, parent, &board_tasks).map_err(broken)?;
        }
        let depends_on = all_on_board(&task.depends_on, &board_tasks)?;
        let depends_on = domain::dependencies(None, &depends_on, &board_tasks).map_err(broken)?;
        let rank = domain::place(&column, None, None, &board_tasks).map_err(broken)?;
        let data = Task {
            title: title.to_string(),
            board: owner.document.id,
            column,
            assignee: task.assignee,
            labels: task.labels,
            priority: task.priority,
            links: Some(domain::Links::default()),
            parent,
            depends_on,
            rank,
            attachments: task.attachments,
            key: key.unwrap_or_default(),
        };
        let added = self.create(Kind::Task, &data, &task.text).await?;
        self.events
            .emitted(BoardEvent::TaskCreated {
                task: added.document.id.clone(),
                board: added.data.board.clone(),
                title: added.data.title.clone(),
                column: added.data.column.clone(),
            })
            .await;
        let tasks = with(tasks, &added);
        // A new task has no comments or decisions yet.
        Ok(Added {
            task: placed(&owner.data, added, &tasks, &Talk::default()),
            existing: false,
        })
    }

    /// Change the task `edit` names from the version the caller read: a
    /// held task too (its text is how its decision is discussed). A branch
    /// links its commits.
    ///
    /// # Errors
    /// No such task; it changed since that version (`Conflict`); a branch
    /// on a board that names no repository; a plan or a dependency the
    /// board's rules refuse; an image memory does not have.
    pub async fn update_task(&self, edit: UpdateTaskRequest) -> Result<Placed, ModuleError> {
        let current = self.task(&edit.task).await?;
        let (id, version) = (current.document.id.clone(), edit.version);
        let id = id.as_str();
        let board = self.board(&current.data.board).await?;
        let tasks = self.board_tasks(&board.document.id).await?;
        let board_tasks = by_id(&tasks);
        let mut task = current.data;
        if let Some(title) = edit.title {
            if title.trim().is_empty() {
                return Err(broken(Broken::Empty("title")));
            }
            task.title = title.trim().to_string();
        }
        if let Some(assignee) = edit.assignee {
            task.assignee = named(assignee);
        }
        if let Some(labels) = edit.labels {
            task.labels = labels.labels;
        }
        if let Some(priority) = edit.priority {
            task.priority = Some(priority);
        }
        if let Some(parent) = edit.parent {
            task.parent = named(parent)
                .map(|p| on_board(&p, &board_tasks))
                .transpose()?;
            if let Some(parent) = &task.parent {
                domain::plan(Some(id), parent, &board_tasks).map_err(broken)?;
            }
        }
        if let Some(depends_on) = edit.depends_on {
            let depends_on = all_on_board(&depends_on.tasks, &board_tasks)?;
            task.depends_on =
                domain::dependencies(Some(id), &depends_on, &board_tasks).map_err(broken)?;
        }
        if let Some(attachments) = edit.attachments {
            task.attachments = attachments.attachments;
        }
        let mut links = task.links.take().unwrap_or_default();
        if let Some(pull_request) = edit.pull_request {
            links.pull_request = Some(pull_request);
        }
        if let Some(branch) = edit.branch {
            let repo = board.data.repo.as_deref().ok_or_else(|| {
                ModuleError::new(
                    ModuleErrorKind::FailedPrecondition,
                    format!(
                        "board {} names no repository for its branches",
                        board.data.name
                    ),
                )
            })?;
            links.commits = self.commits.branch_commits(repo, &branch).await?;
            links.branch = Some(branch);
        }
        task.links = Some(links);
        let text = edit.text.unwrap_or(current.document.text);
        let changed = self.change(Kind::Task, id, version, &task, &text).await?;
        let tasks = with(tasks, &changed);
        let talk = self.talk(Some(id)).await?;
        Ok(placed(&board.data, changed, &tasks, &talk))
    }

    /// The tasks of `board` in its columns' order, each column by rank
    /// (in `column` when one is named), and its plans in the same order.
    ///
    /// # Errors
    /// No such board; a column it does not have; memory failed.
    pub async fn tasks(&self, board: &str, column: Option<&str>) -> Result<Listing, ModuleError> {
        let owner = self.board(board).await?;
        let only = column
            .map(|c| domain::column(&owner.data, Some(c)))
            .transpose()
            .map_err(broken)?;
        let tasks = self.board_tasks(&owner.document.id).await?;
        let talk = self.talk(None).await?;
        let every: Vec<Placed> = in_order(&owner.data, &tasks)
            .into_iter()
            .map(|t| placed(&owner.data, t.clone(), &tasks, &talk))
            .collect();
        let plans = every
            .iter()
            .filter(|p| p.standing.progress.is_some())
            .cloned()
            .collect();
        let tasks = every
            .into_iter()
            .filter(|p| only.as_ref().is_none_or(|c| &p.task.data.column == c))
            .collect();
        Ok(Listing { tasks, plans })
    }

    /// Task `id` with its standing, comments, decisions, the tasks related
    /// to it, and its branch's commits read now (docs/modules/board.md, git):
    /// when they cannot be, the ones kept when it was linked, and why.
    ///
    /// # Errors
    /// No such task; memory failed.
    pub async fn thread(&self, id: &str) -> Result<Thread, ModuleError> {
        let mut task = self.task(id).await?;
        let id = task.document.id.clone();
        let id = id.as_str();
        let board = self.board(&task.data.board).await?;
        let branch = task.data.links.as_ref().and_then(|l| l.branch.clone());
        let commits_unread = match (board.data.repo.as_deref(), branch) {
            (Some(repo), Some(branch)) => match self.commits.branch_commits(repo, &branch).await {
                Ok(commits) => {
                    task.data.links.get_or_insert_default().commits = commits;
                    None
                }
                Err(e) => Some(e.message),
            },
            _ => None,
        };
        let tasks = self.board_tasks(&board.document.id).await?;
        let talk = self.talk(None).await?;
        let task = placed(&board.data, task, &tasks, &talk);
        let related = related(&board.data, &task, &tasks, &talk);
        // The workspace's, newest first: the task's, oldest first.
        let Talk {
            comments,
            decisions,
        } = talk;
        let mut comments: Vec<Item<Comment>> =
            comments.into_iter().filter(|c| c.data.task == id).collect();
        let mut decisions: Vec<Item<Decision>> = decisions
            .into_iter()
            .filter(|d| d.data.task == id)
            .collect();
        comments.reverse();
        decisions.reverse();
        Ok(Thread {
            task,
            comments,
            decisions,
            related,
            commits_unread,
        })
    }

    /// A comment on task `id` with its images, answering comment `answers`.
    ///
    /// # Errors
    /// No such task or comment; an empty text; an image memory does not
    /// have.
    pub async fn comment(
        &self,
        id: &str,
        text: &str,
        answers: Option<String>,
        attachments: Vec<Attachment>,
    ) -> Result<Item<Comment>, ModuleError> {
        let task = self.task(id).await?;
        let id = task.document.id.as_str();
        if let Some(answered) = &answers {
            self.get::<Comment>(Kind::Comment, answered).await?;
        }
        let comment = domain::comment(id, text, answers, attachments).map_err(broken)?;
        self.create(Kind::Comment, &comment, text.trim()).await
    }

    async fn authorize(&self, act: Act) -> Result<(), ModuleError> {
        TeamRules::new(self.docs)
            .authorize(self.author, self.role, self.module, act)
            .await
    }

    fn author(&self) -> Result<&'a str, ModuleError> {
        self.author.filter(|a| !a.trim().is_empty()).ok_or_else(|| {
            ModuleError::new(
                ModuleErrorKind::FailedPrecondition,
                "a decision names who makes it: the call names no author",
            )
        })
    }

    async fn board(&self, id: &str) -> Result<Item<Board>, ModuleError> {
        self.get(Kind::Board, id).await
    }

    /// Task `id`, or the task of the workspace whose id it begins (a short
    /// id, as the app shows it).
    ///
    /// # Errors
    /// No such task; a short id of more than one; memory failed.
    async fn task(&self, id: &str) -> Result<Item<Task>, ModuleError> {
        let typed = Typed::new(self.docs, Kind::Task.document());
        match typed.find(id).await {
            Ok(Some(task)) => return Ok(task),
            // Memory refuses a name that is not a whole id: a short id.
            Ok(None) => {}
            Err(e) if e.kind == ModuleErrorKind::InvalidArgument => {}
            Err(e) => return Err(e),
        }
        if id.chars().count() >= domain::SHORT_ID {
            let every: Vec<Item<Task>> = typed.all(None).await?;
            let named = domain::task_id(id, every.iter().map(|t| t.document.id.as_str()))
                .map_err(broken)?;
            if let Some(task) = named.and_then(|n| every.into_iter().find(|t| t.document.id == n)) {
                return Ok(task);
            }
        }
        Err(typed.not_found(id))
    }

    /// Every task of board `id`.
    async fn board_tasks(&self, id: &str) -> Result<Vec<Item<Task>>, ModuleError> {
        let of = Task {
            board: id.to_string(),
            ..Task::default()
        };
        self.all(Kind::Task, Some((&of, &["board"]))).await
    }

    /// The comments and decisions of task `id`, or of every task.
    async fn talk(&self, id: Option<&str>) -> Result<Talk, ModuleError> {
        match id {
            Some(id) => {
                let of: Comment = of_task(id, |c: &mut Comment, task| c.task = task);
                Ok(Talk {
                    comments: self.all(Kind::Comment, Some((&of, &["task"]))).await?,
                    decisions: self.decisions_of(id).await?,
                })
            }
            None => Ok(Talk {
                comments: self.all(Kind::Comment, None).await?,
                decisions: self.all(Kind::Decision, None).await?,
            }),
        }
    }

    /// The decisions of task `id`, the newest first.
    async fn decisions_of(&self, id: &str) -> Result<Vec<Item<Decision>>, ModuleError> {
        let of: Decision = of_task(id, |d: &mut Decision, task| d.task = task);
        self.all(Kind::Decision, Some((&of, &["task"]))).await
    }

    async fn get<T: Message + Name + Default>(
        &self,
        kind: Kind,
        id: &str,
    ) -> Result<Item<T>, ModuleError> {
        Typed::new(self.docs, kind.document()).get(id).await
    }

    async fn all<T: Message + Name + Default>(
        &self,
        kind: Kind,
        matching: Option<(&T, &[&str])>,
    ) -> Result<Vec<Item<T>>, ModuleError> {
        Typed::new(self.docs, kind.document()).all(matching).await
    }

    async fn create<T: Message + Name + Default>(
        &self,
        kind: Kind,
        value: &T,
        text: &str,
    ) -> Result<Item<T>, ModuleError> {
        Typed::new(self.docs, kind.document())
            .create(value, text)
            .await
    }

    async fn change<T: Message + Name + Default>(
        &self,
        kind: Kind,
        id: &str,
        version: u64,
        value: &T,
        text: &str,
    ) -> Result<Item<T>, ModuleError> {
        Typed::new(self.docs, kind.document())
            .change(id, version, value, text)
            .await
    }
}

mod boards;
mod decisions;
mod moves;
mod placed;
mod rules;

pub use decisions::Entry;
pub use placed::{Placed, Related};

use placed::{Talk, by_id, in_order, named, placed, related, with};
use rules::{all_on_board, broken, on_board};

#[cfg(test)]
mod tests;
