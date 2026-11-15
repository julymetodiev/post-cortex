// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use post_cortex_core::core::blob::MediaType;
use post_cortex_core::core::document::{DocumentData, DocumentKind, KindDeclaration};
use post_cortex_ports::KindsError;
use prost::Message;

use super::*;
use crate::capabilities::test_support::rocksdb_memory;
use crate::capabilities::{CreateWorkspace, Memory, PutDocument, Writer};

/// The board's kinds, as its manifest declares them.
struct Board;

#[async_trait::async_trait]
impl DocumentKinds for Board {
    async fn kinds(&self) -> Result<Vec<KindDeclaration>, KindsError> {
        Ok(vec![KindDeclaration {
            kind: DocumentKind::new("board", "task").unwrap(),
            type_name: "pcx.board.v1.Task".to_string(),
            title_field: "title".to_string(),
            searchable: true,
            descriptors: post_cortex_proto::BOARD_DESCRIPTOR_SET.to_vec(),
        }])
    }
}

/// A `width`×`height` PNG's first bytes (signature and header), and `seed`
/// so each is another blob.
fn png(width: u32, height: u32, seed: u8) -> Vec<u8> {
    let mut bytes = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 13];
    bytes.extend_from_slice(b"IHDR");
    bytes.extend_from_slice(&width.to_be_bytes());
    bytes.extend_from_slice(&height.to_be_bytes());
    bytes.extend_from_slice(&[8, 6, 0, 0, 0, seed]);
    bytes
}

async fn workspace(memory: &Memory) -> Uuid {
    memory
        .workspaces
        .create(CreateWorkspace {
            name: "blobs".into(),
            description: String::new(),
        })
        .await
        .unwrap()
        .id
}

/// A task naming `blobs`.
fn task(workspace_id: Uuid, blobs: &[&BlobInfo]) -> PutDocument {
    let task = post_cortex_proto::board::Task {
        title: "with images".into(),
        attachments: blobs
            .iter()
            .map(|b| post_cortex_proto::pb::Attachment {
                sha256: b.id.to_string(),
                name: "shot.png".into(),
                media_type: b.image.media_type.as_str().into(),
                size: b.size,
                width: b.image.width,
                height: b.image.height,
            })
            .collect(),
        ..Default::default()
    };
    PutDocument {
        workspace_id,
        id: None,
        kind: "board/task".parse().unwrap(),
        data: DocumentData::Message {
            type_name: "pcx.board.v1.Task".into(),
            bytes: task.encode_to_vec(),
        },
        text: String::new(),
        author: None,
        expected_version: 0,
    }
}

fn board() -> Writer<'static> {
    Writer {
        module: Some("board"),
        kinds: &Board,
        reach: None,
    }
}

#[tokio::test]
async fn an_image_is_put_once_by_its_sha256() {
    let (memory, _dir) = rocksdb_memory().await;
    let workspace_id = workspace(&memory).await;
    let put = memory
        .blobs
        .put(workspace_id, png(640, 480, 1), "")
        .await
        .unwrap();
    assert_eq!(
        (put.image.media_type, put.image.width, put.image.height),
        (MediaType::Png, 640, 480)
    );
    let again = memory
        .blobs
        .put(workspace_id, png(640, 480, 1), "")
        .await
        .unwrap();
    assert_eq!(again, put);
    assert_eq!(memory.blobs.stat(workspace_id, &put.id).await.unwrap(), put);
    assert_eq!(
        memory.blobs.get(workspace_id, &put.id).await.unwrap().bytes,
        png(640, 480, 1)
    );
    assert!(matches!(
        memory.blobs.put(workspace_id, b"<svg/>".to_vec(), "").await,
        Err(BlobError::Refused(BlobRefused::NotAnImage))
    ));
    assert!(matches!(
        memory.blobs.put(Uuid::new_v4(), png(1, 1, 1), "").await,
        Err(BlobError::WorkspaceNotFound(_))
    ));
}

#[tokio::test]
async fn a_document_names_only_blobs_its_workspace_has_and_the_rest_are_swept() {
    let (memory, _dir) = rocksdb_memory().await;
    let workspace_id = workspace(&memory).await;
    let named = memory
        .blobs
        .put(workspace_id, png(2, 2, 1), "")
        .await
        .unwrap();
    let loose = memory
        .blobs
        .put(workspace_id, png(3, 3, 2), "")
        .await
        .unwrap();
    let elsewhere = BlobInfo {
        id: BlobId::of_digest(&[7; 32]),
        ..named.clone()
    };
    let refused = memory
        .documents
        .put(task(workspace_id, &[&named, &elsewhere]), board())
        .await
        .unwrap_err();
    assert!(
        matches!(&refused, DocumentError::InvalidArgument(m) if m.contains("no blob")),
        "{refused}"
    );
    memory
        .documents
        .put(task(workspace_id, &[&named]), board())
        .await
        .unwrap();

    // Nothing put before the cutoff: nothing is swept yet.
    let before_them = named.put_at - chrono::Duration::seconds(1);
    let swept = memory.blobs.sweep(before_them, &Board).await.unwrap();
    assert_eq!(swept.deleted, 0);
    let swept = memory.blobs.sweep(Utc::now(), &Board).await.unwrap();
    assert_eq!(
        swept,
        Swept {
            deleted: 1,
            skipped: 0
        }
    );
    assert!(memory.blobs.stat(workspace_id, &named.id).await.is_ok());
    assert!(matches!(
        memory.blobs.stat(workspace_id, &loose.id).await,
        Err(BlobError::NotFound(_))
    ));

    memory.workspaces.delete(workspace_id).await.unwrap();
    let recreated = workspace(&memory).await;
    assert!(matches!(
        memory.blobs.stat(recreated, &named.id).await,
        Err(BlobError::NotFound(_))
    ));
}
