// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The work's ports over the host channel (W23), in the call's scope and
//! the team's workspace: the board's documents (its changes and tasks,
//! read where memory keeps them) and its `GetTask` and `AddComment` RPCs, and the library's
//! `GetSkill`.

use async_trait::async_trait;
use pcx_documents::{HostDocuments, Typed};
use pcx_module_api::grpc::GrpcChannel;
use pcx_module_api::{ModuleError, ModuleErrorKind, Scope};
use post_cortex_proto::board::ListBoardsRequest;
use post_cortex_proto::board::boards_client::BoardsClient;
use post_cortex_proto::board::{
    AddCommentRequest, Comment, GetTaskRequest, Labels as TaskLabels, Task, UpdateTaskRequest,
};
use post_cortex_proto::skills::GetSkillRequest;
use post_cortex_proto::skills::skills_client::SkillsClient;
use post_cortex_proto::tools_git::git_client::GitClient;
use post_cortex_proto::tools_git::{AddWorktreeRequest, Repo};
use tonic::Request;

use crate::going;
use crate::ports::{Board, Heading, Labels, Library, Workplace};
use crate::wake::{Change, Thread};

/// The kind of the board's tasks.
const TASK: &str = "board/task";

/// The kind of the board's comments.
const COMMENT: &str = "board/comment";

/// The board's tool that comments on a task.
const COMMENT_TOOL: &str = "comment";

/// The ports of one call.
pub struct HostWorld<'a> {
    channel: GrpcChannel,
    /// The call's scope, naming its workspace ([`Scope::working_in`]).
    scope: Scope,
    workspace: &'a str,
}

impl<'a> HostWorld<'a> {
    /// The ports over the module's host `channel`, in `scope`, in
    /// `workspace`.
    pub fn new(channel: GrpcChannel, scope: &Scope, workspace: &'a str) -> Self {
        Self {
            channel,
            scope: scope.working_in(workspace),
            workspace,
        }
    }

    fn docs(&self) -> HostDocuments<'_> {
        HostDocuments::new(self.channel.clone(), &self.scope, self.workspace)
    }

    /// `message` as a request in the call's scope.
    fn request<T>(&self, message: T) -> Result<Request<T>, ModuleError> {
        let mut request = Request::new(message);
        self.scope.to_metadata(request.metadata_mut())?;
        Ok(request)
    }
}

#[async_trait]
impl Board for HostWorld<'_> {
    async fn change(
        &self,
        kind: &str,
        id: &str,
        version: u64,
    ) -> Result<Option<Change>, ModuleError> {
        let docs = self.docs();
        match kind {
            TASK => {
                let tasks = Typed::new(&docs, TASK);
                let Some(now) = tasks.find_version::<Task>(id, version).await? else {
                    return Ok(None);
                };
                let was = match version.checked_sub(1).filter(|v| *v > 0) {
                    Some(before) => tasks
                        .find_version::<Task>(id, before)
                        .await?
                        .and_then(|t| t.data.assignee),
                    None => None,
                };
                Ok(Some(Change::Task {
                    task: id.to_string(),
                    author: now.document.author,
                    assignee: now.data.assignee,
                    was,
                }))
            }
            // Only a new comment speaks; an edit of one wakes no one.
            COMMENT if version == 1 => {
                let comments = Typed::new(&docs, COMMENT);
                let Some(comment) = comments.find_version::<Comment>(id, version).await? else {
                    return Ok(None);
                };
                let answers = match &comment.data.answers {
                    Some(answered) => comments
                        .find::<Comment>(answered)
                        .await?
                        .and_then(|c| c.document.author),
                    None => None,
                };
                Ok(Some(Change::Comment {
                    task: comment.data.task,
                    comment: id.to_string(),
                    author: comment.document.author,
                    text: comment.document.text,
                    answers,
                }))
            }
            _ => Ok(None),
        }
    }

    async fn heading(&self, task: &str) -> Result<Option<Heading>, ModuleError> {
        let docs = self.docs();
        Ok(Typed::new(&docs, TASK)
            .find::<Task>(task)
            .await?
            .map(|found| Heading {
                title: found.data.title,
                column: found.data.column,
            }))
    }

    async fn thread(&self, task: &str) -> Result<Thread, ModuleError> {
        let thread = BoardsClient::new(self.channel.clone())
            .get_task(self.request(GetTaskRequest {
                workspace_id: self.workspace.to_string(),
                task: task.to_string(),
            })?)
            .await?
            .into_inner();
        let record = thread.task.ok_or_else(|| {
            ModuleError::new(
                ModuleErrorKind::Internal,
                format!("the board answered no task {task}"),
            )
        })?;
        let data = record.task.unwrap_or_default();
        Ok(Thread {
            title: data.title,
            column: data.column,
            text: record.text,
            comments: thread
                .comments
                .into_iter()
                .map(|c| (c.author.unwrap_or_else(|| "someone".to_string()), c.text))
                .collect(),
            decisions: thread
                .decisions
                .into_iter()
                .map(|d| {
                    let decision = d.decision.unwrap_or_default();
                    (decision.decided_by, decision.what, d.pending)
                })
                .collect(),
        })
    }

    fn answer_tool(&self) -> &str {
        COMMENT_TOOL
    }

    async fn answer(
        &self,
        task: &str,
        text: &str,
        answers: Option<&str>,
    ) -> Result<(), ModuleError> {
        BoardsClient::new(self.channel.clone())
            .add_comment(self.request(AddCommentRequest {
                workspace_id: self.workspace.to_string(),
                task: task.to_string(),
                text: text.to_string(),
                answers: answers.map(str::to_string),
                // A member answers in text.
                attachments: Vec::new(),
            })?)
            .await?;
        Ok(())
    }
}

#[async_trait]
impl Labels for HostWorld<'_> {
    async fn labels(&self, task: &str) -> Result<(u64, Vec<String>), ModuleError> {
        let thread = BoardsClient::new(self.channel.clone())
            .get_task(self.request(GetTaskRequest {
                workspace_id: self.workspace.to_string(),
                task: task.to_string(),
            })?)
            .await?
            .into_inner();
        let record = thread.task.unwrap_or_default();
        let labels = record.task.map(|t| t.labels).unwrap_or_default();
        Ok((record.version, labels))
    }

    async fn closed(&self, task: &str) -> Result<bool, ModuleError> {
        let docs = self.docs();
        let Some(found) = Typed::new(&docs, TASK).find::<Task>(task).await? else {
            return Ok(true);
        };
        let listed = BoardsClient::new(self.channel.clone())
            .list_boards(self.request(ListBoardsRequest {
                workspace_id: self.workspace.to_string(),
                board: found.data.board,
            })?)
            .await?
            .into_inner();
        let Some(board) = listed.boards.into_iter().next().and_then(|b| b.board) else {
            return Ok(true);
        };
        Ok(going::column_closed(
            &board.columns,
            board.stages(),
            &found.data.column,
        ))
    }

    async fn relabel(
        &self,
        task: &str,
        version: u64,
        labels: Vec<String>,
    ) -> Result<(), ModuleError> {
        BoardsClient::new(self.channel.clone())
            .update_task(self.request(UpdateTaskRequest {
                workspace_id: self.workspace.to_string(),
                task: task.to_string(),
                version,
                labels: Some(TaskLabels { labels }),
                ..UpdateTaskRequest::default()
            })?)
            .await?;
        Ok(())
    }
}

#[async_trait]
impl Workplace for HostWorld<'_> {
    async fn repository(&self, task: &str) -> Result<Option<String>, ModuleError> {
        let mut boards = BoardsClient::new(self.channel.clone());
        let thread = boards
            .get_task(self.request(GetTaskRequest {
                workspace_id: self.workspace.to_string(),
                task: task.to_string(),
            })?)
            .await?
            .into_inner();
        let board = thread
            .task
            .and_then(|t| t.task)
            .map(|t| t.board)
            .unwrap_or_default();
        let listed = boards
            .list_boards(self.request(ListBoardsRequest {
                workspace_id: self.workspace.to_string(),
                board,
            })?)
            .await?
            .into_inner();
        Ok(listed
            .boards
            .into_iter()
            .next()
            .and_then(|b| b.board)
            .and_then(|b| b.repo))
    }

    async fn worktree(
        &self,
        repository: &str,
        path: &str,
        branch: &str,
    ) -> Result<String, ModuleError> {
        let made = GitClient::new(self.channel.clone())
            .add_worktree(self.request(AddWorktreeRequest {
                at: Some(Repo {
                    repo: repository.to_string(),
                    root: String::new(),
                }),
                path: path.to_string(),
                branch: branch.to_string(),
                from: String::new(),
            })?)
            .await?
            .into_inner();
        Ok(made.path)
    }

    async fn link(&self, task: &str, branch: &str) -> Result<(), ModuleError> {
        let (version, _) = self.labels(task).await?;
        BoardsClient::new(self.channel.clone())
            .update_task(self.request(UpdateTaskRequest {
                workspace_id: self.workspace.to_string(),
                task: task.to_string(),
                version,
                branch: Some(branch.to_string()),
                ..UpdateTaskRequest::default()
            })?)
            .await?;
        Ok(())
    }
}

#[async_trait]
impl Library for HostWorld<'_> {
    async fn skill(&self, name: &str) -> Result<Option<String>, ModuleError> {
        let found = SkillsClient::new(self.channel.clone())
            .get_skill(self.request(GetSkillRequest {
                name: name.to_string(),
            })?)
            .await;
        match found {
            Ok(skill) => Ok(Some(skill.into_inner().instructions)),
            Err(status) => match ModuleError::from(status) {
                e if e.kind == ModuleErrorKind::NotFound => Ok(None),
                e => Err(e),
            },
        }
    }
}
