// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Ops' ports over its host handle: the cluster, Kubernetes or Nomad,
//! through `pcx-kube` with the environment's own account (its files read
//! through the sandbox at each call), each body encoded and each answer
//! decoded by the proto3 JSON mapping of the mirror messages (Nomad's
//! without its nulls, `pcx-nomad`); the board's `Decide`, `GetTask`,
//! `WithdrawDecision` and `AddComment`; the channel's `Send`; the system clock.

use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use pcx_kube::{Answer, Body, Caller, Clients, Credentials, Method};
use pcx_module_api::grpc::GrpcChannel;
use pcx_module_api::{ModuleError, ModuleErrorKind, ModuleHost, Scope, channel};
use pcx_nomad::paths;
use post_cortex_proto::board::boards_client::BoardsClient;
use post_cortex_proto::board::{
    AddCommentRequest, DecideRequest, DecisionRecord, GetTaskRequest, Risk, WithdrawDecisionRequest,
};
use post_cortex_proto::channel::channel_client::ChannelClient;
use post_cortex_proto::ops::{
    Environment, KubeThing, NomadAlloc, NomadChildren, NomadDeployment, NomadJob, NomadRevert,
    NomadSummary, NomadVersions, Revert, ThingList,
};
use serde::de::DeserializeOwned;
use tokio_util::sync::CancellationToken;
use tonic::Request;

use crate::domain::MODULE_ID;
use crate::ports::{Board, Clock, Cluster, Decision, Nomad, Patch, Teller};

/// The media type of a merge patch.
const MERGE_PATCH: &str = "application/merge-patch+json";

/// The media type of a body Nomad is sent.
const JSON: &str = "application/json";

/// One API of an environment: where it is, its account's files, and what
/// a failure calls it.
struct Reach<'a> {
    base: &'a str,
    credentials: Credentials<'a>,
    who: &'static str,
}

/// `env`'s Kubernetes API.
fn kube(env: &Environment) -> Reach<'_> {
    Reach {
        base: &env.kube_api,
        credentials: Credentials {
            token_file: &env.kube_token_file,
            ca_file: &env.kube_ca_file,
            tls_server_name: &env.kube_tls_server_name,
        },
        who: "the cluster",
    }
}

/// `env`'s Nomad API.
fn nomad(env: &Environment) -> Reach<'_> {
    Reach {
        base: &env.nomad_api,
        credentials: Credentials {
            token_file: &env.nomad_token_file,
            ca_file: &env.nomad_ca_file,
            tls_server_name: &env.nomad_tls_server_name,
        },
        who: "Nomad",
    }
}

/// The deadline of a call to the board or the channel.
const CALL: Duration = Duration::from_secs(30);

/// The system's clock.
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}

/// The world over the module's host handle and channel.
pub struct HostWorld {
    host: Arc<dyn ModuleHost>,
    channel: GrpcChannel,
    stop: CancellationToken,
    clients: Clients,
}

impl HostWorld {
    /// The world over `host` and `channel`; `stop` cancels its calls.
    pub fn new(host: Arc<dyn ModuleHost>, channel: GrpcChannel, stop: CancellationToken) -> Self {
        Self {
            host,
            channel,
            stop,
            clients: Clients::default(),
        }
    }

    /// `method` `path` of `reach`, with its account: its answer when the
    /// status was a success.
    async fn call(
        &self,
        reach: Reach<'_>,
        method: Method,
        path: &str,
        query: &[(&str, String)],
        body: Option<Body>,
    ) -> Result<Answer, ModuleError> {
        let sandbox = self.host.sandbox().await?;
        let url = format!("{}{path}", reach.base.trim().trim_end_matches('/'));
        let answer = Caller::new(sandbox.as_ref(), &self.clients)
            .call(reach.credentials, method, &url, query, body)
            .await?;
        if answer.ok {
            return Ok(answer);
        }
        let kind = match answer.status {
            404 => ModuleErrorKind::NotFound,
            401 | 403 => ModuleErrorKind::Denied,
            _ => ModuleErrorKind::FailedPrecondition,
        };
        let text = String::from_utf8_lossy(&answer.body);
        let head: String = text.chars().take(300).collect();
        Err(ModuleError::new(
            kind,
            format!("{} refused {path}: {head}", reach.who),
        ))
    }

    /// What Nomad answers at `path` with `query`, as `T`.
    async fn nomad_get<T: DeserializeOwned>(
        &self,
        env: &Environment,
        path: &str,
        query: &[(&str, String)],
    ) -> Result<T, ModuleError> {
        let answer = self
            .call(nomad(env), Method::GET, path, query, None)
            .await?;
        pcx_module_api::json::decode(&answer.body).map_err(|e| undecoded("Nomad", path, &e))
    }

    fn scope(&self, workspace: &str, author: &str) -> Scope {
        Scope {
            tenant: String::new(),
            profile: String::new(),
            session_id: None,
            workspace_id: Some(workspace.to_string()),
            author: Some(author.to_string()),
            caller_module: None,
            call_depth: 0,
            turn: None,
            reach: None,
            role: None,
            read_only: false,
            person: None,
            deadline: Instant::now() + CALL,
            cancel: self.stop.child_token(),
        }
    }

    fn request<T>(scope: &Scope, message: T) -> Result<Request<T>, ModuleError> {
        let mut request = Request::new(message);
        scope.to_metadata(request.metadata_mut())?;
        Ok(request)
    }
}

/// An answer of `who` to `path` that is not what it sends.
fn undecoded(who: &str, path: &str, error: &serde_json::Error) -> ModuleError {
    ModuleError::new(
        ModuleErrorKind::Unavailable,
        format!("{who}'s answer to {path} is not what it sends: {error}"),
    )
}

fn decode<T: DeserializeOwned>(path: &str, body: &[u8]) -> Result<T, ModuleError> {
    serde_json::from_slice(body).map_err(|e| undecoded("the cluster", path, &e))
}

/// `message` as a body of `media_type`.
fn encode<T: serde::Serialize>(media_type: &'static str, message: &T) -> Result<Body, ModuleError> {
    let bytes = serde_json::to_vec(message)
        .map_err(|e| ModuleError::new(ModuleErrorKind::Internal, e.to_string()))?;
    Ok(Body { media_type, bytes })
}

#[async_trait]
impl Cluster for HostWorld {
    async fn get(&self, env: &Environment, path: &str) -> Result<KubeThing, ModuleError> {
        let answer = self.call(kube(env), Method::GET, path, &[], None).await?;
        decode(path, &answer.body)
    }

    async fn list(&self, env: &Environment, path: &str) -> Result<ThingList, ModuleError> {
        let answer = self.call(kube(env), Method::GET, path, &[], None).await?;
        decode(path, &answer.body)
    }

    async fn patch(&self, env: &Environment, path: &str, patch: Patch) -> Result<(), ModuleError> {
        let body = match &patch {
            Patch::Deployment(p) => encode(MERGE_PATCH, p)?,
            Patch::Claim(p) => encode(MERGE_PATCH, p)?,
        };
        self.call(kube(env), Method::PATCH, path, &[], Some(body))
            .await?;
        Ok(())
    }

    async fn delete(&self, env: &Environment, path: &str) -> Result<(), ModuleError> {
        let background = [("propagationPolicy", "Background".to_string())];
        self.call(kube(env), Method::DELETE, path, &background, None)
            .await?;
        Ok(())
    }
}

#[async_trait]
impl Nomad for HostWorld {
    async fn job(&self, env: &Environment, name: &str) -> Result<NomadJob, ModuleError> {
        self.nomad_get(env, &paths::job(name), &[]).await
    }

    async fn versions(&self, env: &Environment, name: &str) -> Result<Vec<NomadJob>, ModuleError> {
        let kept: NomadVersions = self.nomad_get(env, &paths::versions(name), &[]).await?;
        Ok(kept.versions)
    }

    async fn deployment(
        &self,
        env: &Environment,
        name: &str,
    ) -> Result<Option<NomadDeployment>, ModuleError> {
        self.nomad_get(env, &paths::deployment(name), &[]).await
    }

    async fn launches(&self, env: &Environment, name: &str) -> Result<NomadChildren, ModuleError> {
        let summary: NomadSummary = self.nomad_get(env, &paths::summary(name), &[]).await?;
        Ok(summary.children.unwrap_or_default())
    }

    async fn allocations(
        &self,
        env: &Environment,
        start: &str,
    ) -> Result<Vec<NomadAlloc>, ModuleError> {
        let prefix = [("prefix", start.to_string())];
        self.nomad_get(env, paths::ALLOCATIONS, &prefix).await
    }

    async fn stop(&self, env: &Environment, id: &str) -> Result<(), ModuleError> {
        self.call(nomad(env), Method::POST, &paths::stop(id), &[], None)
            .await?;
        Ok(())
    }

    async fn launch(&self, env: &Environment, name: &str) -> Result<(), ModuleError> {
        self.call(nomad(env), Method::POST, &paths::launch(name), &[], None)
            .await?;
        Ok(())
    }

    async fn revert(&self, env: &Environment, revert: &Revert) -> Result<(), ModuleError> {
        let body = encode(
            JSON,
            &NomadRevert {
                job_id: revert.job.clone(),
                job_version: revert.to,
                enforce_prior_version: revert.from,
            },
        )?;
        self.call(
            nomad(env),
            Method::POST,
            &paths::revert(&revert.job),
            &[],
            Some(body),
        )
        .await?;
        Ok(())
    }
}

#[async_trait]
impl Board for HostWorld {
    async fn decide(
        &self,
        author: &str,
        decision: Decision,
    ) -> Result<DecisionRecord, ModuleError> {
        let scope = self.scope(&decision.workspace, author);
        let decided = BoardsClient::new(self.channel.clone())
            .decide(Self::request(
                &scope,
                DecideRequest {
                    workspace_id: decision.workspace.clone(),
                    task: decision.task,
                    what: decision.what,
                    why: decision.why,
                    risk: Risk::Operations.into(),
                    stakes: Some(decision.stakes),
                },
            )?)
            .await?
            .into_inner();
        Ok(decided)
    }

    async fn decision(
        &self,
        workspace: &str,
        task: &str,
        id: &str,
    ) -> Result<Option<DecisionRecord>, ModuleError> {
        let scope = self.scope(workspace, MODULE_ID);
        let thread = BoardsClient::new(self.channel.clone())
            .get_task(Self::request(
                &scope,
                GetTaskRequest {
                    workspace_id: workspace.to_string(),
                    task: task.to_string(),
                },
            )?)
            .await?
            .into_inner();
        Ok(thread.decisions.into_iter().find(|d| d.id == id))
    }

    async fn withdraw(
        &self,
        author: &str,
        workspace: &str,
        id: &str,
        why: &str,
    ) -> Result<(), ModuleError> {
        let scope = self.scope(workspace, author);
        BoardsClient::new(self.channel.clone())
            .withdraw_decision(Self::request(
                &scope,
                WithdrawDecisionRequest {
                    workspace_id: workspace.to_string(),
                    decision: id.to_string(),
                    why: why.to_string(),
                },
            )?)
            .await?;
        Ok(())
    }

    async fn comment(&self, workspace: &str, task: &str, text: &str) -> Result<(), ModuleError> {
        let scope = self.scope(workspace, MODULE_ID);
        BoardsClient::new(self.channel.clone())
            .add_comment(Self::request(
                &scope,
                AddCommentRequest {
                    workspace_id: workspace.to_string(),
                    task: task.to_string(),
                    text: text.to_string(),
                    ..AddCommentRequest::default()
                },
            )?)
            .await?;
        Ok(())
    }
}

#[async_trait]
impl Teller for HostWorld {
    async fn tell(&self, destination: &str, text: &str) -> Result<(), ModuleError> {
        let scope = self.scope("", MODULE_ID);
        ChannelClient::new(self.channel.clone())
            .send(channel::send(&scope, destination, text)?)
            .await?;
        Ok(())
    }
}
