// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Migration from data version 2 (session blob, rows by update id, update
//! index) to the rows of version 3.

use super::*;

/// A version 2 session (blob with graph and vectorized ids, rows by update
/// id, index) migrates to version 3 and loads the same: records in order,
/// graph, vectorized ids.
#[tokio::test]
async fn version_2_sessions_migrate_to_rows() {
    let id = Uuid::new_v4();
    let mut session = ActiveSession::new(
        id,
        Some("v2".into()),
        None,
        post_cortex_ports::conformance::at(0),
    );
    for (i, title) in ["a", "b", "c"].iter().enumerate() {
        session.apply_record(post_cortex_ports::conformance::record(
            i as i64,
            title,
            &["X", "Y"],
        ));
    }
    let vectorized = vec![session.records()[1].id];
    let dir = tempdir().unwrap();
    {
        let storage = open(dir.path()).await;
        write_version_2(&storage, &session, &vectorized);
        storage.db.put(DATA_VERSION_KEY, b"2").unwrap();
    }
    let storage = open(dir.path()).await;
    assert_eq!(data_version(&storage.db).unwrap(), DATA_VERSION);
    assert_all_marked(&storage);
    let loaded = storage.load_session(id).await.unwrap();
    assert_eq!(titles(&loaded.session), ["a", "b", "c"]);
    assert_eq!(loaded.vectorized, vectorized);
    assert_eq!(
        post_cortex_ports::conformance::sessions::graph_view(loaded.session.entity_graph()),
        post_cortex_ports::conformance::sessions::graph_view(session.entity_graph())
    );
}
