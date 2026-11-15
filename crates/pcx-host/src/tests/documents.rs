// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Document kinds: checked when a manifest is loaded.

use super::*;

use pcx_module_api::DocumentKind;

/// A module with a kind whose data is `pcx.v1.SessionInfo` (real
/// descriptors: `name` a string, `update_count` a number), titled by
/// `title_field`.
fn with_kind(title_field: &str) -> Fake {
    let mut module = tool(Act::Answer);
    module.manifest.document_kinds = vec![DocumentKind {
        name: "task".to_string(),
        title_field: title_field.to_string(),
        searchable: None,
        r#type: "pcx.v1.SessionInfo".to_string(),
    }];
    module.manifest.descriptors = post_cortex_proto::PCX_DESCRIPTOR_SET.to_vec();
    module
}

#[tokio::test]
async fn a_module_with_document_kinds_starts() {
    let host = Host::start(
        vec![Arc::new(with_kind("name"))],
        &HashMap::new(),
        &HashMap::new(),
    )
    .await;
    assert!(host.is_ok(), "{:?}", host.err());
}

#[tokio::test]
async fn a_kind_without_a_string_title_refuses_the_manifest() {
    let refused = Host::start(
        vec![Arc::new(with_kind("update_count"))],
        &HashMap::new(),
        &HashMap::new(),
    )
    .await;
    match refused {
        Err(HostError::InvalidManifest { reason, .. }) => {
            assert!(reason.contains("title field"), "{reason}");
        }
        other => panic!("expected InvalidManifest, got {:?}", other.err()),
    }
}
