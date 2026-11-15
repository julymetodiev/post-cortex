// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Requests of the browser transfer are parsed strictly, as the RPCs'.

use super::*;

use tonic::Code;
use uuid::Uuid;

#[test]
fn import_queries_are_strict() {
    let settings = import_settings(Some(
        "commit=true&skip_existing=1&session_id=a%20b&session_id=c&workspace_id=w",
    ))
    .unwrap();
    assert!(settings.commit && settings.skip_existing && !settings.overwrite);
    assert_eq!(settings.session_ids, ["a b", "c"]);
    assert_eq!(settings.workspace_ids, ["w"]);
    assert_eq!(
        import_settings(None).unwrap(),
        pb::ImportSettings::default()
    );
    for bad in ["commit=maybe", "galaxy=1", "overwrite="] {
        assert_eq!(
            import_settings(Some(bad)).unwrap_err().code(),
            Code::InvalidArgument,
            "{bad}"
        );
    }
}

#[test]
fn export_bodies_are_checked_as_the_rpc_checks_them() {
    let all = export_request(b"").unwrap();
    assert_eq!(all, pb::ExportRequest::default());
    let ws = Uuid::new_v4();
    let scoped = export_request(
        format!(r#"{{"scope":"workspace","workspace_id":"{ws}","compression":"zstd"}}"#).as_bytes(),
    )
    .unwrap();
    assert_eq!(scoped.workspace_id, ws.to_string());
    assert!(filename(&scoped).ends_with(".json.zst"));
    assert_eq!(content_type(&scoped.compression), "application/zstd");
    for bad in [
        r#"{"scope":"galaxy"}"#,
        r#"{"scope":"session"}"#,
        r#"{"scope":"workspace","workspace_id":"x"}"#,
        r#"{"compression":"rar"}"#,
        r#"{"color":"red"}"#,
        "not json",
    ] {
        assert_eq!(
            export_request(bad.as_bytes()).unwrap_err().code(),
            Code::InvalidArgument,
            "{bad}"
        );
    }
}

#[test]
fn blob_requests_are_strict() {
    assert_eq!(
        blobs::upload_workspace(Some("workspace_id=w")).unwrap(),
        "w"
    );
    assert!(blobs::upload_workspace(None).is_err());
    assert!(blobs::upload_workspace(Some("workspace_id=w&name=x")).is_err());
    let request = blobs::link_request(br#"{"workspace_id": "w", "sha256": "ab"}"#).unwrap();
    assert_eq!(
        (request.workspace_id.as_str(), request.sha256.as_str()),
        ("w", "ab")
    );
    assert!(blobs::link_request(br#"{"workspace_id": "w"}"#).is_err());
    assert!(blobs::link_request(br#"{"workspace_id": "w", "sha256": "ab", "x": 1}"#).is_err());
}
