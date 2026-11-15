// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use post_cortex_proto::module::{BusEvent, Effect, Rpc};
use prost::Message;
use prost_reflect::prost_types::{DescriptorProto, FileDescriptorProto, FileDescriptorSet};
use prost_reflect::{DynamicMessage, SerializeOptions};

use super::pools;
use crate::Manifest;
use crate::rpc_tool::tools_in;

fn manifest(id: &str, descriptors: &[u8], rpcs: Vec<Rpc>) -> Manifest {
    Manifest {
        id: id.into(),
        descriptors: descriptors.to_vec(),
        rpcs,
        ..Manifest::default()
    }
}

/// A set of one file `name` with one empty message `message`.
fn set(name: &str, message: &str) -> Vec<u8> {
    FileDescriptorSet {
        file: vec![FileDescriptorProto {
            name: Some(name.into()),
            package: Some("probe.v1".into()),
            syntax: Some("proto3".into()),
            message_type: vec![DescriptorProto {
                name: Some(message.into()),
                ..DescriptorProto::default()
            }],
            ..FileDescriptorProto::default()
        }],
    }
    .encode_to_vec()
}

#[test]
fn an_answer_that_carries_another_modules_message_maps_to_json() {
    let runs = Rpc {
        method: "/pcx.workflow.v1.Workflows/GetRun".into(),
        tool: "workflow_run_get".into(),
        visibility: Vec::new(),
        effect: Effect::Read.into(),
    };
    let workflow = manifest(
        "workflow",
        post_cortex_proto::WORKFLOW_DESCRIPTOR_SET,
        vec![runs],
    );
    let discord = manifest("discord", post_cortex_proto::DISCORD_DESCRIPTOR_SET, vec![]);
    let pools = pools(&[&workflow, &discord]).unwrap();
    assert!(pools[0] == pools[1], "one pool for both");
    let tool = tools_in(&workflow, &pools[0]).unwrap().remove(0);

    // A run an event of Discord started, as the workflow module answers it.
    let post = post_cortex_proto::discord::PostCreated {
        channel: "help".into(),
        ..Default::default()
    };
    let run = post_cortex_proto::workflow::RunDetail {
        run: Some(post_cortex_proto::workflow::RunRecord {
            run: Some(post_cortex_proto::workflow::Run {
                trigger: Some(post_cortex_proto::workflow::Fired {
                    event: Some(BusEvent {
                        payload: Some(pbjson_types::Any {
                            type_url: "type.googleapis.com/pcx.discord.v1.PostCreated".into(),
                            value: post.encode_to_vec().into(),
                        }),
                        ..BusEvent::default()
                    }),
                    ..Default::default()
                }),
                ..Default::default()
            }),
            ..Default::default()
        }),
        steps: Vec::new(),
    };
    let answer =
        DynamicMessage::decode(tool.response().clone(), run.encode_to_vec().as_slice()).unwrap();
    let json = answer
        .serialize_with_options(
            serde_json::value::Serializer,
            &SerializeOptions::new().use_proto_field_name(true),
        )
        .unwrap();
    assert_eq!(
        json["run"]["run"]["trigger"]["event"]["payload"]["channel"],
        "help"
    );
}

#[test]
fn a_module_whose_file_differs_from_anothers_of_its_name_keeps_its_own_pool() {
    let first = manifest("first", &set("probe.proto", "One"), vec![]);
    let same = manifest("same", &set("probe.proto", "One"), vec![]);
    let other = manifest("other", &set("probe.proto", "Two"), vec![]);
    let pools = pools(&[&first, &same, &other]).unwrap();
    assert!(pools[0] == pools[1], "the same file is one file");
    assert!(pools[0].get_message_by_name("probe.v1.One").is_some());
    assert!(pools[0].get_message_by_name("probe.v1.Two").is_none());
    assert!(pools[2].get_message_by_name("probe.v1.Two").is_some());
    assert!(pools[2].get_message_by_name("probe.v1.One").is_none());

    let unread = manifest("unread", b"not descriptors", vec![]);
    let (index, why) = pools_err(&[&first, &unread]);
    assert_eq!(index, 1);
    assert!(why.contains("do not read"), "{why}");
}

fn pools_err(manifests: &[&Manifest]) -> (usize, String) {
    pools(manifests).unwrap_err()
}
