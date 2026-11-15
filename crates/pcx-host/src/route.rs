// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Calls to the gRPC services of modules (ARCHITECTURE section 7,
//! everything gRPC): a call's path names its service, and the module whose
//! manifest names that service serves it; where several do, the call names
//! the one it goes to. A module's call is checked as
//! W14 and W22a check a tool call (the modules it uses, the RPC's
//! visibility, its turn's reach) and named with its caller one deeper; a
//! client's call names no caller; an agent client's (MCP) is refused an RPC
//! only a person calls. The call's frames are forwarded, never decoded.

use std::convert::Infallible;
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

use arc_swap::ArcSwap;
use pcx_module_api::grpc::{GrpcChannel, channel, refusal};
use pcx_module_api::{
    DEFAULT_DEADLINE, Effect, MAX_CALL_DEPTH, ModuleError, ModuleErrorKind, agent_may_call,
    effect_from_wire, metadata, narrower,
};
use tokio_util::sync::CancellationToken;
use tonic::body::Body;
use tonic::codegen::http::{HeaderMap, HeaderValue, Request, Response};
use tonic::{Code, Status};
use tower::ServiceExt;

use crate::Loaded;
use crate::delegation::{Delegations, reach, reaches};
use crate::policy::Guarded;

/// Who makes a call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Origin {
    /// A running module, by id.
    Module(String),
    /// A client of the host acting for a person (the app).
    Client,
    /// A client of the host that is an agent (MCP).
    Agent,
}

/// The routing of calls of one origin.
#[derive(Clone)]
pub struct ServiceRoute {
    origin: Origin,
    loaded: Weak<ArcSwap<Loaded>>,
    delegations: Arc<Delegations>,
    /// The organisation the host serves (its slug); empty: none.
    tenant: Arc<str>,
}

impl ServiceRoute {
    /// The calls of `origin` to the running set `loaded`, whose modules'
    /// running turns are `delegations`, on the host of `tenant` (empty:
    /// of none).
    pub fn new(
        origin: Origin,
        loaded: Weak<ArcSwap<Loaded>>,
        delegations: Arc<Delegations>,
        tenant: Arc<str>,
    ) -> Self {
        Self {
            origin,
            loaded,
            delegations,
            tenant,
        }
    }

    /// The routing as a channel: a generated client's, or what a socket
    /// serves.
    pub fn into_channel(self) -> GrpcChannel {
        channel(tower::service_fn(move |request: Request<Body>| {
            let route = self.clone();
            async move { Ok::<_, Infallible>(route.call(request).await) }
        }))
    }

    /// Answer `request`: its module's answer, or why it is refused. The
    /// call runs under the module's policy (T4) as a tool call does: its
    /// deadline, a slot, its panic caught, the breaker, counted in its
    /// health; an answer whose status is not OK is a failed call, forwarded
    /// as it came.
    async fn call(&self, mut request: Request<Body>) -> Response<Body> {
        let callee = match self.target(&mut request) {
            Ok(callee) => callee,
            Err(e) => return refusal(&Status::from(e)),
        };
        let deadline = Instant::now() + deadline(request.headers());
        // A client's call ends when it is dropped: nothing else cancels it.
        let answered = callee
            .run_judged(
                &CancellationToken::new(),
                deadline,
                "rpc",
                |module| {
                    Box::pin(async move {
                        let services = module.services().ok_or_else(|| {
                            ModuleError::new(
                                ModuleErrorKind::Unavailable,
                                format!("module {} serves no gRPC services", module.manifest().id),
                            )
                        })?;
                        services.oneshot(request).await.map_err(ModuleError::from)
                    })
                },
                |answer| {
                    Status::from_header_map(answer.headers())
                        .filter(|status| status.code() != Code::Ok)
                        .map(ModuleError::from)
                },
            )
            .await;
        match answered {
            Ok(answer) => answer,
            Err(e) => refusal(&Status::from(e)),
        }
    }

    /// The module that serves `request`, once its origin may call it; the
    /// request's caller, depth and turn as the host sets them, and its
    /// tenant the host's organisation when the host serves one.
    fn target(&self, request: &mut Request<Body>) -> Result<Arc<Guarded>, ModuleError> {
        let path = request.uri().path().to_string();
        let service = service_of(&path)?;
        let loaded = self
            .loaded
            .upgrade()
            .ok_or_else(|| ModuleError::new(ModuleErrorKind::Unavailable, "the host has stopped"))?
            .load_full();
        let named = text(request.headers(), metadata::TARGET)?;
        let index = serving(&loaded.modules, service, named.as_deref())?;
        let callee = &loaded.modules[index];
        let rpc = callee
            .module()
            .manifest()
            .rpcs
            .iter()
            .find(|rpc| rpc.method == path)
            .ok_or_else(|| {
                ModuleError::new(
                    ModuleErrorKind::NotFound,
                    format!("module {} declares no RPC {path}", callee.id()),
                )
            })?;
        let for_agents = agent_may_call(&rpc.visibility, &rpc.tool)?;
        // A caller that may only read calls only what reads (docs/host.md,
        // effects); every call made in its scope carries the mark.
        if request.headers().contains_key(metadata::READ_ONLY)
            && effect_from_wire(rpc.effect, &path)? != Effect::Read
        {
            return Err(ModuleError::new(
                ModuleErrorKind::Denied,
                format!("{path} changes things and the caller is read-only"),
            ));
        }
        // A client's call from afar carries the reach its forwarder (a
        // gateway, a tunnel) holds it to: a module only this machine's
        // people and agents call refuses it (docs/connect.md section 4).
        let from_afar = request.headers().contains_key(metadata::REACH);
        let local_only = loaded
            .configured
            .get(index)
            .is_some_and(|c| c.policy.local_only);
        if from_afar && local_only && !matches!(self.origin, Origin::Module(_)) {
            return Err(ModuleError::new(
                ModuleErrorKind::Denied,
                format!(
                    "{path}: module {} serves only this machine's people and agents, not a call from the cloud",
                    callee.id()
                ),
            ));
        }
        let headers = request.headers_mut();
        let turn = text(headers, metadata::TURN)?;
        let depth = count(headers, metadata::DEPTH)?;
        for key in metadata::HOST {
            headers.remove(key);
        }
        // The host of an organisation names it, whatever the call carried;
        // a host of none passes on the tenant of the call it serves (a
        // runner's calls come from its organisation's host, docs/host.md).
        if !self.tenant.is_empty() {
            headers.insert(metadata::TENANT, header(&self.tenant)?);
        }
        // A person's role is a client's call's only: what a module or an
        // agent does for a person never acts with it (docs/host.md).
        if self.origin != Origin::Client {
            headers.remove(metadata::ROLE);
        }
        match &self.origin {
            Origin::Client => {}
            Origin::Agent if for_agents => {}
            Origin::Agent => return Err(crate::only_for_a_person(&rpc.tool)),
            Origin::Module(caller) => {
                let module = loaded
                    .modules
                    .iter()
                    .find(|g| g.id() == caller)
                    .ok_or_else(|| {
                        ModuleError::new(
                            ModuleErrorKind::FailedPrecondition,
                            format!("module {caller} is not running"),
                        )
                    })?;
                if !reaches(&module.module().manifest().uses, &loaded.added, callee.id()) {
                    return Err(ModuleError::new(
                        ModuleErrorKind::Denied,
                        format!(
                            "module {caller} does not use module {}, which serves {service}",
                            callee.id()
                        ),
                    ));
                }
                let depth = depth + 1;
                if depth > MAX_CALL_DEPTH {
                    return Err(ModuleError::new(
                        ModuleErrorKind::FailedPrecondition,
                        format!(
                            "module {caller} calls {path} {depth} calls deep; at most {MAX_CALL_DEPTH}"
                        ),
                    ));
                }
                let author = text(headers, metadata::AUTHOR)?;
                let reach = reach(
                    &loaded,
                    &self.delegations,
                    module,
                    turn.as_deref(),
                    author.as_deref(),
                )?;
                reach.admit(callee.id(), &rpc.tool, for_agents)?;
                // The agent's call in a turn reaches no more workspaces
                // than the turn was asked to, whatever it carried
                // (docs/connect.md section 4a, a member's reach).
                if let Some(held) = reach.workspaces() {
                    let carried = reach_of(headers)?;
                    let both = narrower(carried.as_deref(), held);
                    headers.insert(metadata::REACH, header(&metadata::reach_value(&both))?);
                }
                headers.insert(metadata::CALLER, header(caller)?);
                headers.insert(metadata::DEPTH, HeaderValue::from(depth));
                if let Some(turn) = &turn {
                    headers.insert(metadata::TURN, header(turn)?);
                }
            }
        }
        Ok(Arc::clone(callee))
    }
}

impl crate::Host {
    /// The channel of `origin`, a client of the host (the app, an agent),
    /// to the running modules' gRPC services: what a transport serves.
    #[must_use]
    pub fn channel(&self, origin: Origin) -> GrpcChannel {
        ServiceRoute::new(
            origin,
            Arc::downgrade(&self.loaded),
            Arc::clone(&self.delegations),
            Arc::clone(&self.tenant),
        )
        .into_channel()
    }
}

/// The module of `modules` a call to `service` goes to: the one `named`
/// (the call's target), which must serve it; without a name, the one
/// module that serves it (docs/host.md, a service several modules serve).
fn serving(
    modules: &[Arc<Guarded>],
    service: &str,
    named: Option<&str>,
) -> Result<usize, ModuleError> {
    let mut serve = modules
        .iter()
        .enumerate()
        .filter(|(_, g)| g.module().manifest().services.iter().any(|s| s == service));
    if let Some(named) = named {
        return serve
            .find(|(_, g)| g.id() == named)
            .map(|(index, _)| index)
            .ok_or_else(|| {
                ModuleError::new(
                    ModuleErrorKind::NotFound,
                    format!("no running module {named} serves {service}"),
                )
            });
    }
    let (first, _) = serve.next().ok_or_else(|| {
        ModuleError::new(
            ModuleErrorKind::NotFound,
            format!("no running module serves {service}"),
        )
    })?;
    let others: Vec<&str> = serve.map(|(_, g)| g.id()).collect();
    if others.is_empty() {
        return Ok(first);
    }
    Err(ModuleError::new(
        ModuleErrorKind::FailedPrecondition,
        format!(
            "{service} is served by {}, {}: the call names the one it goes to ({})",
            modules[first].id(),
            others.join(", "),
            metadata::TARGET
        ),
    ))
}

/// The service a gRPC path names: `/pcx.chief.v1.Chief/ListTalks` names
/// `pcx.chief.v1.Chief`.
fn service_of(path: &str) -> Result<&str, ModuleError> {
    path.strip_prefix('/')
        .and_then(|rest| rest.split_once('/'))
        .map(|(service, _)| service)
        .filter(|service| !service.is_empty())
        .ok_or_else(|| {
            ModuleError::new(
                ModuleErrorKind::InvalidArgument,
                format!("{path} is not a gRPC call's path"),
            )
        })
}

/// Header `key` of `headers` as text; none when it is not set.
fn text(headers: &HeaderMap, key: &str) -> Result<Option<String>, ModuleError> {
    headers
        .get(key)
        .map(|value| {
            value.to_str().map(str::to_string).map_err(|_| {
                ModuleError::new(
                    ModuleErrorKind::InvalidArgument,
                    format!("the call's {key} is not text"),
                )
            })
        })
        .transpose()
}

/// The workspaces the call's reach names; none when it carries none (an
/// empty one names no workspace, as [`pcx_module_api::Scope`] reads it).
fn reach_of(headers: &HeaderMap) -> Result<Option<Vec<String>>, ModuleError> {
    Ok(text(headers, metadata::REACH)?.map(|value| metadata::reach_of(&value)))
}

/// Header `key` of `headers` as a count; 0 when it is not set.
fn count(headers: &HeaderMap, key: &str) -> Result<u32, ModuleError> {
    text(headers, key)?.map_or(Ok(0), |value| {
        value.parse().map_err(|_| {
            ModuleError::new(
                ModuleErrorKind::InvalidArgument,
                format!("the call's {key} is not a count"),
            )
        })
    })
}

/// `value` as a header value.
fn header(value: &str) -> Result<HeaderValue, ModuleError> {
    HeaderValue::from_str(value).map_err(|_| {
        ModuleError::new(
            ModuleErrorKind::InvalidArgument,
            format!("{value} is not a header value"),
        )
    })
}

/// How long a call may run: its deadline's milliseconds, else the default.
fn deadline(headers: &HeaderMap) -> Duration {
    match count(headers, metadata::DEADLINE_MS) {
        Ok(0) | Err(_) => DEFAULT_DEADLINE,
        Ok(ms) => Duration::from_millis(u64::from(ms)),
    }
}

#[cfg(test)]
mod tests;
