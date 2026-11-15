// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Observe's `pcx.observe.v1.Observe` service: each RPC maps its request
//! to the service and its answer or failure back.

use post_cortex_proto::observe::observe_server::Observe;
use post_cortex_proto::observe::{
    AlertList, AlertsRequest, AllocLogRequest, AllocLogTail, Cluster, ClusterRequest, Jobs,
    JobsRequest, LogStreams, LogsRequest, MetricSeries, MetricsRequest, Nodes, NodesRequest,
    PodLogRequest, PodLogTail,
};
use tonic::{Request, Response, Status};

use crate::adapters::SystemClock;
use crate::module::{Shared, running};
use crate::service::Service;

/// The service's handlers.
pub struct Handlers {
    /// The running module.
    pub shared: Shared,
}

#[tonic::async_trait]
impl Observe for Handlers {
    async fn query_metrics(
        &self,
        request: Request<MetricsRequest>,
    ) -> Result<Response<MetricSeries>, Status> {
        let running = running(&self.shared)?;
        let service = Service::new(&running.config, &running.apis, &SystemClock);
        Ok(Response::new(service.metrics(request.into_inner()).await?))
    }

    async fn query_logs(
        &self,
        request: Request<LogsRequest>,
    ) -> Result<Response<LogStreams>, Status> {
        let running = running(&self.shared)?;
        let service = Service::new(&running.config, &running.apis, &SystemClock);
        Ok(Response::new(service.logs(request.into_inner()).await?))
    }

    async fn list_alerts(
        &self,
        request: Request<AlertsRequest>,
    ) -> Result<Response<AlertList>, Status> {
        let running = running(&self.shared)?;
        let service = Service::new(&running.config, &running.apis, &SystemClock);
        Ok(Response::new(service.alerts(request.into_inner()).await?))
    }

    async fn get_cluster(
        &self,
        request: Request<ClusterRequest>,
    ) -> Result<Response<Cluster>, Status> {
        let running = running(&self.shared)?;
        let service = Service::new(&running.config, &running.apis, &SystemClock);
        Ok(Response::new(service.cluster(request.into_inner()).await?))
    }

    async fn pod_log(
        &self,
        request: Request<PodLogRequest>,
    ) -> Result<Response<PodLogTail>, Status> {
        let running = running(&self.shared)?;
        let service = Service::new(&running.config, &running.apis, &SystemClock);
        Ok(Response::new(service.pod_log(request.into_inner()).await?))
    }

    async fn get_jobs(&self, request: Request<JobsRequest>) -> Result<Response<Jobs>, Status> {
        let running = running(&self.shared)?;
        let service = Service::new(&running.config, &running.apis, &SystemClock);
        Ok(Response::new(service.jobs(request.into_inner()).await?))
    }

    async fn alloc_log(
        &self,
        request: Request<AllocLogRequest>,
    ) -> Result<Response<AllocLogTail>, Status> {
        let running = running(&self.shared)?;
        let service = Service::new(&running.config, &running.apis, &SystemClock);
        Ok(Response::new(
            service.alloc_log(request.into_inner()).await?,
        ))
    }

    async fn get_nodes(&self, request: Request<NodesRequest>) -> Result<Response<Nodes>, Status> {
        let running = running(&self.shared)?;
        let service = Service::new(&running.config, &running.apis, &SystemClock);
        Ok(Response::new(service.nodes(request.into_inner()).await?))
    }
}
