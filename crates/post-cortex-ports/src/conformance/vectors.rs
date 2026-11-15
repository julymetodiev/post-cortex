// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The vector port: an upsert keeps one vector per (session, content id),
//! and a batched delete removes only the named session's vectors.

use crate::vectors::VectorMetadata;
use uuid::Uuid;

use crate::conformance::Adapter;
use crate::conformance::at;

fn vector(content_id: &str, session: &str, x: f32) -> (Vec<f32>, VectorMetadata) {
    let mut v = vec![0.0; 16];
    v[0] = x;
    (
        v,
        VectorMetadata {
            id: content_id.into(),
            text: format!("text {content_id}").into(),
            source: session.into(),
            content_type: "UpdateContent".into(),
            timestamp: at(1),
            metadata: Default::default(),
        },
    )
}

/// Storing a content again replaces its vector; another session's vector
/// under the same content id is its own.
pub async fn upsert_keeps_one_vector_per_content(adapter: &Adapter) {
    let (a, b) = (Uuid::new_v4().to_string(), Uuid::new_v4().to_string());
    adapter
        .vectors
        .upsert_vectors(vec![vector("entity:x", &a, 1.0), vector("u1", &a, 1.0)])
        .await
        .unwrap();
    adapter
        .vectors
        .upsert_vectors(vec![
            vector("entity:x", &a, 0.5),
            vector("entity:x", &b, 0.2),
        ])
        .await
        .unwrap();
    assert_eq!(
        adapter.vectors.count_session_embeddings(&a).await.unwrap(),
        2
    );
    assert_eq!(adapter.vectors.total_count().await.unwrap(), 3);
    let stored = adapter.vectors.get_session_vectors(&a).await.unwrap();
    let x = stored.iter().find(|(_, m)| m.id == "entity:x").unwrap();
    assert_eq!(x.0[0], 0.5, "the later vector replaces the first");
}

/// One call deletes many content ids of one session and reports how many
/// were stored; the other session keeps its copies.
pub async fn batched_delete_is_scoped_to_the_session(adapter: &Adapter) {
    let (a, b) = (Uuid::new_v4().to_string(), Uuid::new_v4().to_string());
    adapter
        .vectors
        .upsert_vectors(vec![
            vector("u1", &a, 1.0),
            vector("u1#2", &a, 1.0),
            vector("u2", &a, 1.0),
            vector("u1", &b, 1.0),
        ])
        .await
        .unwrap();
    let ids = ["u1", "u1#2", "missing"].map(String::from);
    assert_eq!(adapter.vectors.delete_vectors(&a, &ids).await.unwrap(), 2);
    let left: Vec<String> = adapter
        .vectors
        .get_session_vectors(&a)
        .await
        .unwrap()
        .into_iter()
        .map(|(_, m)| m.id)
        .collect();
    assert_eq!(left, ["u2"]);
    assert_eq!(
        adapter.vectors.count_session_embeddings(&b).await.unwrap(),
        1
    );
    assert_eq!(adapter.vectors.delete_vectors(&a, &[]).await.unwrap(), 0);
}
