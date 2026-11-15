// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The port conformance suites of `post-cortex-ports`, run against each
//! adapter of this crate (RocksDB in a temporary directory, SurrealDB
//! `mem://`): `conformance::<adapter>::<port>::<case>`.

use std::sync::Arc;

use post_cortex_ports::conformance::Adapter;

/// The RocksDB adapter's ports over a fresh temporary directory, with the
/// directory that keeps it alive.
pub(crate) async fn rocksdb() -> (Adapter, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let storage = crate::RealRocksDBStorage::new(dir.path()).await.unwrap();
    (Adapter::of(Arc::new(storage)), dir)
}

/// The SurrealDB adapter's ports over a fresh `mem://` database.
#[cfg(feature = "surrealdb-storage")]
pub(crate) async fn surrealdb() -> (Adapter, ()) {
    let storage = crate::SurrealDBStorage::new("mem://", None, None, None, None)
        .await
        .unwrap();
    (Adapter::of(Arc::new(storage)), ())
}

mod rocksdb {
    post_cortex_ports::conformance_suite!(crate::conformance::rocksdb());
}

#[cfg(feature = "surrealdb-storage")]
mod surrealdb {
    post_cortex_ports::conformance_suite!(crate::conformance::surrealdb());
}
