// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Server-sent events of a model's answer (any provider's stream): bytes
//! in, each event's `data` out, whole; OpenAI-compatible streams end with
//! `[DONE]`, others when they close. A line is decoded only once it is complete, so a
//! character split between two chunks reads whole.

use std::fmt::Display;

use tokio::sync::mpsc;
use tokio_stream::{Stream, StreamExt};

use crate::model::{ModelEvent, ModelFailure, ModelFailureKind};

/// What the stream says it ends with.
const DONE: &str = "[DONE]";

/// The data lines of a stream, as its chunks arrive.
#[derive(Debug, Default)]
pub struct Lines {
    pending: Vec<u8>,
    done: bool,
}

impl Lines {
    /// Nothing read yet.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The data of the events `chunk` completes, in order; none after the
    /// stream said `[DONE]`.
    ///
    /// # Errors
    /// `Invalid` for a line that is not UTF-8.
    pub fn feed(&mut self, chunk: &[u8]) -> Result<Vec<String>, ModelFailure> {
        if self.done {
            return Ok(Vec::new());
        }
        self.pending.extend_from_slice(chunk);
        let mut data = Vec::new();
        while let Some(end) = self.pending.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = self.pending.drain(..=end).collect();
            let line = std::str::from_utf8(&line).map_err(|e| {
                ModelFailure::new(
                    ModelFailureKind::Invalid,
                    format!("the stream sent a line that is not UTF-8: {e}"),
                )
            })?;
            // Comments (`:`), other fields and blank lines carry no data.
            let Some(payload) = line.trim().strip_prefix("data:") else {
                continue;
            };
            let payload = payload.trim();
            if payload == DONE {
                self.done = true;
                return Ok(data);
            }
            data.push(payload.to_string());
        }
        Ok(data)
    }

    /// Whether the stream said `[DONE]`.
    #[must_use]
    pub fn done(&self) -> bool {
        self.done
    }
}

/// What reads a provider's answer from its events: the deltas each
/// event's data carries, and the answer whole at the stream's end.
pub trait AnswerReader {
    /// The deltas of the event `data`.
    ///
    /// # Errors
    /// The stream's own error, or data the provider would not send.
    fn read(&mut self, data: &str) -> Result<Vec<ModelEvent>, ModelFailure>;

    /// The answer's last events (its tool calls, whole, then the answer).
    ///
    /// # Errors
    /// The answer ended unfinished, or a tool call does not read.
    fn finish(self) -> Result<Vec<ModelEvent>, ModelFailure>;
}

/// Read the `chunks` of an answer's stream through `reader` into `tx`
/// until the stream ends (or says `[DONE]`), fails, or the reader of `tx`
/// is gone; a failure is the stream's last item.
pub async fn relay<S, B, E, R>(
    chunks: S,
    mut reader: R,
    tx: &mpsc::Sender<Result<ModelEvent, ModelFailure>>,
) where
    S: Stream<Item = Result<B, E>>,
    B: AsRef<[u8]>,
    E: Display,
    R: AnswerReader,
{
    let mut chunks = std::pin::pin!(chunks);
    let mut lines = Lines::new();
    let read: Result<(), ModelFailure> = async {
        while !lines.done() {
            let Some(chunk) = chunks.next().await else {
                break;
            };
            // A transport failure: the provider stopped answering.
            let chunk = chunk
                .map_err(|e| ModelFailure::new(ModelFailureKind::Unavailable, e.to_string()))?;
            for data in lines.feed(chunk.as_ref())? {
                for event in reader.read(&data)? {
                    if tx.send(Ok(event)).await.is_err() {
                        // The reader is gone: nobody waits for the rest.
                        return Ok(());
                    }
                }
            }
        }
        Ok(())
    }
    .await;
    match read.and_then(|()| reader.finish()) {
        Ok(events) => {
            for event in events {
                if tx.send(Ok(event)).await.is_err() {
                    return;
                }
            }
        }
        Err(failure) => {
            if tx.send(Err(failure)).await.is_err() {
                tracing::debug!("a model failure after its reader left");
            }
        }
    }
}

#[cfg(test)]
mod tests;
