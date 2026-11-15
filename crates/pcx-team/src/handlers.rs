// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The team's `pcx.team.v1.Teams` service (W23, `ListWork` W25): each RPC's message and
//! the call's scope in (its workspace), the team service's call, its
//! message out. No logic of their own.

use pcx_documents::HostDocuments;
use pcx_module_api::{ModuleError, ModuleErrorKind, Scope};
use post_cortex_proto::team::teams_server::Teams;
use post_cortex_proto::team::{
    AddAgentRequest, AgentRecord, AutonomyRecord, CreateTeamRequest, GetAutonomyRequest,
    ListTeamsRequest, ListWorkRequest, MemberRequest, PauseAutonomyRequest, RosterList,
    SetAutonomyRequest, StopWorkRequest, StopWorkResponse, TeamRecord, UpdateAgentRequest,
    WorkList,
};
use tonic::{Request, Response, Status};

use crate::adapters::HostWorld;
use crate::doing::{Doing, WorkFilter};
use crate::module::{self, Shared};
use crate::service::Service;

/// The team's RPCs over the running module.
pub struct Handlers {
    /// The running module.
    pub shared: Shared,
}

impl Handlers {
    /// The documents of the workspace a call in `scope` works in (`named`
    /// when its scope names none).
    fn docs<'a>(&self, scope: &'a Scope, named: &str) -> Result<Docs<'a>, Status> {
        let running = module::running(&self.shared)?;
        let workspace = scope.workspace(named)?;
        Ok(Docs {
            channel: running.channel,
            scope,
            workspace,
        })
    }
}

/// What a call's documents are made of.
struct Docs<'a> {
    channel: pcx_module_api::grpc::GrpcChannel,
    scope: &'a Scope,
    workspace: String,
}

impl Docs<'_> {
    fn documents(&self) -> HostDocuments<'_> {
        HostDocuments::new(self.channel.clone(), self.scope, &self.workspace)
    }
}

fn required<T>(field: &str, value: Option<T>) -> Result<T, Status> {
    value.ok_or_else(|| {
        Status::from(ModuleError::new(
            ModuleErrorKind::InvalidArgument,
            format!("the request has no {field}"),
        ))
    })
}

#[tonic::async_trait]
impl Teams for Handlers {
    async fn get_autonomy(
        &self,
        request: Request<GetAutonomyRequest>,
    ) -> Result<Response<AutonomyRecord>, Status> {
        let scope = Scope::of(&request)?;
        let request = request.into_inner();
        let docs = self.docs(&scope, &request.workspace_id)?;
        let documents = docs.documents();
        Ok(Response::new(
            Service::new(&documents).autonomy(&request.team).await?,
        ))
    }

    async fn set_autonomy(
        &self,
        request: Request<SetAutonomyRequest>,
    ) -> Result<Response<AutonomyRecord>, Status> {
        let scope = Scope::of(&request)?;
        let request = request.into_inner();
        let docs = self.docs(&scope, &request.workspace_id)?;
        let documents = docs.documents();
        let set = Service::new(&documents)
            .set_autonomy(
                &request.team,
                request.version,
                required("trust", request.trust)?,
                request.exceptions.map(|e| e.exceptions),
            )
            .await?;
        Ok(Response::new(set))
    }

    async fn pause_autonomy(
        &self,
        request: Request<PauseAutonomyRequest>,
    ) -> Result<Response<AutonomyRecord>, Status> {
        let scope = Scope::of(&request)?;
        let request = request.into_inner();
        let docs = self.docs(&scope, &request.workspace_id)?;
        let documents = docs.documents();
        let paused = Service::new(&documents)
            .pause(&request.team, request.version, request.paused)
            .await?;
        Ok(Response::new(paused))
    }

    async fn create_team(
        &self,
        request: Request<CreateTeamRequest>,
    ) -> Result<Response<TeamRecord>, Status> {
        let scope = Scope::of(&request)?;
        let request = request.into_inner();
        let docs = self.docs(&scope, &request.workspace_id)?;
        let documents = docs.documents();
        let team = Service::new(&documents)
            .create_team(&request.name, request.members, &docs.workspace)
            .await?;
        Ok(Response::new(team))
    }

    async fn list_teams(
        &self,
        request: Request<ListTeamsRequest>,
    ) -> Result<Response<RosterList>, Status> {
        let scope = Scope::of(&request)?;
        let request = request.into_inner();
        let docs = self.docs(&scope, &request.workspace_id)?;
        let documents = docs.documents();
        let only = Some(request.team.as_str()).filter(|t| !t.is_empty());
        Ok(Response::new(
            Service::new(&documents).list(only, &docs.workspace).await?,
        ))
    }

    async fn add_agent(
        &self,
        request: Request<AddAgentRequest>,
    ) -> Result<Response<AgentRecord>, Status> {
        let scope = Scope::of(&request)?;
        let request = request.into_inner();
        let docs = self.docs(&scope, &request.workspace_id)?;
        let documents = docs.documents();
        let added = Service::new(&documents)
            .add_agent(required("agent", request.agent)?)
            .await?;
        Ok(Response::new(added))
    }

    async fn update_agent(
        &self,
        request: Request<UpdateAgentRequest>,
    ) -> Result<Response<AgentRecord>, Status> {
        let scope = Scope::of(&request)?;
        let request = request.into_inner();
        let docs = self.docs(&scope, &request.workspace_id)?;
        let documents = docs.documents();
        Ok(Response::new(
            Service::new(&documents).update_agent(request).await?,
        ))
    }

    async fn list_work(
        &self,
        request: Request<ListWorkRequest>,
    ) -> Result<Response<WorkList>, Status> {
        let scope = Scope::of(&request)?;
        let request = request.into_inner();
        let docs = self.docs(&scope, &request.workspace_id)?;
        let documents = docs.documents();
        let board = HostWorld::new(docs.channel.clone(), docs.scope, &docs.workspace);
        let filter = WorkFilter {
            team: Some(request.team.as_str()).filter(|t| !t.is_empty()),
            task: Some(request.task.as_str()).filter(|t| !t.is_empty()),
            busy_only: request.busy_only,
            limit: request.limit,
        };
        let busy = module::running(&self.shared)?.busy(&docs.workspace);
        let work = Doing {
            docs: &documents,
            board: &board,
        }
        .list(&filter, &busy)
        .await?;
        Ok(Response::new(WorkList { work }))
    }

    async fn stop_work(
        &self,
        request: Request<StopWorkRequest>,
    ) -> Result<Response<StopWorkResponse>, Status> {
        let scope = Scope::of(&request)?;
        let request = request.into_inner();
        let docs = self.docs(&scope, &request.workspace_id)?;
        let stopped = module::running(&self.shared)?.stop_work(
            &docs.workspace,
            &request.task,
            &request.member,
        );
        Ok(Response::new(StopWorkResponse { stopped }))
    }

    async fn add_member(
        &self,
        request: Request<MemberRequest>,
    ) -> Result<Response<TeamRecord>, Status> {
        self.member(request, false).await
    }

    async fn remove_member(
        &self,
        request: Request<MemberRequest>,
    ) -> Result<Response<TeamRecord>, Status> {
        self.member(request, true).await
    }
}

impl Handlers {
    /// Add a member to a team, or with `remove` take it out.
    async fn member(
        &self,
        request: Request<MemberRequest>,
        remove: bool,
    ) -> Result<Response<TeamRecord>, Status> {
        let scope = Scope::of(&request)?;
        let request = request.into_inner();
        let docs = self.docs(&scope, &request.workspace_id)?;
        let documents = docs.documents();
        let team = Service::new(&documents)
            .set_member(
                &request.team,
                request.version,
                &request.member,
                remove,
                &docs.workspace,
            )
            .await?;
        Ok(Response::new(team))
    }
}
