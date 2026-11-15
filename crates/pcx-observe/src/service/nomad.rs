// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! A Nomad environment read (ARCHITECTURE section 8, operations): its jobs
//! with their allocations, the end of a task's log, and its nodes with
//! their memory and disks. Nomad's answers are decoded without their nulls
//! (`pcx-nomad`) into the mirror messages.

use pcx_module_api::ModuleError;
use pcx_nomad::paths;
use post_cortex_proto::observe::{
    AllocLogRequest, AllocLogTail, Environment, Jobs, JobsRequest, Nodes, NodesRequest,
    NomadAllocation, NomadHostStats, NomadJob, NomadJobStub, NomadNode,
};
use serde::de::DeserializeOwned;

use crate::domain::{self, ALLOC_LINES, Refused};
use crate::ports::Api;
use crate::service::{LOG_BYTES, Service, failed, refused, undecoded};

/// The API's name, as a failure says it.
const NOMAD: &str = "Nomad";

/// Nomad's answer `body` as `T`.
fn read<T: DeserializeOwned>(body: &[u8]) -> Result<T, ModuleError> {
    pcx_module_api::json::decode(body).map_err(|e| undecoded(NOMAD, &e))
}

impl Service<'_> {
    /// What Nomad answers at `path` with `query`, as `T`.
    async fn nomad<T: DeserializeOwned>(
        &self,
        env: &Environment,
        path: &str,
        query: &[(&str, String)],
    ) -> Result<T, ModuleError> {
        let base = domain::nomad(env).map_err(refused)?;
        let url = format!("{base}{path}");
        let (ok, body) = self.apis.get(env, Api::Nomad, &url, query).await?;
        if !ok {
            return Err(failed(NOMAD, &body));
        }
        read(&body)
    }

    /// A Nomad environment's jobs, each with its version, its allocations
    /// and, when it is periodic, its schedule and launches.
    ///
    /// # Errors
    /// An unknown environment, or one that is not Nomad's; Nomad refused
    /// or failed.
    pub async fn jobs(&self, request: JobsRequest) -> Result<Jobs, ModuleError> {
        let env = self.environment(&request.environment)?;
        let stubs: Vec<NomadJobStub> = self.nomad(env, paths::JOBS, &[]).await?;
        let mut details: Vec<NomadJob> = Vec::new();
        for job in domain::parents(&stubs) {
            if pcx_nomad::name(&job.id) {
                details.push(self.nomad(env, &paths::job(&job.id), &[]).await?);
            }
        }
        let allocations: Vec<NomadAllocation> = self.nomad(env, paths::ALLOCATIONS, &[]).await?;
        Ok(Jobs {
            jobs: domain::jobs(&stubs, &details, &allocations),
        })
    }

    /// The last lines of a task's log in an allocation.
    ///
    /// # Errors
    /// As [`Service::jobs`]; an allocation that is no id nor the start of
    /// one, that none or several start with; a task the allocation does
    /// not have, or none named when it has several.
    pub async fn alloc_log(&self, request: AllocLogRequest) -> Result<AllocLogTail, ModuleError> {
        let env = self.environment(&request.environment)?;
        let base = domain::nomad(env).map_err(refused)?;
        let asked = request.allocation.trim();
        if !pcx_nomad::allocation(asked) {
            return Err(refused(Refused::AllocationId));
        }
        if !request.task.is_empty() && !pcx_nomad::name(&request.task) {
            return Err(refused(Refused::NomadName("task")));
        }
        let start = [("prefix", pcx_nomad::prefix(asked).to_string())];
        let found: Vec<NomadAllocation> = self.nomad(env, paths::ALLOCATIONS, &start).await?;
        let allocation = domain::allocation_of(found, asked).map_err(refused)?;
        let task = domain::task_of(&allocation, &request.task).map_err(refused)?;
        let stream = if request.stderr { "stderr" } else { "stdout" };
        let query = [
            ("task", task),
            ("type", stream.to_string()),
            ("origin", "end".to_string()),
            ("offset", LOG_BYTES.to_string()),
            ("plain", "true".to_string()),
        ];
        let url = format!("{base}{}", paths::log(&allocation.id));
        let (ok, body) = self.apis.get(env, Api::Nomad, &url, &query).await?;
        if !ok {
            return Err(failed(NOMAD, &body));
        }
        let lines = domain::within(request.tail, ALLOC_LINES);
        let text = String::from_utf8_lossy(&body);
        Ok(AllocLogTail {
            text: domain::tail(&text, lines, body.len() >= LOG_BYTES),
        })
    }

    /// A Nomad environment's nodes, each with its memory and disks. A node
    /// whose statistics cannot be read (it is down) is answered with why.
    ///
    /// # Errors
    /// As [`Service::jobs`].
    pub async fn nodes(&self, request: NodesRequest) -> Result<Nodes, ModuleError> {
        let env = self.environment(&request.environment)?;
        let base = domain::nomad(env).map_err(refused)?;
        let mut listed: Vec<NomadNode> = self.nomad(env, paths::NODES, &[]).await?;
        listed.sort_by(|a, b| a.name.cmp(&b.name));
        let url = format!("{base}{}", paths::STATS);
        let mut nodes = Vec::with_capacity(listed.len());
        for node in &listed {
            let of = [("node_id", node.id.clone())];
            let stats = match self.apis.get(env, Api::Nomad, &url, &of).await {
                Ok((true, body)) => read::<NomadHostStats>(&body).map_err(|e| e.message),
                Ok((false, body)) => Err(failed(NOMAD, &body).message),
                Err(unreached) => Err(unreached.message),
            };
            nodes.push(domain::node(node, stats));
        }
        Ok(Nodes { nodes })
    }
}
