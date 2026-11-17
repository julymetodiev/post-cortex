// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Every field the operation table names exists in the message of its RPC
//! (`pcx.v1`, its blobs, or `pcx.views.v1`; the request, or the answer for a count taken
//! from it), checked against the descriptor a gateway reads them with.

use post_cortex_memory::tools::operations::{Count, Effect, OPERATIONS};
use prost::Message;
use prost_types::{DescriptorProto, FileDescriptorSet};

fn message<'a>(set: &'a FileDescriptorSet, name: &str) -> &'a DescriptorProto {
    let name = name.trim_start_matches('.');
    set.file
        .iter()
        .flat_map(|f| f.message_type.iter().map(move |m| (f.package(), m)))
        .find(|(package, m)| format!("{package}.{}", m.name()) == name)
        .map(|(_, m)| m)
        .unwrap_or_else(|| panic!("no message {name}"))
}

/// Whether `path` (`settings.commit`) names a field of `message`.
fn has(set: &FileDescriptorSet, message_name: &str, path: &str) -> bool {
    let mut current = message(set, message_name);
    let mut parts = path.split('.').peekable();
    while let Some(part) = parts.next() {
        let Some(field) = current.field.iter().find(|f| f.name() == part) else {
            return false;
        };
        if parts.peek().is_some() {
            current = message(set, field.type_name());
        }
    }
    true
}

#[test]
fn every_field_the_table_names_is_in_its_messages() {
    let pcx = FileDescriptorSet::decode(post_cortex_proto::PCX_DESCRIPTOR_SET).unwrap();
    let views = FileDescriptorSet::decode(post_cortex_proto::VIEWS_DESCRIPTOR_SET).unwrap();
    let mut checked = 0;
    for op in OPERATIONS {
        let rpcs = op
            .grpc
            .iter()
            .map(|rpc| (&pcx, "PostCortex", rpc))
            .chain(op.blobs.iter().map(|rpc| (&pcx, "Blobs", rpc)))
            .chain(op.views.iter().map(|rpc| (&views, "Views", rpc)));
        for (set, service, rpc) in rpcs {
            let service = set
                .file
                .iter()
                .flat_map(|f| &f.service)
                .find(|s| s.name() == service)
                .unwrap();
            let method = service
                .method
                .iter()
                .find(|m| m.name() == *rpc)
                .unwrap_or_else(|| panic!("{}: no RPC {rpc}", op.name));
            let (input, output) = (method.input_type(), method.output_type());
            let mut request: Vec<&str> = op.scope.sessions.to_vec();
            request.extend(op.scope.workspaces);
            let mut answer = Vec::new();
            let counts = match &op.effect {
                Effect::Write { counts, commit } => {
                    request.extend(commit);
                    *counts
                }
                Effect::Delete { counts } => *counts,
                Effect::Search {
                    rerank: Some(rerank),
                } => {
                    request.push(rerank.field);
                    request.push(rerank.withheld);
                    &[]
                }
                Effect::Search { rerank: None } | Effect::Read => &[],
            };
            for tally in counts {
                match tally.by {
                    Count::One => {}
                    Count::Items(field) => request.push(field),
                    Count::Answer(field) => answer.push(field),
                }
            }
            for field in request {
                assert!(
                    has(set, input, field),
                    "{} {rpc}: {input} has no {field}",
                    op.name
                );
                checked += 1;
            }
            for field in answer {
                assert!(
                    has(set, output, field),
                    "{} {rpc}: {output} has no {field}",
                    op.name
                );
                checked += 1;
            }
        }
    }
    assert!(checked > 40, "only {checked} fields checked");
}
