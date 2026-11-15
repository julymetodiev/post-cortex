// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The `SearchIndex` conformance suite against every adapter the daemon
//! composes: the in-memory index over RocksDB, and over SurrealDB with its
//! native lexical ranking.

use std::sync::Arc;

use post_cortex_ports::SearchIndex;
use post_cortex_ports::conformance::index::DIMENSION;

use crate::compose::{Access, StorageConfig, open_storage, search_index};

async fn rocksdb() -> (Arc<dyn SearchIndex>, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let storage = StorageConfig::RocksDb {
        data_directory: dir.path().to_path_buf(),
    };
    let opened = open_storage(&storage, DIMENSION, Access::ReadWrite)
        .await
        .unwrap();
    (search_index(&opened, DIMENSION), dir)
}

#[cfg(feature = "surrealdb-storage")]
async fn surrealdb() -> (Arc<dyn SearchIndex>, ()) {
    let storage = StorageConfig::SurrealDb {
        endpoint: "mem://".to_string(),
        username: None,
        password: None,
        namespace: None,
        database: None,
    };
    let opened = open_storage(&storage, DIMENSION, Access::ReadWrite)
        .await
        .unwrap();
    (search_index(&opened, DIMENSION), ())
}

mod rocksdb_index {
    post_cortex_ports::index_conformance_suite!(crate::compose_tests::rocksdb());
}

#[cfg(feature = "surrealdb-storage")]
mod surrealdb_index {
    post_cortex_ports::index_conformance_suite!(crate::compose_tests::surrealdb());
}
