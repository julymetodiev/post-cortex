// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The `pcx.tools.git.v1.Git` service of `tools-git` (W23): each RPC's
//! message in, the typed call of the git service, its message out. No
//! logic of their own.

use pcx_module_api::Scope;
use pcx_module_api::sandbox::SandboxPath;
use post_cortex_proto::tools_git::git_server::Git as GitService;
use post_cortex_proto::tools_git::{
    AddWorktreeRequest, BranchCommitsRequest, BranchList, BranchesRequest, CommitRequest,
    CommitResponse, Commits, CreateBranchRequest, CreateBranchResponse, DiffRequest, DiffResponse,
    LogRequest, PushRequest, Pushed, Repo, StatusRequest, StatusResponse, SwitchRequest,
    SwitchResponse, Worktree,
};
use tonic::{Request, Response, Status};

use crate::module::{self, Shared};

/// The module's RPCs over its sandbox.
pub struct Handlers {
    /// The running module.
    pub shared: Shared,
}

/// Some text, or none when empty.
fn named(text: &str) -> Option<&str> {
    Some(text).filter(|t| !t.is_empty())
}

/// The repository a call names: `repo` under `root`; none: the first
/// root.
fn repo(at: Option<Repo>) -> Option<SandboxPath> {
    let at = at.unwrap_or_default();
    named(&at.repo).map(|r| SandboxPath::under(r, named(&at.root)))
}

/// A count on the wire.
fn n(count: usize) -> u64 {
    count as u64
}

#[tonic::async_trait]
impl GitService for Handlers {
    async fn status(
        &self,
        request: Request<StatusRequest>,
    ) -> Result<Response<StatusResponse>, Status> {
        let request = request.into_inner();
        let started = module::started(&self.shared)?;
        let sandbox = started.host.sandbox().await?;
        let changes = started
            .git(sandbox.as_ref())
            .status(repo(request.at))
            .await?;
        Ok(Response::new(StatusResponse { changes }))
    }

    async fn diff(&self, request: Request<DiffRequest>) -> Result<Response<DiffResponse>, Status> {
        let request = request.into_inner();
        let started = module::started(&self.shared)?;
        let sandbox = started.host.sandbox().await?;
        let patch = started
            .git(sandbox.as_ref())
            .diff(repo(request.at), Some(request.staged), named(&request.path))
            .await?;
        Ok(Response::new(DiffResponse {
            truncated: patch.truncated(),
            lines: n(patch.total),
            shown: n(patch.shown),
            patch: patch.text,
        }))
    }

    async fn log(&self, request: Request<LogRequest>) -> Result<Response<Commits>, Status> {
        let request = request.into_inner();
        let started = module::started(&self.shared)?;
        let sandbox = started.host.sandbox().await?;
        let limit = (request.limit != 0).then_some(request.limit as usize);
        let commits = started
            .git(sandbox.as_ref())
            .log(repo(request.at), limit, named(&request.rev))
            .await?;
        Ok(Response::new(Commits { commits }))
    }

    async fn branches(
        &self,
        request: Request<BranchesRequest>,
    ) -> Result<Response<BranchList>, Status> {
        let request = request.into_inner();
        let started = module::started(&self.shared)?;
        let sandbox = started.host.sandbox().await?;
        let branches = started
            .git(sandbox.as_ref())
            .branches(repo(request.at))
            .await?;
        Ok(Response::new(BranchList { branches }))
    }

    async fn create_branch(
        &self,
        request: Request<CreateBranchRequest>,
    ) -> Result<Response<CreateBranchResponse>, Status> {
        let request = request.into_inner();
        let started = module::started(&self.shared)?;
        let sandbox = started.host.sandbox().await?;
        let switched = started
            .git(sandbox.as_ref())
            .create_branch(
                repo(request.at),
                &request.name,
                named(&request.from),
                request.switch,
            )
            .await?;
        Ok(Response::new(CreateBranchResponse {
            name: request.name,
            switched,
        }))
    }

    async fn switch(
        &self,
        request: Request<SwitchRequest>,
    ) -> Result<Response<SwitchResponse>, Status> {
        let request = request.into_inner();
        let started = module::started(&self.shared)?;
        let sandbox = started.host.sandbox().await?;
        started
            .git(sandbox.as_ref())
            .switch(repo(request.at), &request.name)
            .await?;
        Ok(Response::new(SwitchResponse { name: request.name }))
    }

    async fn commit(
        &self,
        request: Request<CommitRequest>,
    ) -> Result<Response<CommitResponse>, Status> {
        let agent = Scope::of(&request)?.agent_author();
        let request = request.into_inner();
        let started = module::started(&self.shared)?;
        let sandbox = started.host.sandbox().await?;
        let committed = started
            .git(sandbox.as_ref())
            .commit(
                repo(request.at),
                &request.message,
                &request.paths,
                agent.as_ref(),
            )
            .await?;
        Ok(Response::new(CommitResponse {
            sha: committed.sha,
            conventional: committed.conventional,
        }))
    }

    async fn branch_commits(
        &self,
        request: Request<BranchCommitsRequest>,
    ) -> Result<Response<Commits>, Status> {
        let request = request.into_inner();
        let started = module::started(&self.shared)?;
        let sandbox = started.host.sandbox().await?;
        let commits = started
            .git(sandbox.as_ref())
            .branch_commits(repo(request.at), &request.branch, named(&request.base))
            .await?;
        Ok(Response::new(Commits { commits }))
    }

    async fn add_worktree(
        &self,
        request: Request<AddWorktreeRequest>,
    ) -> Result<Response<Worktree>, Status> {
        let request = request.into_inner();
        let started = module::started(&self.shared)?;
        let sandbox = started.host.sandbox().await?;
        let (path, existing) = started
            .git(sandbox.as_ref())
            .add_worktree(
                repo(request.at),
                &request.path,
                &request.branch,
                named(&request.from),
            )
            .await?;
        Ok(Response::new(Worktree {
            path,
            branch: request.branch,
            existing,
        }))
    }

    async fn push(&self, request: Request<PushRequest>) -> Result<Response<Pushed>, Status> {
        let request = request.into_inner();
        let started = module::started(&self.shared)?;
        let sandbox = started.host.sandbox().await?;
        let (remote, sha) = started
            .git(sandbox.as_ref())
            .push(repo(request.at), &request.branch)
            .await?;
        Ok(Response::new(Pushed {
            remote,
            branch: request.branch,
            sha,
        }))
    }
}
