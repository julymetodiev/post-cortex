// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use super::*;

#[test]
fn names_round_trip_and_unknown_names_are_errors() {
    for backend in StorageBackendType::ALL {
        assert_eq!(backend.as_str().parse::<StorageBackendType>(), Ok(*backend));
    }
    assert_eq!("Rocks".parse(), Ok(StorageBackendType::RocksDB));
    let err = "rocksbd".parse::<StorageBackendType>().unwrap_err();
    assert_eq!(err.name, "rocksbd");
    assert!(err.to_string().contains("rocksdb"), "{err}");
}
