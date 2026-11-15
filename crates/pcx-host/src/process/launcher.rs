// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The port a module's process is started through (docs/host.md, isolation
//! tiers): the composition chooses the adapter ([`crate::Direct`] for
//! first-party modules; under the OS sandbox or by the launcher of the pot
//! `pcx-modules` for a dynamic one, `pcx-module-launcher`).

use std::path::PathBuf;

use async_trait::async_trait;
use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};
use tokio::sync::mpsc;

/// Lines of a process's output buffered before its reader waits.
const LINES: usize = 256;

/// What to start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Launch {
    /// The module's id.
    pub id: String,
    /// The executable.
    pub command: PathBuf,
    /// Its arguments.
    pub args: Vec<String>,
    /// The socket it serves; its directory is this launch's own, and the
    /// callback socket lies there too.
    pub socket: PathBuf,
}

/// How a process ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ended {
    /// How, for the log (`exited (status)`).
    pub how: String,
    /// Whether it succeeded.
    pub success: bool,
}

/// Starts modules' processes.
#[async_trait]
pub trait ModuleLauncher: Send + Sync {
    /// Whether the host checks the executable's hash itself; a launcher in
    /// another pot runs what its image holds, which this host cannot read.
    fn pins(&self) -> bool {
        true
    }

    /// Start `launch`'s process.
    ///
    /// # Errors
    /// What went wrong, for the launch error.
    async fn launch(&self, launch: &Launch) -> Result<Box<dyn ModuleProcess>, String>;
}

/// A started module process; dropped, it is killed.
#[async_trait]
pub trait ModuleProcess: Send {
    /// The lines it writes on stderr, once.
    fn lines(&mut self) -> Option<mpsc::Receiver<String>>;

    /// How it ended, when it has.
    ///
    /// # Errors
    /// Its state cannot be read.
    fn ended(&mut self) -> Result<Option<Ended>, String>;

    /// Wait until it ends; how it ended. Dropping the wait leaves the
    /// process as it was.
    async fn wait(&mut self) -> Ended;

    /// Kill it.
    ///
    /// # Errors
    /// It cannot be signalled.
    async fn kill(&mut self) -> Result<(), String>;
}

/// The lines of `stream` as they come, and one saying so when it cannot be
/// read.
pub fn output_lines(stream: impl AsyncRead + Send + Unpin + 'static) -> mpsc::Receiver<String> {
    let (tx, rx) = mpsc::channel(LINES);
    tokio::spawn(async move {
        let mut lines = BufReader::new(stream).lines();
        loop {
            let line = match lines.next_line().await {
                Ok(Some(line)) => line,
                Ok(None) => return,
                Err(e) => format!("its stderr could not be read: {e}"),
            };
            if tx.send(line).await.is_err() {
                return;
            }
        }
    });
    rx
}
