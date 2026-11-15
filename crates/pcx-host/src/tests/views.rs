// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Module views: checked against the module's RPCs when it loads, listed,
//! read with their HTML or the module's descriptors; tools only views call
//! stay out of the agents' list.

use super::*;

use pcx_module_api::Capability;
use pcx_module_api::view::{Body, Component, Format, Source, Stat, View, component};

/// Long enough for a fake module's answer.
const SHORT: Duration = Duration::from_secs(1);

/// The RPC the views read: the skills a library has.
const LIST: &str = "/pcx.skills.v1.Skills/ListSkills";

/// A stat over the RPC `rpc`.
fn stat_view(id: &str, rpc: &str) -> View {
    View {
        id: id.to_string(),
        title: "Rows".to_string(),
        description: String::new(),
        body: Some(Body::Root(Box::new(Component {
            kind: Some(component::Kind::Stat(Stat {
                label: "Rows".to_string(),
                source: Some(Source {
                    rpc: rpc.to_string(),
                    request: None,
                    pointer: "/skills".to_string(),
                }),
                format: Format::Number.into(),
            })),
        }))),
    }
}

fn html_view(uri: &str) -> View {
    View {
        id: "card".to_string(),
        title: "Card".to_string(),
        description: String::new(),
        body: Some(Body::ResourceUri(uri.to_string())),
    }
}

/// A tool module with `views`, granted `ui_html`, naming [`LIST`] of its
/// descriptors.
fn with_views(views: Vec<View>) -> Fake {
    let mut module = tool(Act::Answer);
    module.manifest.capabilities = vec![Capability::UiHtml.into()];
    module.manifest.views = views;
    module.manifest.descriptors = post_cortex_proto::SKILLS_DESCRIPTOR_SET.to_vec();
    module.manifest.rpcs = vec![post_cortex_proto::module::Rpc {
        method: LIST.to_string(),
        tool: String::new(),
        visibility: Vec::new(),
        effect: post_cortex_proto::module::Effect::Read.into(),
    }];
    module
}

#[tokio::test]
async fn views_are_listed_and_read_with_their_html() {
    let module = with_views(vec![
        stat_view("rows", LIST),
        html_view("ui://tool/card.html"),
    ]);
    let host = Host::start(vec![Arc::new(module)], &HashMap::new(), &HashMap::new())
        .await
        .unwrap();
    let listed: Vec<(String, String)> = host
        .views()
        .into_iter()
        .map(|(module, view)| (module, view.id))
        .collect();
    assert_eq!(
        listed,
        [
            ("tool".into(), "rows".into()),
            ("tool".into(), "card".into())
        ]
    );

    let rows = host.view(&scope(SHORT), "tool", "rows").await.unwrap();
    assert_eq!((rows.view.id.as_str(), rows.resource), ("rows", None));
    // The app builds its requests from the module's descriptors.
    assert_eq!(rows.descriptors, post_cortex_proto::SKILLS_DESCRIPTOR_SET);
    let card = host.view(&scope(SHORT), "tool", "card").await.unwrap();
    let resource = card.resource.unwrap();
    assert_eq!(resource.mime_type, HTML_MIME);
    assert_eq!(resource.text, "<p>ui://tool/card.html</p>");

    for (module, view) in [("tool", "nope"), ("nobody", "rows")] {
        let error = host.view(&scope(SHORT), module, view).await.unwrap_err();
        assert_eq!(error.kind, ModuleErrorKind::NotFound, "{module}/{view}");
    }
}

#[tokio::test]
async fn tools_only_views_call_stay_out_of_the_agents_list() {
    let host = Host::start(
        vec![Arc::new(tool(Act::Answer))],
        &HashMap::new(),
        &HashMap::new(),
    )
    .await
    .unwrap();
    let names: Vec<String> = host.tools().into_iter().map(|t| t.name).collect();
    assert_eq!(names, ["tool_echo"]);
    assert!(host.has_tool("tool_rows"));
}

#[tokio::test]
async fn a_view_over_an_rpc_the_module_lacks_refuses_the_module() {
    let host = Host::start(vec![], &HashMap::new(), &HashMap::new())
        .await
        .unwrap();
    let module = with_views(vec![stat_view("rows", "/pcx.skills.v1.Skills/GetSkill")]);
    let error = host
        .reload(vec![Arc::new(module)], &HashMap::new(), &HashMap::new())
        .await
        .unwrap_err();
    let HostError::InvalidView { module, error } = error else {
        panic!("{error}");
    };
    assert_eq!(module, "tool");
    assert_eq!(
        (error.view.as_str(), error.at.as_str()),
        ("rows", "root.stat")
    );
    assert!(host.modules().is_empty(), "the running set stays");
}

#[tokio::test]
async fn an_html_view_needs_ui_html() {
    let mut module = with_views(vec![html_view("ui://tool/card.html")]);
    module.manifest.capabilities.clear();
    let Err(HostError::InvalidView { error, .. }) =
        Host::start(vec![Arc::new(module)], &HashMap::new(), &HashMap::new()).await
    else {
        panic!("an HTML view without ui_html loaded");
    };
    assert!(error.reason.contains("ui_html"), "{error}");
}
