// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use super::*;

use std::collections::HashMap;

use async_trait::async_trait;
use pcx_module_api::view::{Body, HTML_MIME, View};
use pcx_module_api::{
    API_VERSION, Capability, Event, HostContext, Manifest, Module, ModuleError, ModuleErrorKind,
    ModuleKind, Outcome, Resource, ToolOutput, ToolSpec, Visibility,
};
use serde_json::{Value, json};

/// A module whose one tool says who called it.
struct Whoami(Manifest);

#[async_trait]
impl Module for Whoami {
    fn manifest(&self) -> &Manifest {
        &self.0
    }
    async fn start(&self, _ctx: HostContext) -> Result<(), ModuleError> {
        Ok(())
    }
    fn mcp_tools(&self) -> Vec<ToolSpec> {
        vec![ToolSpec {
            name: "whoami".into(),
            description: "Who calls".into(),
            input_schema: serde_json::Map::new(),
            visibility: Visibility::Both,
            effect: pcx_module_api::Effect::Read,
        }]
    }
    async fn call_mcp(
        &self,
        scope: &Scope,
        _tool: &str,
        input: Value,
    ) -> Result<ToolOutput, ModuleError> {
        Ok(ToolOutput {
            text: scope.author.clone().unwrap_or_default(),
            data: Some(input),
            failed: false,
        })
    }
    async fn on_event(&self, _scope: &Scope, _event: &Event) -> Result<Outcome, ModuleError> {
        Err(ModuleError::new(ModuleErrorKind::Internal, "no events"))
    }
    async fn resource(&self, uri: &str) -> Result<Resource, ModuleError> {
        Ok(Resource {
            uri: uri.to_string(),
            mime_type: HTML_MIME.to_string(),
            text: "<p>who</p>".to_string(),
        })
    }
    async fn stop(&self) -> Result<(), ModuleError> {
        Ok(())
    }
}

/// The message of the module's configuration: Telegram's, a secret token.
const CONFIG: &str = "pcx.telegram.v1.TelegramConfig";

async fn host() -> Arc<Host> {
    let module = Whoami(Manifest {
        id: "who".into(),
        version: "1".into(),
        api: Some(API_VERSION),
        kind: ModuleKind::Tool.into(),
        descriptors: post_cortex_proto::TELEGRAM_DESCRIPTOR_SET.to_vec(),
        config_type: CONFIG.to_string(),
        capabilities: vec![Capability::UiHtml.into()],
        views: vec![View {
            id: "card".into(),
            title: "Who".into(),
            description: String::new(),
            body: Some(Body::ResourceUri("ui://who/card.html".into())),
        }],
        ..Manifest::default()
    });
    let config = HashMap::from([("who".to_string(), json!({ "bot_token": "t" }))]);
    Arc::new(
        Host::start(vec![Arc::new(module)], &config, &HashMap::new())
            .await
            .unwrap(),
    )
}

async fn service() -> HostApi {
    HostApi::new(host().await, None)
}

/// A settings port that keeps what it is given.
#[derive(Default)]
struct Kept(parking_lot::Mutex<Vec<(String, Value)>>);

#[async_trait]
impl Settings for Kept {
    async fn apply(&self, id: &str, config: Value) -> Result<(), pcx_host::SettingsError> {
        self.0.lock().push((id.to_string(), config));
        Ok(())
    }

    async fn grant(
        &self,
        id: &str,
        _grants: pcx_host::ModuleGrants,
    ) -> Result<(), pcx_host::SettingsError> {
        Err(pcx_host::SettingsError::Unknown {
            module: id.to_string(),
        })
    }

    fn available(&self) -> Vec<String> {
        Vec::new()
    }
}

#[tokio::test]
async fn modules_are_listed_without_their_secrets_or_descriptors_and_one_comes_whole() {
    let listed = service()
        .await
        .list_modules(Request::new(wire::ListModulesRequest {}))
        .await
        .unwrap()
        .into_inner();
    let who = &listed.modules[0];
    assert_eq!(who.manifest.as_ref().unwrap().id, "who");
    let config = who.config.clone().unwrap();
    assert!(config.type_url.ends_with(CONFIG));
    assert!(config.value.is_empty(), "its only setting is a secret");
    assert_eq!(who.secrets_set, ["bot_token"]);
    assert_eq!(who.manifest.as_ref().unwrap().config_type, CONFIG);
    assert!(
        who.manifest.as_ref().unwrap().descriptors.is_empty(),
        "a listing leaves out the descriptors"
    );

    // One module whole: its descriptors, to read its configuration.
    let whole = service()
        .await
        .get_module(Request::new(wire::GetModuleRequest { id: "who".into() }))
        .await
        .unwrap()
        .into_inner()
        .module
        .unwrap();
    assert!(!whole.manifest.as_ref().unwrap().descriptors.is_empty());
    assert_eq!(whole.secrets_set, ["bot_token"]);
    let missing = service()
        .await
        .get_module(Request::new(wire::GetModuleRequest {
            id: "nobody".into(),
        }))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(missing.module, None, "no such module runs");
}

#[tokio::test]
async fn a_configuration_change_goes_to_the_port_or_is_refused_in_the_answer() {
    let kept = Arc::new(Kept::default());
    let kept_by = HostApi::new(host().await, Some(kept.clone()));
    let empty = pbjson_types::Any {
        type_url: format!("type.googleapis.com/{CONFIG}"),
        value: Default::default(),
    };
    let configure = |id: &str, config: Option<pbjson_types::Any>, clear: &[&str]| {
        Request::new(wire::ConfigureModuleRequest {
            id: id.into(),
            config,
            clear: clear.iter().map(|c| (*c).to_string()).collect(),
        })
    };
    let done = kept_by
        .configure_module(configure("who", Some(empty.clone()), &[]))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(done.error, None);
    assert_eq!(
        *kept.0.lock(),
        [("who".to_string(), json!({ "bot_token": "t" }))]
    );

    let refused = kept_by
        .configure_module(configure("who", Some(empty.clone()), &["api_url"]))
        .await
        .unwrap()
        .into_inner()
        .error
        .unwrap();
    assert_eq!(
        refused.kind,
        i32::from(wire::ModuleErrorKind::InvalidArgument)
    );
    let fixed = service()
        .await
        .configure_module(configure("who", Some(empty), &[]))
        .await
        .unwrap()
        .into_inner()
        .error
        .unwrap();
    assert_eq!(
        fixed.kind,
        i32::from(wire::ModuleErrorKind::FailedPrecondition)
    );
    let malformed = kept_by
        .configure_module(configure("who", None, &[]))
        .await
        .unwrap_err();
    assert_eq!(malformed.code(), tonic::Code::InvalidArgument);
}

#[tokio::test]
async fn views_are_listed_and_read_and_an_unknown_one_answers_its_error() {
    let service = service().await;
    let listed = service
        .list_views(Request::new(wire::ListViewsRequest {}))
        .await
        .unwrap()
        .into_inner();
    let ids: Vec<(&str, &str)> = listed
        .views
        .iter()
        .map(|v| (v.module_id.as_str(), v.view.as_ref().unwrap().id.as_str()))
        .collect();
    assert_eq!(ids, [("who", "card")]);

    let get = |view: &str| {
        Request::new(wire::GetViewRequest {
            module_id: "who".into(),
            view_id: view.into(),
        })
    };
    let card = service.get_view(get("card")).await.unwrap().into_inner();
    assert_eq!(card.error, None);
    assert_eq!(card.resource.unwrap().text, "<p>who</p>");
    let unknown = service.get_view(get("nope")).await.unwrap().into_inner();
    assert_eq!(
        unknown.error.map(|e| e.kind),
        Some(wire::ModuleErrorKind::NotFound.into())
    );
    assert_eq!((unknown.view, unknown.resource), (None, None));
}

#[tokio::test]
async fn a_grant_goes_to_the_store_on_the_machine_only_and_a_relative_root_is_refused() {
    let grant = |root: &str| {
        Request::new(wire::GrantModuleRequest {
            id: "tools-git".into(),
            roots: vec![wire::SandboxRoot {
                path: root.into(),
                write: true,
            }],
            network: false,
            spawn: true,
        })
    };
    let kind = |response: wire::GrantModuleResponse| response.error.unwrap().kind;
    let elsewhere = service().await.grant_module(grant("/repo")).await.unwrap();
    assert_eq!(
        kind(elsewhere.into_inner()),
        i32::from(wire::ModuleErrorKind::FailedPrecondition),
        "a host without the store (the cloud's routes) grants nothing"
    );
    let kept_by = HostApi::new(host().await, Some(Arc::new(Kept::default())));
    let relative = kept_by.grant_module(grant("repo")).await.unwrap();
    assert_eq!(
        kind(relative.into_inner()),
        i32::from(wire::ModuleErrorKind::InvalidArgument)
    );
    let unknown = kept_by.grant_module(grant("/repo")).await.unwrap();
    assert_eq!(
        kind(unknown.into_inner()),
        i32::from(wire::ModuleErrorKind::NotFound)
    );
}
