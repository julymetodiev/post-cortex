// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The board's `pcx.board.v1.Boards` service (W23): each RPC's message and
//! the call's scope in (its workspace and author), the board service's
//! call, its message out. No logic of their own.

use pcx_documents::{HostDocuments, Item};
use pcx_module_api::Scope;
use post_cortex_proto::board::boards_server::Boards;
use post_cortex_proto::board::{
    AddCommentRequest, AddTaskRequest, BoardList, BoardRecord, CommentRecord, CreateBoardRequest,
    DecideRequest, DecisionEntry, DecisionList, DecisionRecord, DecisionRequest, GetTaskRequest,
    ListBoardsRequest, ListDecisionsRequest, ListTasksRequest, MoveTaskRequest, Plan, TaskList,
    TaskRecord, Thread, UpdateBoardRequest, UpdateTaskRequest, WithdrawDecisionRequest,
};
use tonic::{Request, Response, Status};

use crate::adapters::HostPorts;
use crate::domain::{Board, Comment, Decision, DecisionRules};
use crate::module::{self, Shared};
use crate::service::{self, Entry, Placed, Service};

/// The board's RPCs over the running module.
pub struct Handlers {
    /// The running module.
    pub shared: Shared,
}

/// What a call's ports are made of: the running module, the scope and the
/// workspace it works in.
struct Call<'a> {
    running: module::Running,
    scope: &'a Scope,
    workspace: String,
}

impl Call<'_> {
    /// The call's documents and ports.
    fn ports(&self) -> (HostDocuments<'_>, HostPorts<'_>) {
        (
            HostDocuments::new(self.running.channel.clone(), self.scope, &self.workspace),
            HostPorts::new(&self.running, self.scope, &self.workspace),
        )
    }
}

impl Handlers {
    /// A call in `scope`, in its workspace (`named` when its scope names
    /// none).
    fn call<'a>(&self, scope: &'a Scope, named: &str) -> Result<Call<'a>, Status> {
        let running = module::running(&self.shared)?;
        Ok(Call {
            running,
            scope,
            workspace: scope.workspace(named)?,
        })
    }
}

fn board(item: Item<Board>) -> BoardRecord {
    BoardRecord {
        id: item.document.id,
        version: item.document.version,
        board: Some(item.data),
        updated_at: item.document.updated_at,
    }
}

fn task(placed: Placed) -> TaskRecord {
    let (item, standing) = (placed.task, placed.standing);
    TaskRecord {
        id: item.document.id,
        version: item.document.version,
        task: Some(item.data),
        text: item.document.text,
        updated_at: item.document.updated_at,
        author: item.document.author,
        waited_on_by: standing.waited_on_by,
        ready: standing.ready,
        progress: standing.progress,
        comments: placed.comments,
        held_by: placed.held_by,
        existing: false,
    }
}

fn comment(item: Item<Comment>) -> CommentRecord {
    CommentRecord {
        id: item.document.id,
        comment: Some(item.data),
        text: item.document.text,
        updated_at: item.document.updated_at,
        author: item.document.author,
    }
}

fn decision(item: Item<Decision>) -> DecisionRecord {
    DecisionRecord {
        id: item.document.id,
        version: item.document.version,
        pending: item.data.pending(),
        decision: Some(item.data),
        updated_at: item.document.updated_at,
    }
}

fn entry(entry: Entry) -> DecisionEntry {
    DecisionEntry {
        decision: Some(decision(entry.decision)),
        task_title: entry.task_title,
        board: entry.board,
        board_name: entry.board_name,
    }
}

fn plan(placed: Placed) -> Plan {
    let progress = placed.standing.progress.unwrap_or_default();
    Plan {
        id: placed.task.document.id,
        title: placed.task.data.title,
        done: progress.done,
        total: progress.total,
        assignee: placed.task.data.assignee,
    }
}

fn thread(thread: service::Thread) -> Thread {
    let related = thread.related;
    let tasks = |placed: Vec<Placed>| placed.into_iter().map(task).collect();
    Thread {
        task: Some(task(thread.task)),
        comments: thread.comments.into_iter().map(comment).collect(),
        decisions: thread.decisions.into_iter().map(decision).collect(),
        plan: related.plan.map(task),
        plan_tasks: tasks(related.plan_tasks),
        waits_on: tasks(related.waits_on),
        waited_on_by: tasks(related.waited_on_by),
        commits_unread: thread.commits_unread.unwrap_or_default(),
    }
}

#[tonic::async_trait]
impl Boards for Handlers {
    async fn create_board(
        &self,
        request: Request<CreateBoardRequest>,
    ) -> Result<Response<BoardRecord>, Status> {
        let scope = Scope::of(&request)?;
        let request = request.into_inner();
        let call = self.call(&scope, &request.workspace_id)?;
        let (docs, ports) = call.ports();
        let created = Service::new(
            &docs,
            &ports,
            &ports,
            &ports,
            scope.author.as_deref(),
            scope.role,
        )
        .create_board(&request.name, request.columns, request.stages, request.repo)
        .await?;
        Ok(Response::new(board(created)))
    }

    async fn update_board(
        &self,
        request: Request<UpdateBoardRequest>,
    ) -> Result<Response<BoardRecord>, Status> {
        let scope = Scope::of(&request)?;
        let request = request.into_inner();
        let call = self.call(&scope, &request.workspace_id)?;
        let (docs, ports) = call.ports();
        let changed = Service::new(
            &docs,
            &ports,
            &ports,
            &ports,
            scope.author.as_deref(),
            scope.role,
        )
        .update_board(&request.board, request.version, request.name, request.repo)
        .await?;
        Ok(Response::new(board(changed)))
    }

    async fn list_boards(
        &self,
        request: Request<ListBoardsRequest>,
    ) -> Result<Response<BoardList>, Status> {
        let scope = Scope::of(&request)?;
        let request = request.into_inner();
        let call = self.call(&scope, &request.workspace_id)?;
        let (docs, ports) = call.ports();
        let only = Some(request.board.as_str()).filter(|b| !b.is_empty());
        let boards = Service::new(
            &docs,
            &ports,
            &ports,
            &ports,
            scope.author.as_deref(),
            scope.role,
        )
        .boards(only)
        .await?;
        Ok(Response::new(BoardList {
            boards: boards.into_iter().map(board).collect(),
        }))
    }

    async fn add_task(
        &self,
        request: Request<AddTaskRequest>,
    ) -> Result<Response<TaskRecord>, Status> {
        let scope = Scope::of(&request)?;
        let request = request.into_inner();
        let call = self.call(&scope, &request.workspace_id)?;
        let (docs, ports) = call.ports();
        let added = Service::new(
            &docs,
            &ports,
            &ports,
            &ports,
            scope.author.as_deref(),
            scope.role,
        )
        .add_task(request)
        .await?;
        Ok(Response::new(TaskRecord {
            existing: added.existing,
            ..task(added.task)
        }))
    }

    async fn update_task(
        &self,
        request: Request<UpdateTaskRequest>,
    ) -> Result<Response<TaskRecord>, Status> {
        let scope = Scope::of(&request)?;
        let request = request.into_inner();
        let call = self.call(&scope, &request.workspace_id)?;
        let (docs, ports) = call.ports();
        let changed = Service::new(
            &docs,
            &ports,
            &ports,
            &ports,
            scope.author.as_deref(),
            scope.role,
        )
        .update_task(request)
        .await?;
        Ok(Response::new(task(changed)))
    }

    async fn move_task(
        &self,
        request: Request<MoveTaskRequest>,
    ) -> Result<Response<TaskRecord>, Status> {
        let scope = Scope::of(&request)?;
        let request = request.into_inner();
        let call = self.call(&scope, &request.workspace_id)?;
        let (docs, ports) = call.ports();
        let moved = Service::new(
            &docs,
            &ports,
            &ports,
            &ports,
            scope.author.as_deref(),
            scope.role,
        )
        .move_task(request)
        .await?;
        Ok(Response::new(task(moved)))
    }

    async fn list_tasks(
        &self,
        request: Request<ListTasksRequest>,
    ) -> Result<Response<TaskList>, Status> {
        let scope = Scope::of(&request)?;
        let request = request.into_inner();
        let call = self.call(&scope, &request.workspace_id)?;
        let (docs, ports) = call.ports();
        let column = Some(request.column.as_str()).filter(|c| !c.is_empty());
        let listing = Service::new(
            &docs,
            &ports,
            &ports,
            &ports,
            scope.author.as_deref(),
            scope.role,
        )
        .tasks(&request.board, column)
        .await?;
        Ok(Response::new(TaskList {
            tasks: listing.tasks.into_iter().map(task).collect(),
            plans: listing.plans.into_iter().map(plan).collect(),
        }))
    }

    async fn get_task(&self, request: Request<GetTaskRequest>) -> Result<Response<Thread>, Status> {
        let scope = Scope::of(&request)?;
        let request = request.into_inner();
        let call = self.call(&scope, &request.workspace_id)?;
        let (docs, ports) = call.ports();
        let found = Service::new(
            &docs,
            &ports,
            &ports,
            &ports,
            scope.author.as_deref(),
            scope.role,
        )
        .thread(&request.task)
        .await?;
        Ok(Response::new(thread(found)))
    }

    async fn add_comment(
        &self,
        request: Request<AddCommentRequest>,
    ) -> Result<Response<CommentRecord>, Status> {
        let scope = Scope::of(&request)?;
        let request = request.into_inner();
        let call = self.call(&scope, &request.workspace_id)?;
        let (docs, ports) = call.ports();
        let added = Service::new(
            &docs,
            &ports,
            &ports,
            &ports,
            scope.author.as_deref(),
            scope.role,
        )
        .comment(
            &request.task,
            &request.text,
            request.answers,
            request.attachments,
        )
        .await?;
        Ok(Response::new(comment(added)))
    }

    async fn decide(
        &self,
        request: Request<DecideRequest>,
    ) -> Result<Response<DecisionRecord>, Status> {
        let scope = Scope::of(&request)?;
        let request = request.into_inner();
        let risk = request.risk();
        let call = self.call(&scope, &request.workspace_id)?;
        let (docs, ports) = call.ports();
        let decided = Service::new(
            &docs,
            &ports,
            &ports,
            &ports,
            scope.author.as_deref(),
            scope.role,
        )
        .called_from(scope.caller_module.as_deref())
        .decide(
            &request.task,
            &request.what,
            &request.why,
            risk,
            request.stakes.unwrap_or_default(),
        )
        .await?;
        Ok(Response::new(decision(decided)))
    }

    async fn approve_decision(
        &self,
        request: Request<DecisionRequest>,
    ) -> Result<Response<DecisionRecord>, Status> {
        let scope = Scope::of(&request)?;
        let request = request.into_inner();
        let call = self.call(&scope, &request.workspace_id)?;
        let (docs, ports) = call.ports();
        let approved = Service::new(
            &docs,
            &ports,
            &ports,
            &ports,
            scope.author.as_deref(),
            scope.role,
        )
        .approve(&request.decision, request.version, request.note)
        .await?;
        Ok(Response::new(decision(approved)))
    }

    async fn revoke_approval(
        &self,
        request: Request<DecisionRequest>,
    ) -> Result<Response<DecisionRecord>, Status> {
        let scope = Scope::of(&request)?;
        let request = request.into_inner();
        let call = self.call(&scope, &request.workspace_id)?;
        let (docs, ports) = call.ports();
        let revoked = Service::new(
            &docs,
            &ports,
            &ports,
            &ports,
            scope.author.as_deref(),
            scope.role,
        )
        .revoke(&request.decision, request.version)
        .await?;
        Ok(Response::new(decision(revoked)))
    }

    async fn decline_decision(
        &self,
        request: Request<DecisionRequest>,
    ) -> Result<Response<DecisionRecord>, Status> {
        let scope = Scope::of(&request)?;
        let request = request.into_inner();
        let call = self.call(&scope, &request.workspace_id)?;
        let (docs, ports) = call.ports();
        let declined = Service::new(
            &docs,
            &ports,
            &ports,
            &ports,
            scope.author.as_deref(),
            scope.role,
        )
        .decline(&request.decision, request.version, request.note)
        .await?;
        Ok(Response::new(decision(declined)))
    }

    async fn withdraw_decision(
        &self,
        request: Request<WithdrawDecisionRequest>,
    ) -> Result<Response<DecisionRecord>, Status> {
        let scope = Scope::of(&request)?;
        let request = request.into_inner();
        let call = self.call(&scope, &request.workspace_id)?;
        let (docs, ports) = call.ports();
        let withdrawn = Service::new(
            &docs,
            &ports,
            &ports,
            &ports,
            scope.author.as_deref(),
            scope.role,
        )
        .withdraw(&request.decision, &request.why)
        .await?;
        Ok(Response::new(decision(withdrawn)))
    }

    async fn list_decisions(
        &self,
        request: Request<ListDecisionsRequest>,
    ) -> Result<Response<DecisionList>, Status> {
        let scope = Scope::of(&request)?;
        let request = request.into_inner();
        let call = self.call(&scope, &request.workspace_id)?;
        let (docs, ports) = call.ports();
        let decisions = Service::new(
            &docs,
            &ports,
            &ports,
            &ports,
            scope.author.as_deref(),
            scope.role,
        )
        .decisions(request.waiting)
        .await?;
        Ok(Response::new(DecisionList {
            decisions: decisions.into_iter().map(entry).collect(),
        }))
    }
}
