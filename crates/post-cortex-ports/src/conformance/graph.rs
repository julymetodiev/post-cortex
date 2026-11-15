// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The entity graph through the session port: a save writes what its
//! records name, a whole-graph save replaces the stored graph, and the
//! vectorized ids follow the changes. After every save the loaded session
//! has exactly the saved snapshot's graph.

use crate::storage::{SessionChanges, Stores};
use post_cortex_core::core::context_update::ContextUpdate;
use post_cortex_core::session::active_session::ActiveSession;
use uuid::Uuid;

use crate::conformance::sessions::graph_view;
use crate::conformance::{at, record};

fn names(session: &ActiveSession) -> Vec<String> {
    graph_view(session.entity_graph())
        .0
        .into_iter()
        .map(|e| e.0)
        .collect()
}

fn edges(session: &ActiveSession) -> Vec<(String, String)> {
    graph_view(session.entity_graph())
        .1
        .into_iter()
        .map(|e| (e.0, e.1))
        .collect()
}

/// `session` as stored now equals `session` in memory: graph and records.
async fn assert_stored(stores: &Stores, session: &ActiveSession) {
    let loaded = stores
        .sessions
        .load_session(session.id())
        .await
        .unwrap()
        .session;
    assert_eq!(
        graph_view(loaded.entity_graph()),
        graph_view(session.entity_graph())
    );
    let ids = |s: &ActiveSession| s.records().iter().map(|u| u.id).collect::<Vec<_>>();
    assert_eq!(ids(&loaded), ids(session));
}

/// A stored session with A -> B, B -> C and A -> C in its graph.
async fn chain(stores: &Stores) -> ActiveSession {
    let mut session = ActiveSession::new(Uuid::new_v4(), None, None, at(0));
    session.apply_record(record(1, "ab", &["A", "B"]));
    session.apply_record(record(2, "bc", &["B", "C"]));
    session.apply_record(record(3, "ac", &["A", "C"]));
    stores
        .sessions
        .save_session(&session, SessionChanges::everything(&session, &[]))
        .await
        .unwrap();
    session
}

/// A save carrying new records writes the entities and edges they name
/// (counts and times of known ones included) and the records' rows.
pub async fn a_record_save_writes_what_the_records_name(stores: &Stores) {
    let mut session = chain(stores).await;
    let bd = ContextUpdate::clone(&session.apply_record(record(4, "bd", &["B", "D"])));
    let de = ContextUpdate::clone(&session.apply_record(record(5, "de", &["D", "E"])));
    stores
        .sessions
        .save_session(
            &session,
            SessionChanges {
                updates: vec![bd, de],
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_stored(stores, &session).await;
    assert_eq!(names(&session), ["A", "B", "C", "D", "E"]);
}

/// A whole-graph save replaces the stored graph: an entity deleted (and
/// its edges with it) or merged into another is gone after a load.
pub async fn a_whole_graph_save_replaces_the_stored_graph(stores: &Stores) {
    let mut session = chain(stores).await;
    session.entity_graph_mut().remove_entity("B");
    let whole = SessionChanges {
        whole_graph: true,
        ..Default::default()
    };
    stores
        .sessions
        .save_session(&session, whole.clone())
        .await
        .unwrap();
    assert_stored(stores, &session).await;
    assert_eq!(names(&session), ["A", "C"]);
    assert_eq!(edges(&session), [("A".to_string(), "C".to_string())]);

    assert!(session.entity_graph_mut().merge_entities("C", "A"));
    stores.sessions.save_session(&session, whole).await.unwrap();
    assert_stored(stores, &session).await;
    assert_eq!(names(&session), ["A"]);
}

/// A save stores the snapshot's graph: entities and edges the snapshot
/// lost (its records were removed and the graph rebuilt) are gone after a
/// load, and so are the removed records.
pub async fn a_save_drops_what_the_snapshot_lost(stores: &Stores) {
    let mut session = ActiveSession::new(Uuid::new_v4(), None, None, at(0));
    session.apply_record(record(1, "ab", &["A", "B"]));
    let bc = session.apply_record(record(2, "bc", &["B", "C"])).id;
    stores
        .sessions
        .save_session(&session, SessionChanges::everything(&session, &[]))
        .await
        .unwrap();
    // A load first, as a running system does before it changes a session.
    let mut session = stores
        .sessions
        .load_session(session.id())
        .await
        .unwrap()
        .session;
    assert_eq!(names(&session), ["A", "B", "C"]);

    let removed = session.remove_records(&[bc]);
    session.rebuild_entity_graph();
    stores
        .sessions
        .save_session(
            &session,
            SessionChanges {
                removed,
                whole_graph: true,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_stored(stores, &session).await;
    assert_eq!(names(&session), ["A", "B"]);
    assert_eq!(edges(&session), [("A".to_string(), "B".to_string())]);
}

/// The embedded record ids follow the additions and removals of each save.
pub async fn vectorized_ids_follow_the_changes(stores: &Stores) {
    let session = chain(stores).await;
    let ids: Vec<Uuid> = session.records().iter().map(|u| u.id).collect();
    let save = |vectorized: Vec<Uuid>, unvectorized: Vec<Uuid>| {
        let session = session.clone();
        async move {
            stores
                .sessions
                .save_session(
                    &session,
                    SessionChanges {
                        vectorized,
                        unvectorized,
                        ..Default::default()
                    },
                )
                .await
                .unwrap();
            let mut stored = stores
                .sessions
                .load_session(session.id())
                .await
                .unwrap()
                .vectorized;
            stored.sort();
            stored
        }
    };
    let mut both = vec![ids[0], ids[1]];
    both.sort();
    assert_eq!(save(vec![ids[0], ids[1]], vec![]).await, both);
    assert_eq!(save(vec![ids[2]], vec![ids[0]]).await, {
        let mut v = vec![ids[1], ids[2]];
        v.sort();
        v
    });
}
