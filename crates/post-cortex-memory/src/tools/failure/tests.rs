// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use super::*;
use uuid::Uuid;

#[test]
fn every_kind_has_its_wire_name() {
    for kind in Kind::ALL {
        assert_eq!(
            serde_json::to_value(kind).unwrap(),
            serde_json::json!(kind.as_str())
        );
    }
}

/// The details of an invalid argument are structured data, never text in
/// the message.
#[test]
fn details_are_data_beside_the_message() {
    let failure = Failure::invalid("Invalid UUID format: 'x'")
        .parameter("session_id")
        .expected("UUID")
        .received("x")
        .hint("Create a session first");
    assert_eq!(failure.message, "Invalid UUID format: 'x'");
    assert_eq!(
        failure.data(),
        serde_json::json!({
            "kind": "invalid_argument",
            "parameter": "session_id",
            "expectedType": "UUID",
            "receivedValue": "x",
            "hint": "Create a session first",
        })
    );
    assert_eq!(
        Failure::internal("disk").data(),
        serde_json::json!({"kind": "internal"})
    );
}

/// The capability errors keep their kind: not found, invalid, conflict and
/// internal are told apart, never guessed from the text.
#[test]
fn capability_errors_keep_their_kind() {
    let id = Uuid::nil();
    let cases: Vec<(Failure, Kind)> = vec![
        (RecordError::SessionNotFound(id).into(), Kind::NotFound),
        (
            RecordError::InvalidArgument("x".into()).into(),
            Kind::InvalidArgument,
        ),
        (
            RecordError::Storage(RuntimeError::Port(post_cortex_ports::error::corrupt(
                "update", "u", "bad",
            )))
            .into(),
            Kind::Internal,
        ),
        (
            RecordError::Storage(RuntimeError::Port(PortError::unavailable("store down"))).into(),
            Kind::Unavailable,
        ),
        (
            RecordError::BulkItem {
                index: 2,
                error: Box::new(RecordError::SessionNotFound(id)),
            }
            .into(),
            Kind::NotFound,
        ),
        (
            SessionError::Storage(RuntimeError::Persist(PersistError::SessionDeleted)).into(),
            Kind::NotFound,
        ),
        (
            WorkspaceError::Conflict("taken".into()).into(),
            Kind::Conflict,
        ),
        (WorkspaceError::NotFound("w".into()).into(), Kind::NotFound),
        (SessionError::NotFound(id).into(), Kind::NotFound),
        (SearchError::WorkspaceNotFound(id).into(), Kind::NotFound),
        (
            ViewsError::Scope(ScopeError::Missing).into(),
            Kind::InvalidArgument,
        ),
        (
            AssemblyError::Scope(ScopeError::NotFound("s".into())).into(),
            Kind::NotFound,
        ),
        (
            TransferError::ModelMismatch("m".into()).into(),
            Kind::FailedPrecondition,
        ),
        (
            CardError::Storage(RuntimeError::Persist(PersistError::Panicked)).into(),
            Kind::Internal,
        ),
        (TransferError::WorkspaceNotFound(id).into(), Kind::NotFound),
    ];
    for (failure, kind) in cases {
        assert_eq!(failure.kind, kind, "{failure:?}");
    }
}
