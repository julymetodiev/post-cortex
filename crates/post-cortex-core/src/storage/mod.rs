// Copyright (c) 2025 Julius ML
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
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
// SOFTWARE.
pub mod export_import;
pub mod rocksdb_storage;
pub mod traits;

// SurrealDB storage backend (optional)
#[cfg(feature = "surrealdb-storage")]
pub mod surrealdb_storage;

// Re-export RealRocksDBStorage as the main storage type
pub use rocksdb_storage::RealRocksDBStorage;
pub use rocksdb_storage::SessionCheckpoint;

// Re-export storage traits
pub use traits::{GraphStorage, Storage, StorageBackend, StorageBackendType, StorageConfig, VectorStorage};

// Re-export SurrealDB storage when enabled
#[cfg(feature = "surrealdb-storage")]
pub use surrealdb_storage::SurrealDBStorage;
#[cfg(feature = "surrealdb-storage")]
pub use traits::SurrealDBConfig;

// Re-export export/import types
pub use export_import::{
    CompressionType, ExportData, ExportMetadata, ExportOptions, ExportStats, ExportType,
    ExportedSession, ExportedWorkspace, ImportOptions, ImportResult,
    read_export_file, write_export_file, list_export_sessions, preview_export_file,
};
