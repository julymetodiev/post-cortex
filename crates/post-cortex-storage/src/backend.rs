// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Which storage backend a deployment runs, parsed strictly.

/// Storage backend type
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StorageBackendType {
    /// RocksDB local storage (default)
    #[default]
    RocksDB,

    /// SurrealDB storage with native graph support
    #[cfg(feature = "surrealdb-storage")]
    SurrealDB,
}

impl StorageBackendType {
    /// Every backend this build supports.
    pub const ALL: &'static [Self] = &[
        Self::RocksDB,
        #[cfg(feature = "surrealdb-storage")]
        Self::SurrealDB,
    ];

    /// Canonical name, as `daemon.toml`, `PC_STORAGE_BACKEND` and
    /// `pcx migrate` spell it; [`FromStr`](std::str::FromStr) inverts it.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::RocksDB => "rocksdb",
            #[cfg(feature = "surrealdb-storage")]
            Self::SurrealDB => "surrealdb",
        }
    }
}

impl std::fmt::Display for StorageBackendType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A storage backend name that names no backend of this build.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error(
    "unknown storage backend {name:?}; this build supports: {}",
    supported_backends()
)]
pub struct UnknownStorageBackend {
    /// The name that was given.
    pub name: String,
}

/// The names of [`StorageBackendType::ALL`], comma-separated.
fn supported_backends() -> String {
    StorageBackendType::ALL
        .iter()
        .map(|b| b.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

impl std::str::FromStr for StorageBackendType {
    type Err = UnknownStorageBackend;

    /// Case-insensitive; `rocks` and `surreal` are accepted as short forms.
    /// Anything else is an error, never a fallback to the default.
    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s.trim().to_lowercase().as_str() {
            "rocksdb" | "rocks" => Ok(Self::RocksDB),
            #[cfg(feature = "surrealdb-storage")]
            "surrealdb" | "surreal" => Ok(Self::SurrealDB),
            _ => Err(UnknownStorageBackend {
                name: s.to_string(),
            }),
        }
    }
}

#[cfg(test)]
mod tests;
