// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Transfer's export and import over the storage adapters and the search
//! index: archives round-trip between backends, merges, the archive
//! format and the golden 1.4.1 bytes.

use std::io::Write;
use std::ops::ControlFlow;
use std::sync::{Arc, Mutex};

use post_cortex_core::archive::{
    CompressionType, ExportData, ExportOptions, ExportScope, ImportOptions, ImportResult,
};
use post_cortex_embeddings::MemoryIndex;
use post_cortex_ports::conformance::Adapter;
use post_cortex_ports::storage::{
    CardStore, CheckpointStore, DismissalStore, DocumentStore, EventCursors, EventLog,
    FreshnessStore, MessageStore, SessionStore, Stores, VectorStore, WorkspaceStore,
};
use post_cortex_ports::{ArchiveCodec, PortError};

use crate::capabilities::ArchivePorts;
use crate::capabilities::transfer::model::mismatch;
use crate::capabilities::transfer::parts::PartReader;
use crate::capabilities::transfer::{
    ArchiveFile, ArchiveSpool, ImportPreview, TransferError, preview as previewing,
};

mod archive;
mod conformance;
mod documents;
mod format;
mod golden;
mod merge;
mod merge_lifecycle;
mod model;
mod scopes;
mod stream;

/// Every port, as one backend struct implements them.
pub trait Backend:
    SessionStore
    + CheckpointStore
    + WorkspaceStore
    + VectorStore
    + MessageStore
    + CardStore
    + DocumentStore
    + DismissalStore
    + FreshnessStore
    + EventLog
    + EventCursors
    + Clone
    + 'static
{
}

impl<T> Backend for T where
    T: SessionStore
        + CheckpointStore
        + WorkspaceStore
        + VectorStore
        + MessageStore
        + CardStore
        + DocumentStore
        + DismissalStore
        + FreshnessStore
        + EventLog
        + EventCursors
        + Clone
        + 'static
{
}

/// The archive ports over `stores` and a new search index of `dimension`
/// over `vectors`, with the JSON codec.
pub fn ports_over(stores: Stores, vectors: Arc<dyn VectorStore>, dimension: usize) -> ArchivePorts {
    let index = MemoryIndex::new(dimension, Some(vectors));
    ArchivePorts {
        stores,
        index: Some(Arc::new(index)),
        codec: Arc::new(post_cortex_storage::JsonArchive),
    }
}

/// The archive ports of `backend` (a clone shares its database), for
/// vectors of 256 dimensions.
pub fn of<B: Backend>(backend: &B) -> ArchivePorts {
    of_dim(backend, 256)
}

/// [`of`] for vectors of `dimension`.
pub fn of_dim<B: Backend>(backend: &B, dimension: usize) -> ArchivePorts {
    let backend = Arc::new(backend.clone());
    ports_over(Stores::of(Arc::clone(&backend)), backend, dimension)
}

/// The archive ports of a conformance adapter, for vectors of `dimension`.
pub fn of_adapter(adapter: &Adapter, dimension: usize) -> ArchivePorts {
    ports_over(
        adapter.stores.clone(),
        Arc::clone(&adapter.vectors),
        dimension,
    )
}

/// RocksDB in a temporary directory, as a conformance adapter.
pub async fn rocksdb() -> (Adapter, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let storage = post_cortex_storage::RealRocksDBStorage::new(dir.path())
        .await
        .unwrap();
    (Adapter::of(Arc::new(storage)), dir)
}

/// SurrealDB `mem://`, as a conformance adapter.
pub async fn surrealdb() -> (Adapter, ()) {
    let storage = post_cortex_storage::SurrealDBStorage::new("mem://", None, None, None, None)
        .await
        .unwrap();
    (Adapter::of(Arc::new(storage)), ())
}

#[tokio::test]
async fn rocksdb_archives() {
    let dir = tempfile::tempdir().unwrap();
    let open = |name: &str| post_cortex_storage::RealRocksDBStorage::new(dir.path().join(name));
    archive::archives_round_trip(&open("src").await.unwrap(), &open("dst").await.unwrap()).await;
    merge_lifecycle::diverged_cards_keep_the_target_unless_asked(&open("dv").await.unwrap()).await;
    archive::cards_and_messages_round_trip(
        &open("cm-src").await.unwrap(),
        &open("cm-fresh").await.unwrap(),
        &open("cm-existing").await.unwrap(),
    )
    .await;
    merge_lifecycle::chunked_message_vectors_round_trip(
        &open("ch-src").await.unwrap(),
        &open("ch-fresh").await.unwrap(),
        &open("ch-existing").await.unwrap(),
    )
    .await;
}

#[tokio::test]
async fn surrealdb_archives() {
    let mem = || post_cortex_storage::SurrealDBStorage::new("mem://", None, None, None, None);
    archive::archives_round_trip(&mem().await.unwrap(), &mem().await.unwrap()).await;
    merge_lifecycle::diverged_cards_keep_the_target_unless_asked(&mem().await.unwrap()).await;
    archive::cards_and_messages_round_trip(
        &mem().await.unwrap(),
        &mem().await.unwrap(),
        &mem().await.unwrap(),
    )
    .await;
    merge_lifecycle::chunked_message_vectors_round_trip(
        &mem().await.unwrap(),
        &mem().await.unwrap(),
        &mem().await.unwrap(),
    )
    .await;
}

/// One `#[tokio::test]` per case, run against RocksDB and SurrealDB.
macro_rules! on_backends {
    ($($case:ident),+ $(,)?) => {
        mod rocksdb {
            $(
                #[tokio::test(flavor = "multi_thread")]
                async fn $case() {
                    let (adapter, _keep) = crate::capabilities::transfer::tests::rocksdb().await;
                    super::$case(&adapter).await;
                }
            )+
        }

        mod surrealdb {
            $(
                #[tokio::test(flavor = "multi_thread")]
                async fn $case() {
                    let (adapter, _keep) = crate::capabilities::transfer::tests::surrealdb().await;
                    super::$case(&adapter).await;
                }
            )+
        }
    };
}
use on_backends;

/// A buffer an export writes into while the test keeps it.
#[derive(Clone, Default)]
pub struct Shared(Arc<Mutex<Vec<u8>>>);

impl Write for Shared {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Shared {
    pub fn bytes(&self) -> Vec<u8> {
        self.0.lock().unwrap().clone()
    }
}

/// The bytes of `data`, as the codec writes them.
pub fn encode(data: &ExportData, compression: CompressionType, pretty: bool) -> Vec<u8> {
    let out = Shared::default();
    let mut writer = post_cortex_storage::JsonArchive
        .writer(Box::new(out.clone()), compression, pretty)
        .unwrap();
    for part in data.parts() {
        writer.write(&part).unwrap();
    }
    writer.finish().unwrap();
    out.bytes()
}

/// The archive in `bytes`.
pub fn decode(bytes: &[u8]) -> Result<ExportData, PortError> {
    let mut parts = Vec::new();
    post_cortex_storage::JsonArchive.read(&mut &bytes[..], &mut |part| {
        parts.push(part);
        ControlFlow::Continue(())
    })?;
    Ok(ExportData::from_parts(parts))
}

/// `data` as an archive file (spooled, deleted with it).
pub async fn archive_file(data: &ExportData) -> ArchiveFile {
    let mut spool = ArchiveSpool::new(u64::MAX).unwrap();
    spool
        .append(&encode(data, CompressionType::None, false))
        .await
        .unwrap();
    spool.finish().await.unwrap()
}

/// Export `scope` from `ports`, both passes, into memory.
pub async fn export(
    ports: &ArchivePorts,
    scope: &ExportScope,
    options: &ExportOptions,
) -> Result<ExportData, TransferError> {
    let plan = crate::capabilities::transfer::export::plan(ports, scope, options.clone()).await?;
    let out = Shared::default();
    plan.write(Box::new(out.clone())).await?;
    Ok(decode(&out.bytes())?)
}

/// What the preview pass says of importing `data` into `ports`.
pub async fn preview_of(
    ports: &ArchivePorts,
    data: &ExportData,
    options: &ImportOptions,
) -> Result<ImportPreview, TransferError> {
    let file = archive_file(data).await;
    let parts = PartReader::start(Arc::clone(&ports.codec), &file)?;
    Ok(
        previewing::scan(&ports.stores, ports.codec.as_ref(), parts, options)
            .await?
            .preview,
    )
}

/// Import `data` into `ports` as the Transfer service does, without the
/// in-memory steps: the preview pass (refusing another model's vectors),
/// then the write pass.
pub async fn import(
    ports: &ArchivePorts,
    data: ExportData,
    options: &ImportOptions,
) -> Result<ImportResult, TransferError> {
    let file = archive_file(&data).await;
    let parts = PartReader::start(Arc::clone(&ports.codec), &file)?;
    let scan = previewing::scan(&ports.stores, ports.codec.as_ref(), parts, options).await?;
    if let Some(why) = mismatch(&scan.vectors, options.embedding_model.as_ref()) {
        return Err(TransferError::ModelMismatch(why));
    }
    let parts = PartReader::start(Arc::clone(&ports.codec), &file)?;
    crate::capabilities::transfer::import::import(ports, parts, options).await
}

/// The service with whole archives in memory, for tests of a running
/// instance.
impl crate::capabilities::Transfer {
    /// Export `scope` (both passes) into memory.
    pub async fn export_data(
        &self,
        scope: ExportScope,
        options: ExportOptions,
    ) -> Result<ExportData, TransferError> {
        let plan = self.plan_export(scope, options).await?;
        let out = Shared::default();
        plan.write(Box::new(out.clone())).await?;
        Ok(decode(&out.bytes())?)
    }

    /// Preview importing `data`.
    pub async fn preview_data(
        &self,
        data: &ExportData,
        options: &ImportOptions,
    ) -> Result<ImportPreview, TransferError> {
        self.preview_import(&archive_file(data).await, options)
            .await
    }

    /// Import `data`.
    pub async fn import_data(
        &self,
        data: ExportData,
        options: ImportOptions,
    ) -> Result<ImportResult, TransferError> {
        Ok(self.import(&archive_file(&data).await, options).await?.1)
    }
}
