// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use super::*;

fn workspace() -> Workspace {
    Workspace::new(
        Uuid::new_v4(),
        "w".into(),
        "w".into(),
        "d".into(),
        SystemTime::UNIX_EPOCH,
    )
}

#[test]
fn members_and_roles() {
    let mut ws = workspace();
    let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
    assert_eq!(ws.add_session(a, SessionRole::Primary), None);
    assert_eq!(ws.add_session(b, SessionRole::Related), None);
    assert_eq!(
        ws.add_session(b, SessionRole::Shared),
        Some(SessionRole::Related)
    );
    assert_eq!(ws.session_count(), 2);
    assert_eq!(ws.get_session_role(&a), Some(SessionRole::Primary));
    assert_eq!(ws.remove_session(&a), Some(SessionRole::Primary));
    assert_eq!(ws.remove_session(&a), None);
    assert_eq!(ws.get_all_sessions(), [(b, SessionRole::Shared)]);
}

#[test]
fn rebinding_a_project_replaces_its_session() {
    let mut ws = workspace();
    let (first, second) = (Uuid::new_v4(), Uuid::new_v4());
    for session_id in [first, second] {
        ws.set_project(WorkspaceProject {
            key: "github.com/a/b".into(),
            session_id,
        });
    }
    assert_eq!(ws.projects().len(), 1);
    assert_eq!(ws.project("github.com/a/b").unwrap().session_id, second);
    assert!(ws.project("x").is_none());
}

#[test]
fn archiving_reports_the_previous_state() {
    let mut ws = workspace();
    let now = chrono::Utc::now();
    assert_eq!(ws.set_archived(Some(now)), None);
    assert!(ws.is_archived());
    assert_eq!(ws.set_archived(None), Some(now));
    assert!(!ws.is_archived());
}

/// A workspace serialises whole: members, projects and archive time come
/// back (they were `serde(skip)`ped and lost).
#[test]
fn a_workspace_round_trips_with_members_and_projects() {
    let mut ws = workspace();
    ws.add_session(Uuid::new_v4(), SessionRole::Dependency);
    ws.set_project(WorkspaceProject {
        key: "local:abc".into(),
        session_id: Uuid::new_v4(),
    });
    ws.set_archived(Some(chrono::Utc::now()));
    let back: Workspace = serde_json::from_value(serde_json::to_value(&ws).unwrap()).unwrap();
    assert_eq!(back, ws);
}

/// A copy is independent: changing it leaves the original as it was
/// (copies shared their members and metadata).
#[test]
fn copies_are_independent() {
    let mut ws = workspace();
    let copy = ws.clone();
    ws.add_session(Uuid::new_v4(), SessionRole::Primary);
    ws.set_archived(Some(chrono::Utc::now()));
    assert_eq!(copy.session_count(), 0);
    assert!(!copy.is_archived());
}
