// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Where memory keeps the images documents name (`[blobs]`, W24): an S3
//! endpoint, its bucket and region. The keys are never in `daemon.toml`:
//! they come from `PCX_BLOBS_ACCESS_KEY` and `PCX_BLOBS_SECRET_KEY`.

use post_cortex_blobs::S3Config;
use serde::{Deserialize, Serialize};

use crate::daemon::config::ConfigError;

/// The variable of the access key's id.
const ACCESS_KEY: &str = "PCX_BLOBS_ACCESS_KEY";
/// The variable of the secret key.
const SECRET_KEY: &str = "PCX_BLOBS_SECRET_KEY";

/// The `[blobs]` table: an S3 endpoint and bucket.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BlobsConfig {
    /// The endpoint's URL, e.g. `https://s3.example.net:9443`.
    pub endpoint: String,
    /// The bucket.
    pub bucket: String,
    /// The region (`us-east-1` for most S3-compatible servers).
    pub region: String,
    /// The access key's id and the secret key, from the environment.
    #[serde(skip)]
    keys: Option<(String, String)>,
}

impl std::fmt::Debug for BlobsConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BlobsConfig")
            .field("endpoint", &self.endpoint)
            .field("bucket", &self.bucket)
            .field("region", &self.region)
            .finish_non_exhaustive()
    }
}

impl BlobsConfig {
    /// The keys from the environment, read by `var`.
    ///
    /// # Errors
    /// A key is not set.
    pub fn read_keys(
        &mut self,
        var: impl Fn(&str) -> Result<Option<String>, ConfigError>,
    ) -> Result<(), ConfigError> {
        let key = |name: &str| {
            var(name)?
                .filter(|k| !k.is_empty())
                .ok_or_else(|| ConfigError::Env(format!("[blobs] needs {name} in the environment")))
        };
        self.keys = Some((key(ACCESS_KEY)?, key(SECRET_KEY)?));
        Ok(())
    }

    /// The S3 client's configuration.
    ///
    /// # Errors
    /// The keys were not read.
    pub fn s3(&self) -> Result<S3Config, String> {
        let (access_key, secret_key) = self
            .keys
            .clone()
            .ok_or_else(|| format!("[blobs] needs {ACCESS_KEY} and {SECRET_KEY}"))?;
        Ok(S3Config {
            endpoint: self.endpoint.clone(),
            bucket: self.bucket.clone(),
            region: self.region.clone(),
            access_key,
            secret_key,
            // One daemon, one store: its keys are the bucket's own.
            prefix: String::new(),
        })
    }
}
