// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The runner's first look at the jobs (docs/modules/scheduler.md): one
//! interval after the start, never at once — the host makes its running
//! set callable once every module has started, and the scheduler starts
//! before that set is swapped in, so a look at once would find no module
//! serving the jobs.

use std::convert::Infallible;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use pcx_module_api::agent::Turns;
use pcx_module_api::context::ContextEngine;
use pcx_module_api::grpc::{self, GrpcChannel};
use pcx_module_api::model::ModelProvider;
use pcx_module_api::sandbox::Sandbox;
use pcx_module_api::{
    Event, EventKind, Manifest, ModuleError, ModuleErrorKind, ModuleHost, Outcome, Scope, UsedTool,
};
use tonic::body::Body;
use tonic::codegen::http::Request;

use super::*;

/// A host the runner asks nothing of while it only looks for jobs.
struct NoHost;

fn unasked(what: &str) -> ModuleError {
    ModuleError::new(
        ModuleErrorKind::FailedPrecondition,
        format!("the test host serves no {what}"),
    )
}

#[async_trait::async_trait]
impl ModuleHost for NoHost {
    async fn manifests(&self) -> Result<Vec<Manifest>, ModuleError> {
        Ok(Vec::new())
    }

    async fn sandbox(&self) -> Result<Arc<dyn Sandbox>, ModuleError> {
        Err(unasked("sandbox"))
    }

    async fn model(&self) -> Result<Arc<dyn ModelProvider>, ModuleError> {
        Err(unasked("model"))
    }

    async fn context(&self) -> Result<Option<Arc<dyn ContextEngine>>, ModuleError> {
        Ok(None)
    }

    async fn turns(&self) -> Result<Arc<dyn Turns>, ModuleError> {
        Err(unasked("turns"))
    }

    async fn turns_of(&self, _module: &str) -> Result<Arc<dyn Turns>, ModuleError> {
        Err(unasked("turns"))
    }

    async fn tools(&self, _scope: &Scope) -> Result<Vec<UsedTool>, ModuleError> {
        Ok(Vec::new())
    }

    async fn bus(&self) -> Result<Arc<dyn pcx_module_api::bus::Bus>, ModuleError> {
        Err(unasked("bus"))
    }

    async fn dispatch(
        &self,
        _scope: &Scope,
        _kind: EventKind,
        _event: &Event,
    ) -> Result<Outcome, ModuleError> {
        Ok(Outcome::default())
    }

    async fn report(&self, _problem: &str) -> Result<(), ModuleError> {
        Ok(())
    }
}

/// A channel that counts each call, then answers as the host would while
/// no module serves the jobs.
fn counting(asked: Arc<AtomicUsize>) -> GrpcChannel {
    grpc::channel(tower::service_fn(move |_request: Request<Body>| {
        asked.fetch_add(1, Ordering::SeqCst);
        async {
            Ok::<_, Infallible>(grpc::refusal(&tonic::Status::not_found(
                "no running module serves pcx.v1.PostCortex",
            )))
        }
    }))
}

/// Let the runner's loop take what is ready.
async fn settle() {
    for _ in 0..4 {
        tokio::task::yield_now().await;
    }
}

#[tokio::test(start_paused = true)]
async fn the_first_look_for_jobs_is_one_interval_after_the_start() {
    let asked = Arc::new(AtomicUsize::new(0));
    let runner = Runner::new(
        Arc::new(NoHost),
        counting(Arc::clone(&asked)),
        "ws".to_string(),
        None,
    );
    runner.start();
    settle().await;
    assert_eq!(
        asked.load(Ordering::SeqCst),
        0,
        "nothing is asked while the module starts"
    );
    // One interval later it looks once: the look itself is answered as the
    // daemon's start answers it, until the host has swapped its set in.
    tokio::time::sleep(TICK).await;
    settle().await;
    assert_eq!(
        asked.load(Ordering::SeqCst),
        1,
        "the first look is one interval after the start"
    );
    tokio::time::sleep(TICK).await;
    settle().await;
    assert_eq!(
        asked.load(Ordering::SeqCst),
        2,
        "each further interval looks once"
    );
    runner.stop();
}
