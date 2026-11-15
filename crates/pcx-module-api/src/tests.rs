// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use super::*;
use post_cortex_proto::module as wire;
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;

#[test]
fn a_module_runs_on_the_same_major_up_to_the_hosts_minor() {
    let host = ApiVersion { major: 1, minor: 2 };
    assert!(compatible(&host, &ApiVersion { major: 1, minor: 0 }));
    assert!(compatible(&host, &ApiVersion { major: 1, minor: 2 }));
    assert!(!compatible(&host, &ApiVersion { major: 1, minor: 3 }));
    assert!(!compatible(&host, &ApiVersion { major: 2, minor: 0 }));
}

#[test]
fn scope_crosses_the_wire_with_the_time_left() {
    let scope = Scope {
        tenant: "t".into(),
        profile: "p".into(),
        session_id: Some("s".into()),
        workspace_id: None,
        author: Some("claude".into()),
        caller_module: Some("board".into()),
        call_depth: 2,
        turn: None,
        reach: None,
        role: None,
        read_only: false,
        person: None,
        deadline: Instant::now() + Duration::from_secs(2),
        cancel: CancellationToken::new(),
    };
    let wire = scope.to_wire();
    assert!(
        wire.deadline_ms > 1000 && wire.deadline_ms <= 2000,
        "{wire:?}"
    );
    assert_eq!(wire.workspace_id, "");
    let back = Scope::from_wire(wire, CancellationToken::new(), Duration::from_secs(9));
    assert_eq!(back.session_id.as_deref(), Some("s"));
    assert_eq!(back.workspace_id, None);
    assert_eq!(
        (back.caller_module.as_deref(), back.call_depth),
        (Some("board"), 2)
    );
    assert!(back.remaining() <= Duration::from_secs(2));
    let unset = wire::Scope::default();
    let defaulted = Scope::from_wire(unset, CancellationToken::new(), Duration::from_secs(9));
    assert!(defaulted.remaining() > Duration::from_secs(8));
    assert_eq!((defaulted.caller_module, defaulted.call_depth), (None, 0));
}

#[test]
fn a_reach_crosses_the_wire_and_the_metadata_and_none_is_not_every_one() {
    let scope = |reach: Option<Vec<String>>| Scope {
        tenant: String::new(),
        profile: String::new(),
        session_id: None,
        workspace_id: None,
        author: None,
        caller_module: None,
        call_depth: 0,
        turn: None,
        reach,
        role: None,
        read_only: false,
        person: None,
        deadline: Instant::now() + Duration::from_secs(2),
        cancel: CancellationToken::new(),
    };
    for reach in [
        None,
        Some(Vec::new()),
        Some(vec!["w1".to_string(), "w2".to_string()]),
    ] {
        let wire = scope(reach.clone()).to_wire();
        let back = Scope::from_wire(wire, CancellationToken::new(), Duration::from_secs(9));
        assert_eq!(back.reach, reach);
        let mut metadata = tonic::metadata::MetadataMap::new();
        scope(reach.clone()).to_metadata(&mut metadata).unwrap();
        let back =
            Scope::from_metadata(&metadata, CancellationToken::new(), Duration::from_secs(9))
                .unwrap();
        assert_eq!(back.reach, reach);
    }
    // An empty reach, however a forwarder sent it, reaches no workspace.
    let mut metadata = tonic::metadata::MetadataMap::new();
    metadata.insert(metadata::REACH, "".parse().unwrap());
    let back =
        Scope::from_metadata(&metadata, CancellationToken::new(), Duration::from_secs(9)).unwrap();
    assert_eq!(back.reach, Some(Vec::new()));
}

#[test]
fn hook_names_map_to_event_kinds() {
    assert_eq!(event_kind("SessionStart"), Some(EventKind::SessionStart));
    assert_eq!(event_kind("UserPromptSubmit"), Some(EventKind::TurnStart));
    assert_eq!(event_kind("Stop"), Some(EventKind::TurnEnd));
    assert_eq!(event_kind("PreToolUse"), Some(EventKind::PreTool));
    assert_eq!(event_kind("Notification"), None);
}

#[test]
fn error_kinds_cross_the_wire() {
    for kind in [
        ModuleErrorKind::InvalidArgument,
        ModuleErrorKind::NotFound,
        ModuleErrorKind::Unavailable,
        ModuleErrorKind::DeadlineExceeded,
        ModuleErrorKind::Denied,
        ModuleErrorKind::Internal,
        ModuleErrorKind::Conflict,
        ModuleErrorKind::FailedPrecondition,
    ] {
        let wire: wire::ModuleError = ModuleError::new(kind, "x").into();
        assert_eq!(ModuleError::from(wire).kind, kind);
    }
    // An unset kind is a defect of the module, not something to act on.
    let unset = wire::ModuleError {
        kind: 0,
        message: "?".into(),
        field: String::new(),
    };
    assert_eq!(ModuleError::from(unset).kind, ModuleErrorKind::Internal);
}

#[test]
fn the_field_at_fault_crosses_the_wire() {
    let error = ModuleError::new(ModuleErrorKind::InvalidArgument, "bad").with_field("token");
    let back = ModuleError::from(wire::ModuleError::from(error.clone()));
    assert_eq!(back, error);
    let none = ModuleError::from(wire::ModuleError::from(ModuleError::new(
        ModuleErrorKind::NotFound,
        "gone",
    )));
    assert_eq!(none.field, None);
}

#[test]
fn tool_visibility_crosses_the_wire_as_mcp_apps_reads_it() {
    let spec = |visibility| ToolSpec {
        name: "t".into(),
        description: String::new(),
        input_schema: serde_json::Map::new(),
        visibility,
        effect: Effect::External,
    };
    for visibility in [Visibility::Both, Visibility::Model, Visibility::App] {
        let back = ToolSpec::try_from(wire::ToolSpec::from(spec(visibility))).unwrap();
        assert_eq!(back.visibility, visibility);
        assert_eq!(back.effect, Effect::External);
    }
    assert!(
        wire::ToolSpec::from(spec(Visibility::Both))
            .visibility
            .is_empty()
    );
    assert!(Visibility::Both.model() && Visibility::Model.model());
    assert!(!Visibility::App.model());

    let mut unset = wire::ToolSpec::from(spec(Visibility::Model));
    unset.visibility = vec![wire::ToolVisibility::Unspecified.into()];
    let error = ToolSpec::try_from(unset).unwrap_err();
    assert_eq!(error.kind, ModuleErrorKind::InvalidArgument);
}

#[test]
fn a_persons_role_crosses_the_wire_and_the_metadata_and_an_unknown_one_is_refused() {
    let scope = |role: Option<PersonRole>| Scope {
        tenant: String::new(),
        profile: String::new(),
        session_id: None,
        workspace_id: None,
        author: Some("julius".into()),
        caller_module: None,
        call_depth: 0,
        turn: None,
        reach: None,
        role,
        read_only: false,
        person: Some("0b3f6f0e-1c2d-4e5f-8a9b-0c1d2e3f4a5b".into()),
        deadline: Instant::now() + Duration::from_secs(2),
        cancel: CancellationToken::new(),
    };
    for role in [None, Some(PersonRole::Owner), Some(PersonRole::Viewer)] {
        let back = Scope::from_wire(
            scope(role).to_wire(),
            CancellationToken::new(),
            Duration::from_secs(9),
        );
        assert_eq!(back.role, role);
        assert_eq!(
            back.person,
            scope(role).person,
            "the person crosses the wire"
        );
        let mut metadata = tonic::metadata::MetadataMap::new();
        scope(role).to_metadata(&mut metadata).unwrap();
        let back =
            Scope::from_metadata(&metadata, CancellationToken::new(), Duration::from_secs(9))
                .unwrap();
        assert_eq!(back.role, role);
        assert_eq!(back.person, scope(role).person, "and the metadata");
    }
    let mut metadata = tonic::metadata::MetadataMap::new();
    metadata.insert(metadata::ROLE, "root".parse().unwrap());
    let refused = Scope::from_metadata(&metadata, CancellationToken::new(), Duration::from_secs(9))
        .unwrap_err();
    assert_eq!(refused.kind, ModuleErrorKind::InvalidArgument);
    // A role this version does not know, over the wire, is none: the least.
    let unknown = wire::Scope {
        role: 99,
        ..wire::Scope::default()
    };
    let back = Scope::from_wire(unknown, CancellationToken::new(), Duration::from_secs(9));
    assert_eq!(back.role, None);
}

#[test]
fn a_read_only_caller_crosses_the_wire_and_the_metadata_and_another_value_is_refused() {
    let scope = |read_only: bool| Scope {
        tenant: String::new(),
        profile: String::new(),
        session_id: None,
        workspace_id: None,
        author: Some("julius".into()),
        caller_module: None,
        call_depth: 0,
        turn: None,
        reach: None,
        role: None,
        read_only,
        person: None,
        deadline: Instant::now() + Duration::from_secs(2),
        cancel: CancellationToken::new(),
    };
    for read_only in [false, true] {
        let back = Scope::from_wire(
            scope(read_only).to_wire(),
            CancellationToken::new(),
            Duration::from_secs(9),
        );
        assert_eq!(back.read_only, read_only);
        let mut metadata = tonic::metadata::MetadataMap::new();
        scope(read_only).to_metadata(&mut metadata).unwrap();
        assert_eq!(metadata.contains_key(metadata::READ_ONLY), read_only);
        let back =
            Scope::from_metadata(&metadata, CancellationToken::new(), Duration::from_secs(9))
                .unwrap();
        assert_eq!(back.read_only, read_only);
    }
    let mut metadata = tonic::metadata::MetadataMap::new();
    metadata.insert(metadata::READ_ONLY, "yes".parse().unwrap());
    let refused = Scope::from_metadata(&metadata, CancellationToken::new(), Duration::from_secs(9))
        .unwrap_err();
    assert_eq!(refused.kind, ModuleErrorKind::InvalidArgument);
}
