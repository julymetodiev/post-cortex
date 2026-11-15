// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The codec on a blocking thread, joined to the async export and import
//! by bounded channels of parts: a reader hands an archive's parts on as
//! it parses them, a writer encodes the parts an export sends. At most
//! [`BUFFERED`] parts wait in either.

use std::io::Write;
use std::ops::ControlFlow;
use std::sync::Arc;

use post_cortex_core::archive::{ArchivePart, ArchiveSize, CompressionType};
use post_cortex_ports::{ArchiveCodec, PortError};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::capabilities::transfer::{ArchiveFile, TransferError};

/// Parts waiting between the codec and the capability.
const BUFFERED: usize = 8;

/// The parts of an archive file, in archive order.
pub struct PartReader {
    parts: mpsc::Receiver<ArchivePart>,
    task: JoinHandle<Result<(), PortError>>,
}

impl PartReader {
    /// Start reading `archive` with `codec`.
    ///
    /// # Errors
    /// The file cannot be opened.
    pub fn start(
        codec: Arc<dyn ArchiveCodec>,
        archive: &ArchiveFile,
    ) -> Result<Self, TransferError> {
        let mut file = archive.open()?;
        let (tx, parts) = mpsc::channel(BUFFERED);
        let task = tokio::task::spawn_blocking(move || {
            codec.read(&mut file, &mut |part| match tx.blocking_send(part) {
                Ok(()) => ControlFlow::Continue(()),
                // The capability stopped reading.
                Err(_) => ControlFlow::Break(()),
            })
        });
        Ok(Self { parts, task })
    }

    /// The next part; `None` at the end of the archive, or when reading
    /// failed (see [`Self::finish`]).
    pub async fn next(&mut self) -> Option<ArchivePart> {
        self.parts.recv().await
    }

    /// Whether the whole archive was read.
    ///
    /// # Errors
    /// The bytes are not an archive, or reading failed.
    pub async fn finish(self) -> Result<(), TransferError> {
        drop(self.parts);
        self.task.await?.map_err(TransferError::Archive)
    }
}

/// What an export sends its writer.
enum Sent {
    Part(ArchivePart),
    /// Every part was sent: finish the archive.
    End,
}

/// An archive being written by the codec.
pub struct PartWriter {
    parts: mpsc::Sender<Sent>,
    task: JoinHandle<Result<ArchiveSize, PortError>>,
}

impl PartWriter {
    /// Start writing an archive into `out` with `codec`.
    pub fn start(
        codec: Arc<dyn ArchiveCodec>,
        out: Box<dyn Write + Send>,
        compression: CompressionType,
        pretty: bool,
    ) -> Self {
        let (parts, mut rx) = mpsc::channel(BUFFERED);
        let task = tokio::task::spawn_blocking(move || {
            let mut writer = codec.writer(out, compression, pretty)?;
            loop {
                match rx.blocking_recv() {
                    Some(Sent::Part(part)) => writer.write(&part)?,
                    Some(Sent::End) => return writer.finish(),
                    // Abandoned: the archive is left unfinished, so it
                    // never reads as a complete one.
                    None => {
                        return Err(PortError::Unavailable(
                            "the export stopped before its archive was complete".into(),
                        ));
                    }
                }
            }
        });
        Self { parts, task }
    }

    /// Send `part`. `false` when the writer has failed (see
    /// [`Self::finish`]).
    pub async fn send(&self, part: ArchivePart) -> bool {
        self.parts.send(Sent::Part(part)).await.is_ok()
    }

    /// Finish the archive.
    ///
    /// # Errors
    /// Writing failed.
    pub async fn finish(self) -> Result<ArchiveSize, TransferError> {
        if self.parts.send(Sent::End).await.is_err() {
            // The writer failed and stopped receiving; its error follows.
            tracing::debug!("the archive writer stopped before the end");
        }
        drop(self.parts);
        Ok(self.task.await??)
    }

    /// Leave the archive unfinished (the export failed).
    pub async fn abandon(self) {
        drop(self.parts);
        match self.task.await {
            Ok(Ok(_)) => {}
            Ok(Err(e)) => tracing::debug!("abandoned export archive: {e}"),
            Err(e) => tracing::warn!("the archive writer failed: {e}"),
        }
    }
}
