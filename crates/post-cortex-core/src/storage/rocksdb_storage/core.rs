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

use anyhow::Result;
use rocksdb::{DB, Options};
use std::path::Path;
use std::sync::Arc;
use tracing::{debug, info};

use crate::core::vector_db::{VectorDB, VectorDbConfig};

use super::RealRocksDBStorage;
use super::types::EMBEDDING_DIMENSION;

impl RealRocksDBStorage {
    /// Create new RocksDB storage instance
    pub async fn new<P: AsRef<Path>>(path: P) -> Result<Self> {
        let data_dir = path.as_ref().to_path_buf();

        // Create directory if it doesn't exist
        if !data_dir.exists() {
            std::fs::create_dir_all(&data_dir)?;
        }

        let db_path = data_dir.join("rocksdb");

        // Configure RocksDB options for balanced performance and safety
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

        // Compaction settings
        opts.set_compaction_style(rocksdb::DBCompactionStyle::Universal);

        // Enable prefix bloom filter (16 bytes)
        // NOTE: prefix_iterator() only works correctly when the search prefix is >= 16 bytes.
        // For shorter prefixes like "session:" (8 bytes), use iterator() with IteratorMode::From instead.
        opts.set_prefix_extractor(rocksdb::SliceTransform::create_fixed_prefix(16));

        let db = DB::open(&opts, &db_path)?;
        let db = Arc::new(db);

        info!(
            "RealRocksDBStorage: Initialized RocksDB at {}",
            db_path.display()
        );

        // Create in-memory HNSW index for fast vector search
        let vector_config = VectorDbConfig {
            dimension: EMBEDDING_DIMENSION,
            enable_hnsw_index: true,
            max_connections: 16,
            num_layers: 4,
            ..Default::default()
        };
        let vector_index = Arc::new(VectorDB::new(vector_config)?);

        let storage = Self {
            db: db.clone(),
            data_dir,
            vector_index,
        };

        // Load existing embeddings from RocksDB into the in-memory HNSW index
        storage.rebuild_hnsw_index().await?;

        Ok(storage)
    }

    /// Rebuild the in-memory HNSW index from RocksDB embeddings using batch insertion
    async fn rebuild_hnsw_index(&self) -> Result<()> {
        let embeddings = self.load_all_embeddings().await?;
        let count = embeddings.len();

        if count == 0 {
            info!("RealRocksDBStorage: No embeddings to load into HNSW index");
            return Ok(());
        }

        info!(
            "RealRocksDBStorage: Loading {} embeddings into HNSW index...",
            count
        );

        let start = std::time::Instant::now();
        const BATCH_SIZE: usize = 100;
        let mut loaded = 0;

        for chunk in embeddings.chunks(BATCH_SIZE) {
            let batch: Vec<(Vec<f32>, _)> = chunk
                .iter()
                .map(|e| (e.vector.clone(), e.to_metadata()))
                .collect();

            match self.vector_index.add_vectors_batch(batch) {
                Ok(ids) => loaded += ids.len(),
                Err(e) => {
                    tracing::warn!("Failed to add embedding batch to HNSW index: {}", e);
                }
            }

            if loaded % 500 == 0 && loaded > 0 {
                debug!(
                    "RealRocksDBStorage: HNSW rebuild progress: {}/{} vectors",
                    loaded, count
                );
            }
        }

        let elapsed = start.elapsed();
        info!(
            "RealRocksDBStorage: HNSW index ready with {} vectors (rebuilt in {:.1}ms)",
            self.vector_index.len(),
            elapsed.as_secs_f64() * 1000.0
        );

        Ok(())
    }

    /// Get database statistics
    pub async fn get_stats(&self) -> Result<String> {
        let db = self.db.clone();

        tokio::task::spawn_blocking(move || -> Result<String> {
            let stats = db
                .property_value(rocksdb::properties::STATS)?
                .unwrap_or_else(|| "No stats available".to_string());

            Ok(stats)
        })
        .await
        .map_err(|e| anyhow::anyhow!("Task join error: {}", e))?
    }

    /// Force database compaction
    pub async fn compact(&self) -> Result<()> {
        let db = self.db.clone();

        tokio::task::spawn_blocking(move || -> Result<()> {
            db.compact_range(None::<&[u8]>, None::<&[u8]>);
            info!("RealRocksDBStorage: Database compacted");
            Ok(())
        })
        .await
        .map_err(|e| anyhow::anyhow!("Task join error: {}", e))?
    }

    /// Get estimated number of keys in database (O(1) using RocksDB property)
    pub async fn get_key_count(&self) -> Result<usize> {
        let db = self.db.clone();

        tokio::task::spawn_blocking(move || -> Result<usize> {
            // Use RocksDB's estimate-num-keys property for O(1) performance
            // This is an approximation but avoids full table scan
            if let Some(count_str) = db.property_value(rocksdb::properties::ESTIMATE_NUM_KEYS)? {
                if let Ok(count) = count_str.parse::<usize>() {
                    return Ok(count);
                }
            }

            // Fallback to counting if property not available (shouldn't happen)
            let mut count = 0;
            let iter = db.iterator(rocksdb::IteratorMode::Start);
            for item in iter {
                let _ = item?;
                count += 1;
            }

            Ok(count)
        })
        .await
        .map_err(|e| anyhow::anyhow!("Task join error: {}", e))?
    }
}
