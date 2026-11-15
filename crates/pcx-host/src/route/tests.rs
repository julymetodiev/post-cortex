// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Calls to a module's gRPC services through the host (W23): a module that
//! uses another calls it with a generated client over its channel, named as
//! the caller one deeper whatever it claims; one that does not is refused;
//! a client's call names no caller, an agent client's is refused an RPC
//! only a person calls; a person's role reaches a module from a client
//! only; every call is counted in the module's health, a failed answer
//! forwarded as it came.

use std::collections::HashMap;

use async_trait::async_trait;
use parking_lot::Mutex;
use pcx_module_api::{
    API_VERSION, Event, HostContext, Manifest, Module, ModuleKind, Outcome, Resource, Scope, grpc,
};
use post_cortex_proto::module as wire;
use post_cortex_proto::module::hooks_client::HooksClient;
use post_cortex_proto::module::hooks_server::{Hooks, HooksServer};
use tonic::Code;

use super::*;
use crate::Host;

/// The RPC the echo module serves.
const DISPATCH: &str = "/pcx.module.v1.Hooks/Dispatch";

/// Answers who called it and how deep, from the call's metadata; to an
/// event named `role`, the person's role it carries; to one named
/// `tenant`, its tenant; to one named `who`, its own module's id.
struct Echo(String);

#[tonic::async_trait]
impl Hooks for Echo {
    async fn dispatch(
        &self,
        request: tonic::Request<wire::DispatchRequest>,
    ) -> Result<tonic::Response<wire::DispatchResponse>, tonic::Status> {
        // An event named `fail` fails it; `refuse` refuses who calls.
        match request
            .get_ref()
            .event
            .as_ref()
            .map(|e| e.hook_event_name.as_str())
        {
            Some("fail") => return Err(tonic::Status::internal("echo broke")),
            Some("refuse") => return Err(tonic::Status::unauthenticated("echo does not know you")),
            _ => {}
        }
        let name = request
            .get_ref()
            .event
            .as_ref()
            .map(|e| e.hook_event_name.clone())
            .unwrap_or_default();
        let scope = Scope::of(&request)?;
        let seen = match name.as_str() {
            "role" => Some(
                scope
                    .role
                    .map(|r| r.as_str_name())
                    .unwrap_or_default()
                    .to_string(),
            ),
            "tenant" => Some(scope.tenant.clone()),
            "who" => Some(self.0.clone()),
            _ => None,
        };
        if let Some(seen) = seen {
            return Ok(tonic::Response::new(wire::DispatchResponse {
                outcome: Some(Outcome {
                    decision: seen,
                    ..Outcome::default()
                }),
                error: None,
            }));
        }
        Ok(tonic::Response::new(wire::DispatchResponse {
            outcome: Some(Outcome {
                decision: scope.caller_module.unwrap_or_default(),
                reason: scope.call_depth.to_string(),
                hook_specific_output: None,
            }),
            error: None,
        }))
    }
}

/// A module of these tests: one whose id starts with `echo` serves
/// [`Echo`]; any other keeps its channel to call it.
struct Test {
    manifest: Manifest,
    channel: Mutex<Option<GrpcChannel>>,
}

fn module(id: &str, uses: &[&str], visibility: Vec<i32>, effect: Effect) -> Arc<Test> {
    let serves = id.starts_with("echo");
    Arc::new(Test {
        manifest: Manifest {
            id: id.to_string(),
            version: "0.1.0".to_string(),
            api: Some(API_VERSION),
            kind: ModuleKind::Tool.into(),
            uses: uses.iter().map(|u| (*u).to_string()).collect(),
            services: if serves {
                vec!["pcx.module.v1.Hooks".to_string()]
            } else {
                Vec::new()
            },
            rpcs: if serves {
                vec![wire::Rpc {
                    method: DISPATCH.to_string(),
                    // No tool: a module's and a person's RPC, no agent's.
                    tool: String::new(),
                    visibility,
                    effect: effect.into(),
                }]
            } else {
                Vec::new()
            },
            ..Manifest::default()
        },
        channel: Mutex::new(None),
    })
}

#[async_trait]
impl Module for Test {
    fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    async fn start(&self, ctx: HostContext) -> Result<(), ModuleError> {
        *self.channel.lock() = Some(ctx.channel);
        Ok(())
    }

    async fn on_event(&self, _: &Scope, _: &Event) -> Result<Outcome, ModuleError> {
        Ok(Outcome::default())
    }

    async fn resource(&self, uri: &str) -> Result<Resource, ModuleError> {
        Err(ModuleError::new(ModuleErrorKind::NotFound, uri))
    }

    async fn stop(&self) -> Result<(), ModuleError> {
        Ok(())
    }

    fn services(&self) -> Option<GrpcChannel> {
        self.manifest.id.starts_with("echo").then(|| {
            grpc::routes(tonic::service::Routes::new(HooksServer::new(Echo(
                self.manifest.id.clone(),
            ))))
        })
    }
}

/// Dispatch through `channel`, its metadata claiming `caller` and `depth`;
/// who answered it named as the caller, and how deep.
async fn dispatch(
    channel: GrpcChannel,
    claimed: Option<(&str, &str)>,
) -> Result<(String, String), tonic::Status> {
    let mut request = tonic::Request::new(wire::DispatchRequest::default());
    if let Some((caller, depth)) = claimed {
        let metadata = request.metadata_mut();
        metadata.insert(metadata::CALLER, caller.parse().unwrap());
        metadata.insert(metadata::DEPTH, depth.parse().unwrap());
    }
    let outcome = HooksClient::new(channel)
        .dispatch(request)
        .await?
        .into_inner()
        .outcome
        .unwrap();
    Ok((outcome.decision, outcome.reason))
}

/// The role the echo module sees of a call through `channel` that claims
/// the person's role is the owner's.
async fn role_seen(channel: GrpcChannel) -> Result<String, tonic::Status> {
    let mut request = tonic::Request::new(wire::DispatchRequest {
        event: Some(wire::Event {
            hook_event_name: "role".to_string(),
            ..wire::Event::default()
        }),
        ..wire::DispatchRequest::default()
    });
    request
        .metadata_mut()
        .insert(metadata::ROLE, "owner".parse().unwrap());
    let outcome = HooksClient::new(channel)
        .dispatch(request)
        .await?
        .into_inner()
        .outcome
        .unwrap();
    Ok(outcome.decision)
}

/// The tenant the echo module sees of a call through `channel` that
/// claims the tenant `claimed`.
async fn tenant_seen(channel: GrpcChannel, claimed: &str) -> String {
    let mut request = tonic::Request::new(wire::DispatchRequest {
        event: Some(wire::Event {
            hook_event_name: "tenant".to_string(),
            ..wire::Event::default()
        }),
        ..wire::DispatchRequest::default()
    });
    request
        .metadata_mut()
        .insert(metadata::TENANT, claimed.parse().unwrap());
    HooksClient::new(channel)
        .dispatch(request)
        .await
        .unwrap()
        .into_inner()
        .outcome
        .unwrap()
        .decision
}

async fn host(visibility: Vec<i32>) -> (Host, Arc<Test>, Arc<Test>) {
    host_of(visibility, Effect::Read).await
}

/// The echo module serving its RPC of `visibility` and `effect`, a module
/// that uses it, and one that does not.
async fn host_of(visibility: Vec<i32>, effect: Effect) -> (Host, Arc<Test>, Arc<Test>) {
    let user = module("user", &["echo"], Vec::new(), Effect::Read);
    let stranger = module("stranger", &[], Vec::new(), Effect::Read);
    let modules: Vec<Arc<dyn Module>> = vec![
        module("echo", &[], visibility, effect),
        user.clone(),
        stranger.clone(),
    ];
    let host = Host::start(modules, &HashMap::new(), &HashMap::new())
        .await
        .unwrap();
    (host, user, stranger)
}

fn channel_of(module: &Test) -> GrpcChannel {
    module.channel.lock().clone().unwrap()
}

#[tokio::test]
async fn a_module_calls_one_it_uses_as_itself_one_deeper() {
    let (_host, user, stranger) = host(Vec::new()).await;
    let answered = dispatch(channel_of(&user), Some(("echo", "3")))
        .await
        .unwrap();
    assert_eq!(answered, ("user".to_string(), "4".to_string()));
    let answered = dispatch(channel_of(&user), None).await.unwrap();
    assert_eq!(answered, ("user".to_string(), "1".to_string()));

    let refused = dispatch(channel_of(&stranger), None).await.unwrap_err();
    assert_eq!(refused.code(), Code::PermissionDenied, "{refused:?}");
    let deep = dispatch(channel_of(&user), Some(("user", "4")))
        .await
        .unwrap_err();
    assert_eq!(deep.code(), Code::FailedPrecondition, "{deep:?}");
}

#[tokio::test]
async fn a_client_names_no_caller_and_an_agent_is_refused_a_persons_rpc() {
    let (host, _, _) = host(vec![wire::ToolVisibility::App.into()]).await;
    let answered = dispatch(host.channel(Origin::Client), Some(("user", "2")))
        .await
        .unwrap();
    assert_eq!(answered, (String::new(), "0".to_string()));
    let refused = dispatch(host.channel(Origin::Agent), None)
        .await
        .unwrap_err();
    assert_eq!(refused.code(), Code::PermissionDenied, "{refused:?}");
}

#[tokio::test]
async fn a_persons_role_reaches_a_module_from_a_client_only() {
    let (host, user, _) = host(Vec::new()).await;
    assert_eq!(
        role_seen(host.channel(Origin::Client)).await.unwrap(),
        "PERSON_ROLE_OWNER"
    );
    assert_eq!(role_seen(host.channel(Origin::Agent)).await.unwrap(), "");
    assert_eq!(role_seen(channel_of(&user)).await.unwrap(), "");
}

/// The host of an organisation names it on every call it routes, whatever
/// the call claimed; a host of none passes the call's on (a runner's calls
/// come from its organisation's host).
#[tokio::test]
async fn the_host_of_an_organisation_names_it_on_every_call() {
    let user = module("user", &["echo"], Vec::new(), Effect::Read);
    let modules: Vec<Arc<dyn Module>> =
        vec![module("echo", &[], Vec::new(), Effect::Read), user.clone()];
    let acme = Host::start_with(modules, &HashMap::new(), &HashMap::new(), None, "acme")
        .await
        .unwrap();
    for channel in [
        acme.channel(Origin::Client),
        acme.channel(Origin::Agent),
        channel_of(&user),
    ] {
        assert_eq!(tenant_seen(channel, "forged").await, "acme");
    }
    let (none, _, _) = host(Vec::new()).await;
    assert_eq!(
        tenant_seen(none.channel(Origin::Client), "acme").await,
        "acme"
    );
}

#[tokio::test]
async fn a_call_no_module_serves_is_not_found() {
    let (host, _, _) = host(Vec::new()).await;
    let mut client =
        post_cortex_proto::module::host_client::HostClient::new(host.channel(Origin::Client));
    let missing = client
        .list_modules(wire::ListModulesRequest {})
        .await
        .unwrap_err();
    assert_eq!(missing.code(), Code::NotFound, "{missing:?}");
}

/// The module that answers a call through `channel` that names `target`.
async fn who_answers(channel: GrpcChannel, target: Option<&str>) -> Result<String, tonic::Status> {
    let mut request = tonic::Request::new(wire::DispatchRequest {
        event: Some(wire::Event {
            hook_event_name: "who".to_string(),
            ..wire::Event::default()
        }),
        ..wire::DispatchRequest::default()
    });
    if let Some(target) = target {
        request
            .metadata_mut()
            .insert(metadata::TARGET, target.parse().unwrap());
    }
    let outcome = HooksClient::new(channel)
        .dispatch(request)
        .await?
        .into_inner()
        .outcome
        .unwrap();
    Ok(outcome.decision)
}

/// Two modules serve one service (docs/host.md, a service several modules
/// serve): a call names the one it goes to, which is checked as any
/// callee; a call that names none is refused, naming them.
#[tokio::test]
async fn a_call_to_a_service_two_modules_serve_names_its_target() {
    let user = module("user", &["echo"], Vec::new(), Effect::Read);
    let modules: Vec<Arc<dyn Module>> = vec![
        module("echo", &[], Vec::new(), Effect::Read),
        module("echo2", &[], Vec::new(), Effect::Read),
        user.clone(),
    ];
    let host = Host::start(modules, &HashMap::new(), &HashMap::new())
        .await
        .unwrap();
    let client = || host.channel(Origin::Client);
    assert_eq!(who_answers(client(), Some("echo")).await.unwrap(), "echo");
    assert_eq!(who_answers(client(), Some("echo2")).await.unwrap(), "echo2");

    let unnamed = who_answers(client(), None).await.unwrap_err();
    assert_eq!(unnamed.code(), Code::FailedPrecondition, "{unnamed:?}");
    assert!(
        unnamed.message().contains("echo, echo2"),
        "{}",
        unnamed.message()
    );
    // A module that does not serve the service, or does not run.
    for other in ["user", "nobody"] {
        let missing = who_answers(client(), Some(other)).await.unwrap_err();
        assert_eq!(missing.code(), Code::NotFound, "{missing:?}");
    }

    // The target is a callee as any other: `user` uses `echo` only.
    assert_eq!(
        who_answers(channel_of(&user), Some("echo")).await.unwrap(),
        "echo"
    );
    let refused = who_answers(channel_of(&user), Some("echo2"))
        .await
        .unwrap_err();
    assert_eq!(refused.code(), Code::PermissionDenied, "{refused:?}");
}

/// One module serves the service: a call needs no target, and one it
/// names is that module or none.
#[tokio::test]
async fn a_call_to_a_service_one_module_serves_needs_no_target() {
    let (host, _, _) = host(Vec::new()).await;
    let client = || host.channel(Origin::Client);
    assert_eq!(who_answers(client(), None).await.unwrap(), "echo");
    assert_eq!(who_answers(client(), Some("echo")).await.unwrap(), "echo");
    let missing = who_answers(client(), Some("user")).await.unwrap_err();
    assert_eq!(missing.code(), Code::NotFound, "{missing:?}");
}

#[tokio::test]
async fn a_routed_call_counts_in_its_modules_health_and_its_answer_comes_as_it_was() {
    let (host, _, _) = host(Vec::new()).await;
    let named = |name: &str| wire::DispatchRequest {
        event: Some(Event {
            hook_event_name: name.to_string(),
            ..Event::default()
        }),
        ..wire::DispatchRequest::default()
    };
    let mut client = HooksClient::new(host.channel(Origin::Client));
    client.dispatch(named("")).await.unwrap();
    let failed = client.dispatch(named("fail")).await.unwrap_err();
    assert_eq!(failed.code(), Code::Internal, "{failed:?}");
    let refused = client.dispatch(named("refuse")).await.unwrap_err();
    assert_eq!(
        refused.code(),
        Code::Unauthenticated,
        "the code as the module sent it"
    );
    assert_eq!(refused.message(), "echo does not know you");

    let echo = host
        .health()
        .into_iter()
        .find(|h| h.module == "echo")
        .unwrap();
    assert_eq!(echo.calls, 3);
    assert_eq!(echo.errors, 2, "a failed and a refused answer");
    let log = host.module_log("echo").unwrap();
    assert!(
        log.iter()
            .any(|l| l.text.contains("rpc failed: echo broke")),
        "{log:?}"
    );
}

/// Dispatch through `channel` as a caller that may only read.
async fn dispatch_read_only(channel: GrpcChannel) -> Result<(String, String), tonic::Status> {
    let mut request = tonic::Request::new(wire::DispatchRequest::default());
    request.metadata_mut().insert(
        metadata::READ_ONLY,
        metadata::READ_ONLY_VALUE.parse().unwrap(),
    );
    let outcome = HooksClient::new(channel)
        .dispatch(request)
        .await?
        .into_inner()
        .outcome
        .unwrap();
    Ok((outcome.decision, outcome.reason))
}

#[tokio::test]
async fn a_caller_that_may_only_read_calls_only_what_reads() {
    let (reads, user, _) = host_of(Vec::new(), Effect::Read).await;
    assert!(
        dispatch_read_only(reads.channel(Origin::Client))
            .await
            .is_ok()
    );
    assert!(dispatch_read_only(channel_of(&user)).await.is_ok());

    let (writes, user, _) = host_of(Vec::new(), Effect::Write).await;
    for channel in [
        writes.channel(Origin::Client),
        writes.channel(Origin::Agent),
        channel_of(&user),
    ] {
        let refused = dispatch_read_only(channel).await.unwrap_err();
        assert_eq!(refused.code(), Code::PermissionDenied, "{refused:?}");
    }
    assert!(dispatch(writes.channel(Origin::Client), None).await.is_ok());
}
