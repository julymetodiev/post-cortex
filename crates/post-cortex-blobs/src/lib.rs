// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The blob adapter (ARCHITECTURE section 5, `BlobStore`; W24): a blob is
//! the object `[<prefix>/]<workspace>/<sha256>` of an object store (any S3
//! endpoint, under a prefix when several stores share a bucket: a cloud
//! tenant's; in memory for tests), its media type its content type, and its width,
//! height and the time it was put user metadata. The daemon's composition
//! opens it with the endpoint, bucket and region of its configuration and
//! the keys of its environment.

#![forbid(unsafe_code)]

use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use futures::TryStreamExt;
use object_store::aws::AmazonS3Builder;
use object_store::memory::InMemory;
use object_store::path::Path;
use object_store::prefix::PrefixStore;
use object_store::{
    Attribute, AttributeValue, Attributes, GetOptions, ObjectStore, ObjectStoreExt, PutOptions,
    PutPayload,
};
use post_cortex_core::core::blob::{BlobId, BlobInfo, Image, MediaType};
use post_cortex_ports::{Blob, BlobStore, PortError};
use uuid::Uuid;

/// The metadata key of a blob's width.
const WIDTH: &str = "width";
/// The metadata key of a blob's height.
const HEIGHT: &str = "height";
/// The metadata key of when a blob was put (RFC 3339).
const PUT_AT: &str = "put-at";

/// Where the blobs are: an S3 endpoint, its bucket and region, and the
/// keys (the daemon reads them from its environment).
#[derive(Clone, PartialEq, Eq)]
pub struct S3Config {
    /// The endpoint's URL, e.g. `https://s3.example:9443`.
    pub endpoint: String,
    /// The bucket.
    pub bucket: String,
    /// The region (`us-east-1` for most S3-compatible servers).
    pub region: String,
    /// The access key's id.
    pub access_key: String,
    /// The secret key.
    pub secret_key: String,
    /// The prefix of every key (a cloud tenant's); empty: none.
    pub prefix: String,
}

impl std::fmt::Debug for S3Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("S3Config")
            .field("endpoint", &self.endpoint)
            .field("bucket", &self.bucket)
            .field("region", &self.region)
            .field("prefix", &self.prefix)
            .finish_non_exhaustive()
    }
}

/// Why the blob store did not open.
#[derive(Debug, thiserror::Error)]
pub enum OpenError {
    /// The configuration does not make an S3 client.
    #[error("the blob store's S3 configuration: {0}")]
    Config(String),
}

/// Blobs as the objects of an object store.
pub struct ObjectBlobs {
    store: Arc<dyn ObjectStore>,
}

impl ObjectBlobs {
    /// The blobs of the S3 bucket `config` names, under its prefix
    /// (path-style requests, as S3-compatible servers take them).
    ///
    /// # Errors
    /// The configuration does not make a client.
    pub fn s3(config: &S3Config) -> Result<Self, OpenError> {
        let store = AmazonS3Builder::new()
            .with_endpoint(&config.endpoint)
            .with_bucket_name(&config.bucket)
            .with_region(&config.region)
            .with_access_key_id(&config.access_key)
            .with_secret_access_key(&config.secret_key)
            .with_virtual_hosted_style_request(false)
            .with_allow_http(config.endpoint.starts_with("http://"))
            .build()
            .map_err(|e| OpenError::Config(e.to_string()))?;
        let prefix = config.prefix.trim_matches('/');
        let store: Arc<dyn ObjectStore> = if prefix.is_empty() {
            Arc::new(store)
        } else {
            Arc::new(PrefixStore::new(store, prefix))
        };
        Ok(Self { store })
    }

    /// Blobs kept in memory, for tests.
    #[must_use]
    pub fn in_memory() -> Self {
        Self {
            store: Arc::new(InMemory::new()),
        }
    }
}

fn path(workspace_id: Uuid, id: &BlobId) -> Path {
    Path::from(format!("{workspace_id}/{id}"))
}

fn unavailable(e: &object_store::Error) -> PortError {
    PortError::unavailable(format!("blob store: {e}"))
}

/// `info`'s attributes.
fn attributes(info: &BlobInfo) -> Attributes {
    let mut attributes = Attributes::new();
    attributes.insert(
        Attribute::ContentType,
        AttributeValue::from(info.image.media_type.as_str()),
    );
    let mut meta = |key: &'static str, value: String| {
        attributes.insert(Attribute::Metadata(key.into()), AttributeValue::from(value));
    };
    meta(WIDTH, info.image.width.to_string());
    meta(HEIGHT, info.image.height.to_string());
    meta(PUT_AT, info.put_at.to_rfc3339());
    attributes
}

/// What blob `id` of `size` bytes is, from its attributes.
fn info(id: &BlobId, size: u64, attributes: &Attributes) -> Result<BlobInfo, PortError> {
    let corrupt = |reason: String| PortError::Corrupt {
        kind: "blob",
        id: id.to_string(),
        reason,
    };
    let read = |attribute: Attribute, name: &str| {
        attributes
            .get(&attribute)
            .map(|value| value.as_ref().to_string())
            .ok_or_else(|| corrupt(format!("no {name}")))
    };
    let media_type: MediaType = read(Attribute::ContentType, "content type")?
        .parse()
        .map_err(|e| corrupt(format!("{e}")))?;
    let meta = |key: &'static str| read(Attribute::Metadata(key.into()), key);
    let pixels = |key: &'static str| {
        meta(key)?
            .parse::<u32>()
            .map_err(|e| corrupt(format!("{key}: {e}")))
    };
    let put_at = DateTime::parse_from_rfc3339(&meta(PUT_AT)?)
        .map_err(|e| corrupt(format!("{PUT_AT}: {e}")))?
        .with_timezone(&Utc);
    Ok(BlobInfo {
        id: id.clone(),
        size,
        image: Image {
            media_type,
            width: pixels(WIDTH)?,
            height: pixels(HEIGHT)?,
        },
        put_at,
    })
}

#[async_trait]
impl BlobStore for ObjectBlobs {
    async fn put_blob(&self, workspace_id: Uuid, blob: Blob) -> Result<BlobInfo, PortError> {
        // Its bytes are its id: one kept already is the same blob.
        if let Some(kept) = self.stat_blob(workspace_id, &blob.info.id).await? {
            return Ok(kept);
        }
        let options = PutOptions {
            attributes: attributes(&blob.info),
            ..PutOptions::default()
        };
        self.store
            .put_opts(
                &path(workspace_id, &blob.info.id),
                PutPayload::from(blob.bytes),
                options,
            )
            .await
            .map_err(|e| unavailable(&e))?;
        Ok(blob.info)
    }

    async fn get_blob(&self, workspace_id: Uuid, id: &BlobId) -> Result<Option<Blob>, PortError> {
        let found = match self.store.get(&path(workspace_id, id)).await {
            Ok(found) => found,
            Err(object_store::Error::NotFound { .. }) => return Ok(None),
            Err(e) => return Err(unavailable(&e)),
        };
        let info = info(id, found.meta.size, &found.attributes)?;
        let bytes = found.bytes().await.map_err(|e| unavailable(&e))?;
        Ok(Some(Blob {
            info,
            bytes: bytes.to_vec(),
        }))
    }

    async fn stat_blob(
        &self,
        workspace_id: Uuid,
        id: &BlobId,
    ) -> Result<Option<BlobInfo>, PortError> {
        let head = GetOptions {
            head: true,
            ..GetOptions::default()
        };
        match self.store.get_opts(&path(workspace_id, id), head).await {
            Ok(found) => info(id, found.meta.size, &found.attributes).map(Some),
            Err(object_store::Error::NotFound { .. }) => Ok(None),
            Err(e) => Err(unavailable(&e)),
        }
    }

    async fn list_blobs(&self, workspace_id: Uuid) -> Result<Vec<BlobInfo>, PortError> {
        let prefix = Path::from(workspace_id.to_string());
        let objects: Vec<_> = self
            .store
            .list(Some(&prefix))
            .try_collect()
            .await
            .map_err(|e| unavailable(&e))?;
        let mut listed = Vec::with_capacity(objects.len());
        for object in objects {
            let corrupt = |reason: String| PortError::Corrupt {
                kind: "blob",
                id: object.location.to_string(),
                reason,
            };
            let id: BlobId = object
                .location
                .filename()
                .ok_or_else(|| corrupt("no file name".to_string()))?
                .parse()
                .map_err(|e| corrupt(format!("{e}")))?;
            if let Some(found) = self.stat_blob(workspace_id, &id).await? {
                listed.push(found);
            }
        }
        Ok(listed)
    }

    async fn delete_blob(&self, workspace_id: Uuid, id: &BlobId) -> Result<bool, PortError> {
        if self.stat_blob(workspace_id, id).await?.is_none() {
            return Ok(false);
        }
        self.store
            .delete(&path(workspace_id, id))
            .await
            .map_err(|e| unavailable(&e))?;
        Ok(true)
    }
}
