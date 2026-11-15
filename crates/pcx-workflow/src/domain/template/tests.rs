// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use post_cortex_proto::module::TemplateParameter;
use prost_reflect::DescriptorPool;

use super::*;

fn parameter(name: &str, default: &str, pattern: &str) -> TemplateParameter {
    TemplateParameter {
        name: name.into(),
        description: String::new(),
        default: default.into(),
        pattern: pattern.into(),
    }
}

/// A template whose trigger hears channel `${channel}` and whose limit is
/// `${limit}`.
fn template() -> WorkflowTemplate {
    WorkflowTemplate {
        name: "help".into(),
        description: "Answers".into(),
        workflow: r#"{
            "name": "anything",
            "description": "Hears ${channel}",
            "graph": {"blocks": [{"id": "post", "trigger": {"event": {
                "name": "discord.post_created",
                "filter": "event.payload.channel == '${channel}'"
            }}}]},
            "budgets": {"steps": "${limit}"}
        }"#
        .into(),
        parameters: vec![
            parameter("channel", "help", "[a-z0-9-]{1,64}"),
            parameter("limit", "", "[0-9]{1,4}"),
        ],
    }
}

fn given(values: &[(&str, &str)]) -> HashMap<String, String> {
    values
        .iter()
        .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
        .collect()
}

#[test]
fn a_template_is_made_a_workflow_with_the_values_given_or_its_defaults() {
    let workflow = installed(&template(), &given(&[("limit", "20")])).unwrap();
    assert_eq!(workflow.name, "help", "named as the template");
    assert_eq!(workflow.description, "Hears help");
    assert_eq!(workflow.budgets.unwrap().steps, 20);
    let mine = installed(
        &template(),
        &given(&[("limit", "5"), ("channel", "support")]),
    )
    .unwrap();
    let Some(post_cortex_proto::workflow::block::Kind::Trigger(trigger)) =
        &mine.graph.unwrap().blocks[0].kind
    else {
        panic!("a trigger");
    };
    let Some(post_cortex_proto::workflow::trigger::Kind::Event(event)) = &trigger.kind else {
        panic!("an event trigger");
    };
    assert_eq!(event.filter, "event.payload.channel == 'support'");
}

#[test]
fn a_value_that_breaks_its_pattern_or_names_nothing_is_refused() {
    let refused = |values: &[(&str, &str)]| installed(&template(), &given(values)).unwrap_err();
    assert_eq!(refused(&[]).field, "values.limit", "no default");
    let quote = refused(&[("limit", "1"), ("channel", "x' || true || '")]);
    assert_eq!(
        quote.field, "values.channel",
        "a quote cannot reach the filter"
    );
    assert_eq!(
        refused(&[("limit", "1"), ("other", "x")]).field,
        "values.other"
    );

    let mut unnamed = template();
    unnamed.workflow = unnamed.workflow.replace("${channel}'", "${chanel}'");
    let named = installed(&unnamed, &given(&[("limit", "1")])).unwrap_err();
    assert_eq!(named.field, "template.workflow");
    assert!(named.message.contains("${chanel}"), "{}", named.message);

    let mut loose = template();
    loose.parameters[0].pattern = String::new();
    let field = installed(&loose, &given(&[("limit", "1")]))
        .unwrap_err()
        .field;
    assert_eq!(
        field, "template.parameters.channel",
        "every value has a pattern"
    );

    let mut wrong = template();
    wrong.workflow = r#"{"graph": {"blocks": "no"}}"#.into();
    let field = installed(&wrong, &given(&[("limit", "1")]))
        .unwrap_err()
        .field;
    assert_eq!(field, "template.workflow");
}

/// The tools of `manifest`'s RPCs that are tools, by path, as the host
/// lists them.
fn tools_of(manifest: &pcx_module_api::Manifest) -> Vec<(String, crate::domain::Tool)> {
    let pool = DescriptorPool::decode(manifest.descriptors.as_slice()).unwrap();
    manifest
        .rpcs
        .iter()
        .filter(|rpc| !rpc.tool.is_empty())
        .map(|rpc| {
            let (service, method) = rpc.method[1..].split_once('/').unwrap();
            let found = pool
                .get_service_by_name(service)
                .unwrap()
                .methods()
                .find(|m| m.name() == method)
                .unwrap();
            let tool = crate::domain::Tool {
                name: rpc.tool.clone(),
                description: String::new(),
                request_schema: serde_json::Map::new(),
                response_schema: serde_json::Map::new(),
                module: manifest.id.clone(),
                effect: pcx_module_api::Effect::try_from(rpc.effect).unwrap(),
                request: found.input(),
                response: found.output(),
            };
            (rpc.method.clone(), tool)
        })
        .collect()
}

#[test]
fn the_discord_templates_make_workflows_that_check_against_its_tools() {
    use pcx_module_api::Module;

    let discord = pcx_discord::DiscordModule::new();
    let manifest = discord.manifest();
    let mut tools = tools_of(manifest);
    tools.extend(crate::test_support::board_tools());
    let catalog = crate::domain::Catalog::new(tools).unwrap();
    let names: Vec<&str> = manifest
        .workflow_templates
        .iter()
        .map(|t| t.name.as_str())
        .collect();
    assert_eq!(
        names,
        [
            "discord-help",
            "discord-welcome",
            "discord-changelog",
            "discord-announce",
            "discord-feedback",
            "discord-automod"
        ]
    );
    for shipped in &manifest.workflow_templates {
        let workflow =
            installed(shipped, &HashMap::new()).unwrap_or_else(|r| panic!("{}: {r}", shipped.name));
        let checked = crate::domain::check(&workflow, &catalog, &BTreeMap::new())
            .unwrap_or_else(|r| panic!("{}: {r}", shipped.name));
        let acts = crate::domain::grants::calling(workflow, &checked.found);
        assert!(
            !acts.grants.unwrap().tools.is_empty(),
            "{} acts in Discord",
            shipped.name
        );
    }
    let help = installed(&manifest.workflow_templates[0], &HashMap::new()).unwrap();
    let checked = crate::domain::check(&help, &catalog, &BTreeMap::new()).unwrap();
    assert_eq!(
        crate::domain::grants::calling(help, &checked.found)
            .grants
            .unwrap()
            .tools,
        [
            "discord_ban",
            "discord_lock_thread",
            "discord_post",
            "discord_reply",
            "discord_tag_post",
            "discord_timeout"
        ]
    );
}

/// The kinds an answer may be are the template's own words: the tag of an
/// answered post is the organisation's, and renaming it changes no kind.
#[test]
fn the_help_template_keeps_its_kinds_when_the_answered_tag_is_renamed() {
    use pcx_module_api::Module;
    use post_cortex_proto::workflow::{block::Kind, value};

    let discord = pcx_discord::DiscordModule::new();
    let help = installed(
        &discord.manifest().workflow_templates[0],
        &given(&[("answered_tag", "solved")]),
    )
    .unwrap();
    let blocks = help.graph.unwrap().blocks;
    let answer = blocks.iter().find(|b| b.id == "answer").unwrap();
    let Some(Kind::Agent(agent)) = &answer.kind else {
        panic!("answer is an agent block");
    };
    let fields = &agent.output.as_ref().unwrap().fields;
    let kind = fields.iter().find(|f| f.name == "kind").unwrap();
    let kinds: Vec<&str> = kind
        .constraints
        .as_ref()
        .unwrap()
        .one_of
        .iter()
        .map(|v| match &v.kind {
            Some(value::Kind::String(text)) => text.as_str(),
            other => panic!("a kind is text, not {other:?}"),
        })
        .collect();
    assert_eq!(kinds, ["answered", "not_in_docs", "abuse"]);
    let tagged = blocks.iter().find(|b| b.id == "answered").unwrap();
    let call = format!("{:?}", tagged.kind);
    assert!(
        call.contains("solved"),
        "the post is tagged with the organisation's tag: {call}"
    );
}
