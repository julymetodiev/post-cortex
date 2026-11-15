// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The blob port's conformance suite on a real S3 endpoint, run by hand
//! (`cargo test -p post-cortex-blobs --test s3 -- --ignored`) with the
//! endpoint and keys in `PCX_BLOBS_ENDPOINT`, `PCX_BLOBS_BUCKET`,
//! `PCX_BLOBS_ACCESS_KEY` and `PCX_BLOBS_SECRET_KEY` (`PCX_BLOBS_REGION`:
//! `us-east-1` unless set; `PCX_BLOBS_PREFIX`: none unless set). Each case works in a new workspace and deletes
//! what it put.

use post_cortex_blobs::{ObjectBlobs, S3Config};

fn var(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("{name} is not set"))
}

async fn open() -> ObjectBlobs {
    ObjectBlobs::s3(&S3Config {
        endpoint: var("PCX_BLOBS_ENDPOINT"),
        bucket: var("PCX_BLOBS_BUCKET"),
        region: std::env::var("PCX_BLOBS_REGION").unwrap_or_else(|_| "us-east-1".into()),
        access_key: var("PCX_BLOBS_ACCESS_KEY"),
        secret_key: var("PCX_BLOBS_SECRET_KEY"),
        prefix: std::env::var("PCX_BLOBS_PREFIX").unwrap_or_default(),
    })
    .unwrap()
}

macro_rules! on_s3 {
    ($($case:ident),+) => {
        $(
            #[tokio::test(flavor = "multi_thread")]
            #[ignore = "needs an S3 endpoint"]
            async fn $case() {
                let store = open().await;
                post_cortex_ports::conformance::blobs::$case(&store).await;
            }
        )+
    };
}

on_s3!(
    a_blob_round_trips,
    a_blob_is_written_once,
    blobs_are_per_workspace
);
