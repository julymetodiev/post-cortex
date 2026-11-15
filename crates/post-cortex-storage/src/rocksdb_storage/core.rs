// Copyright (c) 2025, 2026 Julius ML
//
// Permission is hereby granted, free of charge, to any person obtaining a copy
// of this software and associated documentation files (the "Software"), to deal
// in the Software without restriction, including without limitation the rights
// to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
// copies of the Software, and to permit persons to whom the Software is
// furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in all
// copies or substantial portions of the Software.

//! Constructor and global database utilities for [`RealRocksDBStorage`].

use crate::port_error::Result;
use parking_lot::{Mutex, MutexGuard};
use rocksdb::{
    BlockBasedOptions, Cache, DB, DBCompressionType, DBIteratorWithThreadMode, Direction,
    IteratorMode, Options, ReadOptions, SnapshotWithThreadMode,
};
use std::path::Path;
use std::sync::Arc;
use tracing::info;
use uuid::Uuid;

use crate::rocksdb_storage::RealRocksDBStorage;

/// Block cache shared by all SST readers (data, index and filter blocks).
const BLOCK_CACHE_BYTES: usize = 64 * 1024 * 1024;

/// Number of stripes in [`SessionLocks`].
const SESSION_LOCK_STRIPES: usize = 64;

/// Striped mutexes keyed by session id.
///
/// Held (inside `spawn_blocking`) around read-modify-write sequences on one
/// session's records, e.g. diffing the stored update index against a new
/// snapshot. Different sessions rarely share a stripe.
pub struct SessionLocks {
    stripes: Vec<Mutex<()>>,
}

impl SessionLocks {
    fn new() -> Self {
        Self {
            stripes: (0..SESSION_LOCK_STRIPES).map(|_| Mutex::new(())).collect(),
        }
    }

    /// Lock the stripe that owns `session_id`.
    pub fn lock(&self, session_id: Uuid) -> MutexGuard<'_, ()> {
        let stripe = (session_id.as_u128() % SESSION_LOCK_STRIPES as u128) as usize;
        self.stripes[stripe].lock()
    }
}

/// Smallest key greater than every key that starts with `prefix`, or `None`
/// when no such bound exists (the prefix is all `0xFF`).
pub fn prefix_upper_bound(prefix: &[u8]) -> Option<Vec<u8>> {
    let mut bound = prefix.to_vec();
    while let Some(last) = bound.pop() {
        if last < u8::MAX {
            bound.push(last + 1);
            return Some(bound);
        }
    }
    None
}

/// Read options for a scan over every key starting with `prefix`.
///
/// Total-order seek because the 16-byte prefix extractor (and the prefix
/// entries in the bloom filters) only cover keys that share the seek key's
/// first 16 bytes, while many scans here cross that boundary. The upper bound
/// stops the iterator at the end of the prefix instead of reading on.
pub fn prefix_read_opts(prefix: &[u8]) -> ReadOptions {
    let mut opts = ReadOptions::default();
    opts.set_total_order_seek(true);
    if let Some(upper) = prefix_upper_bound(prefix) {
        opts.set_iterate_upper_bound(upper);
    }
    opts
}

/// Iterate over all `(key, value)` pairs whose key starts with `prefix`, in
/// key order.
pub fn scan_prefix<'a>(db: &'a DB, prefix: &[u8]) -> DBIteratorWithThreadMode<'a, DB> {
    db.iterator_opt(
        IteratorMode::From(prefix, Direction::Forward),
        prefix_read_opts(prefix),
    )
}

/// [`scan_prefix`] in `snapshot`.
pub fn scan_snapshot_prefix<'a>(
    snapshot: &'a SnapshotWithThreadMode<'a, DB>,
    prefix: &[u8],
) -> DBIteratorWithThreadMode<'a, DB> {
    snapshot.iterator_opt(
        IteratorMode::From(prefix, Direction::Forward),
        prefix_read_opts(prefix),
    )
}

/// Collect the keys of every record starting with `prefix`.
pub fn keys_with_prefix(db: &DB, prefix: &[u8]) -> Result<Vec<Box<[u8]>>> {
    let mut keys = Vec::new();
    for item in scan_prefix(db, prefix) {
        let (key, _) = item?;
        keys.push(key);
    }
    Ok(keys)
}

/// Run blocking RocksDB work off the async runtime.
pub async fn blocking<T, F>(f: F) -> Result<T>
where
    F: FnOnce() -> Result<T> + Send + 'static,
    T: Send + 'static,
{
    tokio::task::spawn_blocking(f).await?
}

impl RealRocksDBStorage {
    /// Open (or create) RocksDB storage that accepts embeddings of any
    /// non-empty length.
    ///
    /// Use [`Self::new_with_dimension`] to reject vectors whose length does
    /// not match the embedding model.
    pub async fn new<P: AsRef<Path>>(path: P) -> Result<Self> {
        Self::open(path.as_ref(), None).await
    }

    /// Open (or create) RocksDB storage that only accepts embeddings of
    /// exactly `vector_dimension` floats, mirroring
    /// `SurrealDBStorage::new_with_dimension`. `0` behaves like [`Self::new`].
    pub async fn new_with_dimension<P: AsRef<Path>>(
        path: P,
        vector_dimension: usize,
    ) -> Result<Self> {
        let dimension = (vector_dimension > 0).then_some(vector_dimension);
        Self::open(path.as_ref(), dimension).await
    }

    /// Open the database in `path` to read it while another process (the
    /// daemon) may hold it open for writing: no lock is taken, nothing is
    /// migrated or written. A database that still needs a migration is
    /// refused (a write open, such as starting the daemon, migrates it).
    ///
    /// # Errors
    /// The database could not be opened or is at another data version.
    pub async fn open_read_only<P: AsRef<Path>>(path: P, vector_dimension: usize) -> Result<Self> {
        let db_path = path.as_ref().join("rocksdb");
        let db = DB::open_for_read_only(&db_options(), &db_path, false)?;
        let version = crate::rocksdb_storage::migrate::data_version(&db)?;
        if version != crate::rocksdb_storage::migrate::DATA_VERSION {
            return Err(post_cortex_ports::PortError::unavailable(format!(
                "the database at {} is at data version {version}, this build reads {}: \
                 open it once for writing (start the daemon) to migrate it",
                db_path.display(),
                crate::rocksdb_storage::migrate::DATA_VERSION
            ))
            .into());
        }
        info!(
            "RealRocksDBStorage: Opened RocksDB at {} read-only",
            db_path.display()
        );
        Ok(Self {
            db: Arc::new(db),
            vector_dimension: (vector_dimension > 0).then_some(vector_dimension),
            session_locks: Arc::new(SessionLocks::new()),
            bus_lock: Arc::default(),
            breaker: Arc::default(),
        })
    }

    async fn open(path: &Path, vector_dimension: Option<usize>) -> Result<Self> {
        std::fs::create_dir_all(path)?;
        let db_path = path.join("rocksdb");
        let db = DB::open(&db_options(), &db_path)?;
        let db = Arc::new(db);

        info!(
            "RealRocksDBStorage: Initialized RocksDB at {}",
            db_path.display()
        );

        let storage = Self {
            db,
            vector_dimension,
            session_locks: Arc::new(SessionLocks::new()),
            bus_lock: Arc::default(),
            breaker: Arc::default(),
        };
        let db = storage.db.clone();
        blocking(move || crate::rocksdb_storage::migrate::migrate(&db)).await?;
        Ok(storage)
    }
}

/// RocksDB options for balanced performance and safety.
fn db_options() -> Options {
    let mut opts = Options::default();
    opts.create_if_missing(true);
    opts.create_missing_column_families(true);

    // Memory settings - balanced for daemon use (max ~512MB for write buffers)
    opts.set_max_open_files(1000); // Reduced from 10000 - most systems have lower limits
    opts.set_write_buffer_size(64 * 1024 * 1024); // 64MB per buffer (was 512MB)
    opts.set_max_write_buffer_number(6); // Max 384MB for write buffers (was 32 = 16GB!)
    opts.set_min_write_buffer_number_to_merge(2); // Merge sooner (was 4)
    opts.set_target_file_size_base(64 * 1024 * 1024); // 64MB SST files (was 1GB)

    // Write throttling - prevent memory exhaustion
    opts.set_level_zero_slowdown_writes_trigger(20); // Enable throttling (was 0 = disabled)
    opts.set_level_zero_stop_writes_trigger(36); // Reduced from 2000

    // Durability settings - balanced for daemon use
    opts.set_use_fsync(false); // fdatasync is sufficient for most cases
    opts.set_bytes_per_sync(1024 * 1024); // Sync every 1MB (was 0 = never)

    // Compaction settings. The bottommost level holds most of the data
    // and is rewritten least often, so it gets the stronger codec; SST
    // files record their own codec, so existing files stay readable.
    opts.set_compaction_style(rocksdb::DBCompactionStyle::Universal);
    opts.set_bottommost_compression_type(DBCompressionType::Zstd);

    // Fixed 16-byte prefix extractor (kept for on-disk compatibility).
    // Scans use total-order seek with an upper bound (`scan_prefix`), so
    // they are correct for any prefix length.
    opts.set_prefix_extractor(rocksdb::SliceTransform::create_fixed_prefix(16));

    // Table format: full-key bloom filters let point lookups (sessions,
    // update rows, source refs, ...) skip SST files without the key, and
    // a bounded LRU block cache holds data, index and filter blocks.
    let mut table_opts = BlockBasedOptions::default();
    table_opts.set_bloom_filter(10.0, false);
    table_opts.set_whole_key_filtering(true);
    let cache = Cache::new_lru_cache(BLOCK_CACHE_BYTES);
    table_opts.set_block_cache(&cache);
    table_opts.set_cache_index_and_filter_blocks(true);
    table_opts.set_pin_l0_filter_and_index_blocks_in_cache(true);
    opts.set_block_based_table_factory(&table_opts);
    opts
}

#[cfg(test)]
mod tests {
    use super::prefix_upper_bound;

    #[test]
    fn upper_bound_increments_last_byte() {
        assert_eq!(prefix_upper_bound(b"session:"), Some(b"session;".to_vec()));
        assert_eq!(prefix_upper_bound(b"a\xff"), Some(b"b".to_vec()));
        assert_eq!(prefix_upper_bound(b"\xff\xff"), None);
        assert_eq!(prefix_upper_bound(b""), None);
    }
}
