// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Blobs: kept by id per workspace, written once, listed and deleted. Run
//! against a blob adapter with [`blob_conformance_suite!`](crate::blob_conformance_suite).
//! Each case deletes what it put, so it runs on a real store too.

use chrono::{DateTime, Duration, Utc};

use crate::conformance::at as second;
use uuid::Uuid;

use post_cortex_core::core::blob::{BlobId, BlobInfo, Image, MediaType};

use crate::blobs::{Blob, BlobStore};

/// A blob of `bytes`, put at `at` (its id is made up: a store does not
/// hash).
fn blob(seed: u8, bytes: &[u8], at: DateTime<Utc>) -> Blob {
    Blob {
        info: BlobInfo {
            id: BlobId::of_digest(&[seed; 32]),
            size: bytes.len() as u64,
            image: Image {
                media_type: MediaType::Webp,
                width: 640,
                height: 480,
            },
            put_at: at,
        },
        bytes: bytes.to_vec(),
    }
}

/// A blob put is read back whole, and its info stays.
pub async fn a_blob_round_trips(store: &dyn BlobStore) {
    let workspace = Uuid::new_v4();
    let put = blob(1, &[0, 1, 2, 127, 128, 255], second(0));
    let kept = store.put_blob(workspace, put.clone()).await.unwrap();
    assert_eq!(kept, put.info);
    assert_eq!(
        store.get_blob(workspace, &put.info.id).await.unwrap(),
        Some(put.clone())
    );
    assert_eq!(
        store.stat_blob(workspace, &put.info.id).await.unwrap(),
        Some(put.info.clone())
    );
    assert!(store.delete_blob(workspace, &put.info.id).await.unwrap());
}

/// A blob put again is not written again: the first one stays.
pub async fn a_blob_is_written_once(store: &dyn BlobStore) {
    let workspace = Uuid::new_v4();
    let first = blob(2, b"same", second(0));
    store.put_blob(workspace, first.clone()).await.unwrap();
    let again = blob(2, b"same", second(0) + Duration::hours(1));
    let kept = store.put_blob(workspace, again).await.unwrap();
    assert_eq!(kept.put_at, first.info.put_at);
    assert!(store.delete_blob(workspace, &first.info.id).await.unwrap());
}

/// One workspace's blobs are not another's; they are listed and deleted.
pub async fn blobs_are_per_workspace(store: &dyn BlobStore) {
    let (one, other) = (Uuid::new_v4(), Uuid::new_v4());
    let a = blob(3, b"a", second(0));
    let b = blob(4, b"bb", second(1));
    store.put_blob(one, a.clone()).await.unwrap();
    store.put_blob(one, b.clone()).await.unwrap();
    assert_eq!(store.stat_blob(other, &a.info.id).await.unwrap(), None);
    assert!(store.list_blobs(other).await.unwrap().is_empty());
    let mut listed = store.list_blobs(one).await.unwrap();
    listed.sort_by(|x, y| x.id.cmp(&y.id));
    assert_eq!(listed, [a.info.clone(), b.info.clone()]);
    assert!(store.delete_blob(one, &a.info.id).await.unwrap());
    assert!(!store.delete_blob(one, &a.info.id).await.unwrap());
    assert_eq!(store.get_blob(one, &a.info.id).await.unwrap(), None);
    assert_eq!(
        store.list_blobs(one).await.unwrap(),
        std::slice::from_ref(&b.info)
    );
    assert!(store.delete_blob(one, &b.info.id).await.unwrap());
}

/// One `#[tokio::test]` per case of the blob suite, run against the
/// adapter that `$open` (an async expression with `crate::` paths,
/// evaluating to the adapter) opens afresh for each case.
#[macro_export]
macro_rules! blob_conformance_suite {
    ($open:expr) => {
        $crate::blob_conformance_cases!(
            $open;
            a_blob_round_trips, a_blob_is_written_once, blobs_are_per_workspace
        );
    };
}

/// The expansion of [`blob_conformance_suite!`].
#[doc(hidden)]
#[macro_export]
macro_rules! blob_conformance_cases {
    ($open:expr; $($case:ident),+) => {
        mod blobs {
            $(
                #[tokio::test(flavor = "multi_thread")]
                async fn $case() {
                    let store = $open.await;
                    $crate::conformance::blobs::$case(&store).await;
                }
            )+
        }
    };
}
