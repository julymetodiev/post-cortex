// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The scheduler's `pcx.scheduler.v1.Scheduler` service: each RPC maps its
//! request to the service (or the runner, to run a job now) and its answer
//! or failure back.

use pcx_documents::HostDocuments;
use pcx_module_api::Scope;
use post_cortex_proto::scheduler::scheduler_server::Scheduler;
use post_cortex_proto::scheduler::{
    AddJobRequest, Health, JobHealthAnswer, JobHealthRequest, JobList, JobRecord, JobRequest,
    ListJobsRequest, ListRunsRequest, RemoveJobResponse, RunList, RunRecord, UpdateJobRequest,
};
use tonic::{Request, Response, Status};

use crate::adapters::SystemClock;
use crate::domain;
use crate::module::{Shared, running};
use crate::service::Service;

/// The service's handlers.
pub struct Handlers {
    /// The running module.
    pub shared: Shared,
}

#[tonic::async_trait]
impl Scheduler for Handlers {
    async fn add_job(
        &self,
        request: Request<AddJobRequest>,
    ) -> Result<Response<JobRecord>, Status> {
        let scope = Scope::of(&request)?;
        let runner = running(&self.shared)?;
        let workspace = runner.asked_from(&scope)?;
        let docs = HostDocuments::new(runner.channel(), &scope, &workspace);
        let added = Service::new(&docs, &SystemClock)
            .add_job(request.into_inner().job.unwrap_or_default())
            .await?;
        Ok(Response::new(added))
    }

    async fn update_job(
        &self,
        request: Request<UpdateJobRequest>,
    ) -> Result<Response<JobRecord>, Status> {
        let scope = Scope::of(&request)?;
        let runner = running(&self.shared)?;
        let workspace = runner.asked_from(&scope)?;
        let docs = HostDocuments::new(runner.channel(), &scope, &workspace);
        let changed = Service::new(&docs, &SystemClock)
            .update_job(request.into_inner())
            .await?;
        Ok(Response::new(changed))
    }

    async fn list_jobs(
        &self,
        request: Request<ListJobsRequest>,
    ) -> Result<Response<JobList>, Status> {
        let scope = Scope::of(&request)?;
        let runner = running(&self.shared)?;
        let workspace = runner.asked_from(&scope)?;
        let docs = HostDocuments::new(runner.channel(), &scope, &workspace);
        Ok(Response::new(
            Service::new(&docs, &SystemClock).list_jobs().await?,
        ))
    }

    async fn remove_job(
        &self,
        request: Request<JobRequest>,
    ) -> Result<Response<RemoveJobResponse>, Status> {
        let scope = Scope::of(&request)?;
        let runner = running(&self.shared)?;
        let workspace = runner.asked_from(&scope)?;
        let docs = HostDocuments::new(runner.channel(), &scope, &workspace);
        Service::new(&docs, &SystemClock)
            .remove_job(&request.into_inner().id)
            .await?;
        Ok(Response::new(RemoveJobResponse {}))
    }

    async fn run_job(&self, request: Request<JobRequest>) -> Result<Response<RunRecord>, Status> {
        let scope = Scope::of(&request)?;
        let runner = running(&self.shared)?;
        let workspace = runner.asked_from(&scope)?;
        Ok(Response::new(
            runner.run_now(&workspace, &request.into_inner().id).await?,
        ))
    }

    async fn list_runs(
        &self,
        request: Request<ListRunsRequest>,
    ) -> Result<Response<RunList>, Status> {
        let scope = Scope::of(&request)?;
        let runner = running(&self.shared)?;
        let workspace = runner.asked_from(&scope)?;
        let docs = HostDocuments::new(runner.channel(), &scope, &workspace);
        let request = request.into_inner();
        let limit = usize::try_from(request.limit).unwrap_or(usize::MAX);
        let runs = Service::new(&docs, &SystemClock)
            .list_runs(&request.job, limit)
            .await?;
        Ok(Response::new(runs))
    }

    async fn job_health(
        &self,
        request: Request<JobHealthRequest>,
    ) -> Result<Response<JobHealthAnswer>, Status> {
        let runner = running(&self.shared)?;
        let stands = runner.health(&request.into_inner().name).await?;
        let health = match stands {
            domain::Health::Ok => Health::Ok,
            domain::Health::Paused => Health::Paused,
            domain::Health::Late(_) => Health::Late,
        };
        Ok(Response::new(JobHealthAnswer {
            health: health.into(),
            why: stands.to_string(),
        }))
    }
}
