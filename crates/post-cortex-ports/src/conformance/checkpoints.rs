// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Checkpoints: listed per session, and deleted with their session (and
//! only its).

use crate::storage::Stores;
use std::collections::HashMap;

use post_cortex_core::core::structured_context::StructuredContext;
use post_cortex_core::session::active_session::ActiveSession;
use uuid::Uuid;

use crate::conformance::{at, record};
use crate::storage::SessionCheckpoint;

/// A checkpoint of `session_id` with one record.
pub fn checkpoint(session_id: Uuid) -> SessionCheckpoint {
    SessionCheckpoint {
        id: Uuid::new_v4(),
        session_id,
        created_at: at(60),
        structured_context: StructuredContext::default(),
        recent_updates: vec![record(1, "kept", &["A", "B"])],
        code_references: HashMap::new(),
        change_history: Vec::new(),
        total_updates: 1,
        context_quality_score: 1.0,
        compression_ratio: 1.0,
    }
}

async fn checkpoint_ids(stores: &Stores, session_id: Uuid) -> Vec<Uuid> {
    stores
        .checkpoints
        .list_checkpoints(session_id)
        .await
        .unwrap()
        .into_iter()
        .map(|c| c.id)
        .collect()
}

/// A checkpoint lists back as saved, under its session only, by id.
pub async fn checkpoint_round_trip(stores: &Stores) {
    let (sid, other) = (Uuid::new_v4(), Uuid::new_v4());
    let (c, d) = (checkpoint(sid), checkpoint(sid));
    for x in [&c, &d, &checkpoint(other)] {
        stores.checkpoints.save_checkpoint(x).await.unwrap();
    }
    let listed = stores.checkpoints.list_checkpoints(sid).await.unwrap();
    let mut ids = vec![c.id, d.id];
    ids.sort();
    assert_eq!(listed.iter().map(|x| x.id).collect::<Vec<_>>(), ids);
    let loaded = listed.iter().find(|x| x.id == c.id).unwrap();
    assert_eq!(
        (loaded.session_id, loaded.created_at, loaded.total_updates),
        (c.session_id, c.created_at, 1)
    );
    assert_eq!(
        serde_json::to_value(&loaded.recent_updates).unwrap(),
        serde_json::to_value(&c.recent_updates).unwrap()
    );
    assert!(checkpoint_ids(stores, Uuid::new_v4()).await.is_empty());
}

/// Deleting a session deletes its checkpoints and only its own.
pub async fn deleting_a_session_deletes_its_checkpoints(stores: &Stores) {
    let (gone, kept) = (Uuid::new_v4(), Uuid::new_v4());
    for sid in [gone, kept] {
        stores
            .sessions
            .save_session(
                &ActiveSession::new(sid, None, None, at(0)),
                crate::storage::SessionChanges {
                    whole_graph: true,
                    ..Default::default()
                },
            )
            .await
            .unwrap();
    }
    let other = checkpoint(kept);
    for c in [&checkpoint(gone), &checkpoint(gone), &other] {
        stores.checkpoints.save_checkpoint(c).await.unwrap();
    }

    stores.sessions.delete_session(gone).await.unwrap();

    assert!(checkpoint_ids(stores, gone).await.is_empty());
    assert_eq!(checkpoint_ids(stores, kept).await, [other.id]);
}
