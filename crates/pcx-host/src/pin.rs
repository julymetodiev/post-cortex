// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Executables pinned by the SHA-256 of their file: refused at load when
//! they are not the approved one, and at each run once they changed.

use std::collections::HashMap;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use parking_lot::Mutex;
use pcx_module_api::{ModuleError, ModuleErrorKind};
use sha2::{Digest, Sha256};

use crate::HostError;

/// A file as it was when it was last hashed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Stamp {
    device: u64,
    inode: u64,
    size: u64,
    modified: (i64, i64),
}

impl Stamp {
    fn of(meta: &std::fs::Metadata) -> Self {
        Self {
            device: meta.dev(),
            inode: meta.ino(),
            size: meta.size(),
            modified: (meta.mtime(), meta.mtime_nsec()),
        }
    }
}

/// The files this process found approved: an executable (an interpreter is
/// 130 MB) is hashed once, and again only when its file changed
/// (docs/modules/dynamic.md, pinned).
static APPROVED: LazyLock<Mutex<HashMap<(PathBuf, String), Stamp>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// The SHA-256 of `bytes`, in lowercase hex.
pub fn sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// The hash of module `module`'s executable `path`, which must be
/// `approved`.
///
/// # Errors
/// The file cannot be read or is not the approved one.
pub fn at_load(module: &str, path: &Path, approved: &str) -> Result<String, HostError> {
    let unreadable = |source| HostError::ExecutableUnreadable {
        module: module.to_string(),
        path: path.to_path_buf(),
        source,
    };
    let approved = approved.to_ascii_lowercase();
    let key = (path.to_path_buf(), approved.clone());
    let stamp = Stamp::of(&std::fs::metadata(path).map_err(unreadable)?);
    if APPROVED.lock().get(&key) == Some(&stamp) {
        return Ok(approved);
    }
    let bytes = std::fs::read(path).map_err(unreadable)?;
    let found = sha256(&bytes);
    if found == approved {
        APPROVED.lock().insert(key, stamp);
        return Ok(found);
    }
    Err(HostError::ExecutableNotApproved {
        module: module.to_string(),
        path: path.to_path_buf(),
        found,
    })
}

/// Refuse an executable that changed since it was approved.
///
/// # Errors
/// The file cannot be read (`Internal`) or changed (`Denied`).
pub async fn at_run(path: &Path, approved: &str) -> Result<(), ModuleError> {
    let bytes = tokio::fs::read(path).await.map_err(|e| {
        ModuleError::new(
            ModuleErrorKind::Internal,
            format!("read {}: {e}", path.display()),
        )
    })?;
    if sha256(&bytes).eq_ignore_ascii_case(approved) {
        return Ok(());
    }
    Err(ModuleError::new(
        ModuleErrorKind::Denied,
        format!(
            "{} changed since it was approved; approve its new sha256",
            path.display()
        ),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_changed_after_its_approval_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("runtime");
        std::fs::write(&path, b"approved").unwrap();
        let approved = sha256(b"approved");
        assert_eq!(at_load("m", &path, &approved).unwrap(), approved);
        // Found again from the stamp, then changed under it.
        assert_eq!(at_load("m", &path, &approved).unwrap(), approved);
        std::fs::write(&path, b"replaced").unwrap();
        assert!(matches!(
            at_load("m", &path, &approved),
            Err(HostError::ExecutableNotApproved { .. })
        ));
    }
}
