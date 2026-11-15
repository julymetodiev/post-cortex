// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The running daemon's process id, in a file of its data directory, so
//! `pcx stop` knows which process to end.

use std::io;
use std::path::{Path, PathBuf};

use tracing::warn;

/// The file's name below the data directory.
const NAME: &str = "daemon.pid";

/// The file of a daemon that runs; removed when it is dropped.
pub struct PidFile {
    path: PathBuf,
}

impl PidFile {
    /// Writes this process's id below `data_directory`.
    ///
    /// # Errors
    /// The directory or the file could not be written.
    pub fn write(data_directory: &Path) -> io::Result<Self> {
        std::fs::create_dir_all(data_directory)?;
        let path = data_directory.join(NAME);
        std::fs::write(&path, std::process::id().to_string())?;
        Ok(Self { path })
    }
}

impl Drop for PidFile {
    fn drop(&mut self) {
        if let Err(e) = std::fs::remove_file(&self.path) {
            warn!("Cannot remove {}: {e}", self.path.display());
        }
    }
}

/// The process id a daemon wrote below `data_directory`; none without a
/// file.
///
/// # Errors
/// The file could not be read, or holds no process id.
pub fn recorded(data_directory: &Path) -> io::Result<Option<u32>> {
    let path = data_directory.join(NAME);
    match std::fs::read_to_string(&path) {
        Ok(text) => text.trim().parse().map(Some).map_err(|e| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("{} holds no process id: {e}", path.display()),
            )
        }),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_running_daemon_is_recorded_and_a_stopped_one_is_not() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(recorded(dir.path()).unwrap(), None);
        let file = PidFile::write(dir.path()).unwrap();
        assert_eq!(recorded(dir.path()).unwrap(), Some(std::process::id()));
        drop(file);
        assert_eq!(recorded(dir.path()).unwrap(), None);
    }

    #[test]
    fn a_file_that_holds_no_process_id_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(NAME), "soon").unwrap();
        let refused = recorded(dir.path()).unwrap_err();
        assert_eq!(refused.kind(), io::ErrorKind::InvalidData);
    }
}
