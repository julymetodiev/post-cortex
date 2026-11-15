// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! An archive records the embedding model of its vectors; an import into a
//! store of another model is refused before anything is written, and a
//! preview warns.

use crate::capabilities::transfer::model::ArchivedVectors;
use crate::capabilities::transfer::tests::{export, import, of_dim, preview_of};
use crate::capabilities::transfer::*;
use post_cortex_core::archive::*;
use post_cortex_core::session::active_session::ActiveSession;
use post_cortex_ports::storage::{SessionStore, VectorStore};
use post_cortex_storage::RealRocksDBStorage;
use uuid::Uuid;

fn model(name: &str, dimension: usize) -> EmbeddingModelInfo {
    EmbeddingModelInfo {
        model: name.into(),
        dimension,
    }
}

fn embedding(session: Uuid, dimension: usize) -> ExportedEmbedding {
    ExportedEmbedding {
        content_id: Uuid::new_v4().to_string(),
        session_id: session.to_string(),
        vector: vec![0.5; dimension],
        text: "text".into(),
        content_type: "UpdateContent".into(),
        timestamp: chrono::Utc::now().to_rfc3339(),
        metadata: Default::default(),
    }
}

async fn stores(dir: &tempfile::TempDir) -> RealRocksDBStorage {
    RealRocksDBStorage::new(dir.path()).await.unwrap()
}

#[tokio::test]
async fn an_archive_of_another_model_is_refused_and_writes_nothing() {
    let (source_dir, target_dir) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let source = stores(&source_dir).await;
    let session = ActiveSession::new(Uuid::new_v4(), Some("s".into()), None, chrono::Utc::now());
    source
        .save_session(
            &session,
            post_cortex_ports::storage::SessionChanges::everything(&session, &[]),
        )
        .await
        .unwrap();
    let mut archive = export(
        &of_dim(&source, 512),
        &ExportScope::All,
        &ExportOptions {
            embedding_model: Some(model("PotionCode", 512)),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(archive.format_version, "1.10.0");
    assert_eq!(
        archive.metadata.embedding_model,
        Some(model("PotionCode", 512))
    );
    archive.embeddings.push(embedding(session.id(), 512));

    let target = stores(&target_dir).await;
    let other = ImportOptions {
        embedding_model: Some(model("PotionMultilingual", 256)),
        ..Default::default()
    };
    let preview = preview_of(&of_dim(&target, 512), &archive, &other)
        .await
        .unwrap();
    assert_eq!(preview.warnings.len(), 1, "{:?}", preview.warnings);
    let err = import(&of_dim(&target, 512), archive.clone(), &other)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("PotionCode"), "{err}");
    assert!(target.list_sessions().await.unwrap().is_empty());
    assert_eq!(target.get_all_vectors().await.unwrap().len(), 0);

    // The same model imports.
    let same = ImportOptions {
        embedding_model: Some(model("PotionCode", 512)),
        ..Default::default()
    };
    assert!(
        preview_of(&of_dim(&target, 512), &archive, &same)
            .await
            .unwrap()
            .warnings
            .is_empty()
    );
    import(&of_dim(&target, 512), archive, &same).await.unwrap();
    assert_eq!(target.list_sessions().await.unwrap().len(), 1);
}

/// An archive written before the model was recorded is compared by the
/// length of its vectors; one without vectors fits any store.
#[test]
fn older_archives_are_compared_by_dimension() {
    let session = Uuid::new_v4();
    let target = model("PotionMultilingual", 256);
    let mut archived = ArchivedVectors::default();
    assert_eq!(model::mismatch(&archived, Some(&target)), None);
    archived.add(&embedding(session, 384));
    assert!(model::mismatch(&archived, Some(&target)).is_some());
    let mut fitting = ArchivedVectors::default();
    fitting.add(&embedding(session, 256));
    assert_eq!(model::mismatch(&fitting, Some(&target)), None);
    assert_eq!(model::mismatch(&archived, None), None, "unknown target");
}
