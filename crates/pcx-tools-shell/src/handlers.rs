// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The `pcx.tools.shell.v1.Shell` service of `tools-shell` (W23): each
//! RPC's message in, the typed call of the shell or tmux service, its
//! message out. No logic of their own.

use pcx_module_api::Scope;
use pcx_module_api::sandbox::{ExecExit, SandboxPath};
use post_cortex_proto::tools_shell::shell_server::Shell as ShellService;
use post_cortex_proto::tools_shell::{
    BashRequest, BashResponse, Cwd, Exit, TmuxEntry, TmuxKillRequest, TmuxListRequest,
    TmuxReadRequest, TmuxReadResponse, TmuxSendRequest, TmuxSession, TmuxSessions,
    TmuxStartRequest, exit,
};
use tonic::{Request, Response, Status};

use crate::module::{self, Shared};
use crate::tmux::Tmux;

/// The module's RPCs over its sandbox.
pub struct Handlers {
    /// The running module.
    pub shared: Shared,
}

/// Where a command starts: `cwd` under `root`; none: the first root.
fn cwd(at: Option<Cwd>) -> Option<SandboxPath> {
    let at = at.unwrap_or_default();
    let root = Some(at.root.as_str()).filter(|r| !r.is_empty());
    Some(at.cwd)
        .filter(|c| !c.is_empty())
        .map(|c| SandboxPath::under(c, root))
}

/// How a process ended, on the wire.
fn ended(exit: ExecExit) -> Exit {
    let how = match exit {
        ExecExit::Code(code) => exit::How::Code(code),
        ExecExit::Signal => exit::How::Signal(true),
        ExecExit::TimedOut => exit::How::TimedOut(true),
        ExecExit::Cancelled => exit::How::Cancelled(true),
    };
    Exit { how: Some(how) }
}

#[tonic::async_trait]
impl ShellService for Handlers {
    async fn bash(&self, request: Request<BashRequest>) -> Result<Response<BashResponse>, Status> {
        let agent = Scope::of(&request)?.agent_author();
        let request = request.into_inner();
        let started = module::started(&self.shared)?;
        let sandbox = started.host.sandbox().await?;
        let timeout = (request.timeout != 0).then_some(u64::from(request.timeout));
        let ran = started
            .shell(sandbox.as_ref(), agent.as_ref())
            .bash(&request.command, cwd(request.at), timeout)
            .await?;
        Ok(Response::new(BashResponse {
            output: ran.output,
            truncated: ran.truncated,
            lines: ran.lines as u64,
            exit: Some(ended(ran.exit)),
        }))
    }

    async fn tmux_start(
        &self,
        request: Request<TmuxStartRequest>,
    ) -> Result<Response<TmuxSession>, Status> {
        let agent = Scope::of(&request)?.agent_author();
        let request = request.into_inner();
        let started = module::started(&self.shared)?;
        let sandbox = started.host.sandbox().await?;
        let shell = started.shell(sandbox.as_ref(), agent.as_ref());
        let session = Tmux::new(&shell)
            .start(&request.command, cwd(request.at))
            .await?;
        Ok(Response::new(TmuxSession { session }))
    }

    async fn tmux_read(
        &self,
        request: Request<TmuxReadRequest>,
    ) -> Result<Response<TmuxReadResponse>, Status> {
        let request = request.into_inner();
        let started = module::started(&self.shared)?;
        let sandbox = started.host.sandbox().await?;
        let shell = started.shell(sandbox.as_ref(), None);
        let lines = (request.lines != 0).then_some(request.lines as usize);
        let (pane, exited) = Tmux::new(&shell).read(&request.session, lines).await?;
        Ok(Response::new(TmuxReadResponse {
            session: request.session,
            pane,
            exited,
        }))
    }

    async fn tmux_send(
        &self,
        request: Request<TmuxSendRequest>,
    ) -> Result<Response<TmuxSession>, Status> {
        let request = request.into_inner();
        let started = module::started(&self.shared)?;
        let sandbox = started.host.sandbox().await?;
        let shell = started.shell(sandbox.as_ref(), None);
        Tmux::new(&shell)
            .send(
                &request.session,
                &request.text,
                request.enter.unwrap_or(true),
            )
            .await?;
        Ok(Response::new(TmuxSession {
            session: request.session,
        }))
    }

    async fn tmux_list(
        &self,
        _request: Request<TmuxListRequest>,
    ) -> Result<Response<TmuxSessions>, Status> {
        let started = module::started(&self.shared)?;
        let sandbox = started.host.sandbox().await?;
        let shell = started.shell(sandbox.as_ref(), None);
        let sessions = Tmux::new(&shell).list().await?;
        Ok(Response::new(TmuxSessions {
            sessions: sessions
                .into_iter()
                .map(|s| TmuxEntry {
                    session: s.name,
                    command: s.command,
                    exited: s.exited,
                })
                .collect(),
        }))
    }

    async fn tmux_kill(
        &self,
        request: Request<TmuxKillRequest>,
    ) -> Result<Response<TmuxSession>, Status> {
        let request = request.into_inner();
        let started = module::started(&self.shared)?;
        let sandbox = started.host.sandbox().await?;
        let shell = started.shell(sandbox.as_ref(), None);
        Tmux::new(&shell).kill(&request.session).await?;
        Ok(Response::new(TmuxSession {
            session: request.session,
        }))
    }
}
