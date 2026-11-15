// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! `GET /health/jobs/{name}` (docs/modules/scheduler.md, a job watched from
//! outside): the scheduler's `JobHealth` through the host's channel of a
//! client, and its answer as an HTTP status a probe reads: 200 while the
//! job runs on time, 503 with why when it does not, 404 for no such job or
//! no scheduler. Not behind the instance's token, as `/health`: it says
//! whether a job of a name runs, and nothing of what it does.

use axum::Router;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use pcx_module_api::grpc::GrpcChannel;
use post_cortex_proto::scheduler::scheduler_client::SchedulerClient;
use post_cortex_proto::scheduler::{Health, JobHealthRequest};

use crate::grpc::http_failure;

/// The route over `channel`, the host's channel of a client.
pub fn routes(channel: GrpcChannel) -> Router {
    Router::new()
        .route("/health/jobs/{name}", get(job))
        .with_state(channel)
}

async fn job(State(channel): State<GrpcChannel>, Path(name): Path<String>) -> Response {
    match SchedulerClient::new(channel)
        .job_health(JobHealthRequest { name })
        .await
    {
        Ok(answer) => {
            let answer = answer.into_inner();
            let status = match answer.health() {
                Health::Ok => StatusCode::OK,
                Health::Paused | Health::Late | Health::Unspecified => {
                    StatusCode::SERVICE_UNAVAILABLE
                }
            };
            (status, format!("{}\n", answer.why)).into_response()
        }
        Err(status) => http_failure(&status),
    }
}
