// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Why the daemon's configuration could not be loaded.

use std::path::PathBuf;

/// Why the daemon's configuration could not be loaded.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// `daemon.toml` exists but cannot be read.
    #[error("cannot read {}: {source}", path.display())]
    Read {
        /// The file.
        path: PathBuf,
        /// The read error.
        source: std::io::Error,
    },
    /// `daemon.toml` is not a valid configuration.
    #[error("{} is not valid: {source}", path.display())]
    Parse {
        /// The file.
        path: PathBuf,
        /// What is wrong with it.
        source: toml::de::Error,
    },
    /// An environment override is not valid.
    #[error("{0}")]
    Env(String),
    /// The resulting configuration does not validate.
    #[error("invalid configuration: {0}")]
    Invalid(String),
}
