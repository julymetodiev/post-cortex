// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The blob port's conformance suite over the object store adapter, in
//! memory (the S3 client is the object store's; staging runs it on S3).

async fn open() -> post_cortex_blobs::ObjectBlobs {
    post_cortex_blobs::ObjectBlobs::in_memory()
}

mod in_memory {
    post_cortex_ports::blob_conformance_suite!(crate::open());
}
