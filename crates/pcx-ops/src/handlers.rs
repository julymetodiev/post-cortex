// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Ops' `pcx.ops.v1.Ops` service: each RPC maps its request, as its
//! caller asks it, to the service and its answer or failure back.

use pcx_module_api::{ModuleError, ModuleErrorKind, Scope};
use post_cortex_proto::ops::ops_server::Ops;
use post_cortex_proto::ops::{
    ActionAnswer, ActionList, AllocationRequest, JobRequest, ListActionsRequest, NamespaceRequest,
    ResizeRequest, RevertRequest, ScaleRequest, TargetRequest,
};
use tonic::{Request, Response, Status};

use crate::domain::Wanted;
use crate::module::{Shared, running};
use crate::service::Asked;

/// The service's handlers.
pub struct Handlers {
    /// The running module.
    pub shared: Shared,
}

/// Who asks: the call's author (an action names who asks).
fn asker(scope: &Scope) -> Result<String, ModuleError> {
    scope
        .author
        .clone()
        .filter(|a| !a.trim().is_empty())
        .ok_or_else(|| {
            ModuleError::new(
                ModuleErrorKind::FailedPrecondition,
                "an action names who asks it: the call names no author",
            )
        })
}

impl Handlers {
    async fn act(&self, scope: &Scope, asked: Asked) -> Result<Response<ActionAnswer>, Status> {
        let asker = asker(scope)?;
        let running = running(&self.shared)?;
        let docs = running.docs(scope);
        Ok(Response::new(
            running.service(&docs).act(&asker, asked).await?,
        ))
    }
}

#[tonic::async_trait]
impl Ops for Handlers {
    async fn restart(
        &self,
        request: Request<TargetRequest>,
    ) -> Result<Response<ActionAnswer>, Status> {
        let scope = Scope::of(&request)?;
        let r = request.into_inner();
        let asked = Asked {
            ask: r.ask.unwrap_or_default(),
            wanted: Wanted::Restart {
                namespace: r.namespace,
                name: r.name,
            },
        };
        self.act(&scope, asked).await
    }

    async fn delete_pod(
        &self,
        request: Request<TargetRequest>,
    ) -> Result<Response<ActionAnswer>, Status> {
        let scope = Scope::of(&request)?;
        let r = request.into_inner();
        let asked = Asked {
            ask: r.ask.unwrap_or_default(),
            wanted: Wanted::DeletePod {
                namespace: r.namespace,
                name: r.name,
            },
        };
        self.act(&scope, asked).await
    }

    async fn clean_jobs(
        &self,
        request: Request<NamespaceRequest>,
    ) -> Result<Response<ActionAnswer>, Status> {
        let scope = Scope::of(&request)?;
        let r = request.into_inner();
        let asked = Asked {
            ask: r.ask.unwrap_or_default(),
            wanted: Wanted::CleanJobs {
                namespace: r.namespace,
            },
        };
        self.act(&scope, asked).await
    }

    async fn scale(
        &self,
        request: Request<ScaleRequest>,
    ) -> Result<Response<ActionAnswer>, Status> {
        let scope = Scope::of(&request)?;
        let r = request.into_inner();
        let asked = Asked {
            ask: r.ask.unwrap_or_default(),
            wanted: Wanted::Scale {
                namespace: r.namespace,
                name: r.name,
                replicas: r.replicas,
            },
        };
        self.act(&scope, asked).await
    }

    async fn resize_volume(
        &self,
        request: Request<ResizeRequest>,
    ) -> Result<Response<ActionAnswer>, Status> {
        let scope = Scope::of(&request)?;
        let r = request.into_inner();
        let asked = Asked {
            ask: r.ask.unwrap_or_default(),
            wanted: Wanted::ResizeVolume {
                namespace: r.namespace,
                name: r.name,
                gibibytes: r.gibibytes,
            },
        };
        self.act(&scope, asked).await
    }

    async fn new_allocation(
        &self,
        request: Request<AllocationRequest>,
    ) -> Result<Response<ActionAnswer>, Status> {
        let scope = Scope::of(&request)?;
        let r = request.into_inner();
        let asked = Asked {
            ask: r.ask.unwrap_or_default(),
            wanted: Wanted::NewAllocation {
                allocation: r.allocation,
            },
        };
        self.act(&scope, asked).await
    }

    async fn run_periodic(
        &self,
        request: Request<JobRequest>,
    ) -> Result<Response<ActionAnswer>, Status> {
        let scope = Scope::of(&request)?;
        let r = request.into_inner();
        let asked = Asked {
            ask: r.ask.unwrap_or_default(),
            wanted: Wanted::RunPeriodic { job: r.job },
        };
        self.act(&scope, asked).await
    }

    async fn revert_jobs(
        &self,
        request: Request<RevertRequest>,
    ) -> Result<Response<ActionAnswer>, Status> {
        let scope = Scope::of(&request)?;
        let r = request.into_inner();
        let asked = Asked {
            ask: r.ask.unwrap_or_default(),
            wanted: Wanted::RevertJobs { jobs: r.jobs },
        };
        self.act(&scope, asked).await
    }

    async fn list_actions(
        &self,
        request: Request<ListActionsRequest>,
    ) -> Result<Response<ActionList>, Status> {
        let scope = Scope::of(&request)?;
        let running = running(&self.shared)?;
        let docs = running.docs(&scope);
        let limit = request.into_inner().limit as usize;
        Ok(Response::new(running.service(&docs).list(limit).await?))
    }
}
