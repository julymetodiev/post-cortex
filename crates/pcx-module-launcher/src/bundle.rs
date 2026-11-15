// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! A bundle's tar unpacked by its rules (docs/modules/dynamic.md, what a
//! bundle holds).

use std::path::{Component, Path};

use crate::prepare::{PrepareError, step};

/// A bundle's limits, unpacked.
const MAX_FILES: usize = 1_000;
const MAX_UNPACKED: u64 = 64 * 1024 * 1024;

/// `tar` unpacked into the new directory `into`: regular files and
/// directories only, beneath it, within the bundle's limits.
///
/// # Errors
/// [`PrepareError::Bundle`] naming the file at fault.
pub fn unpack(tar_bytes: &[u8], into: &Path) -> Result<(), PrepareError> {
    let refused = |m: String| PrepareError::Bundle(m);
    let mut archive = tar::Archive::new(tar_bytes);
    let entries = archive
        .entries()
        .map_err(|e| refused(format!("not a tar: {e}")))?;
    let (mut files, mut size) = (0usize, 0u64);
    for entry in entries {
        let mut entry = entry.map_err(|e| refused(format!("not a tar: {e}")))?;
        let path = entry
            .path()
            .map_err(|e| refused(format!("a name: {e}")))?
            .into_owned();
        let named = path.display().to_string();
        if !path
            .components()
            .all(|c| matches!(c, Component::Normal(_) | Component::CurDir))
        {
            return Err(refused(format!("{named}: a path stays beneath the bundle")));
        }
        let kind = entry.header().entry_type();
        if !(kind.is_file() || kind.is_dir()) {
            return Err(refused(format!(
                "{named}: a bundle holds files and directories only"
            )));
        }
        files += 1;
        size += entry.size();
        if files > MAX_FILES {
            return Err(refused(format!("more than {MAX_FILES} files")));
        }
        if size > MAX_UNPACKED {
            return Err(refused(format!("more than {MAX_UNPACKED} bytes unpacked")));
        }
        let target = into.join(&path);
        if kind.is_dir() {
            std::fs::create_dir_all(&target).map_err(|e| step("make", &target, &e))?;
        } else {
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent).map_err(|e| step("make", parent, &e))?;
            }
            let mut file = std::fs::File::create(&target).map_err(|e| step("make", &target, &e))?;
            std::io::copy(&mut entry, &mut file).map_err(|e| step("write", &target, &e))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tar_of(entries: &[(&str, tar::EntryType)]) -> Vec<u8> {
        let mut builder = tar::Builder::new(Vec::new());
        for (name, kind) in entries {
            let mut header = tar::Header::new_gnu();
            header.set_entry_type(*kind);
            header.set_size(0);
            header.set_mode(0o644);
            // Written by hand: the crate's own checks refuse what a bad
            // archive would hold.
            let bytes = header.as_old_mut();
            bytes.name[..name.len()].copy_from_slice(name.as_bytes());
            header.set_cksum();
            builder.append(&header, std::io::empty()).unwrap();
        }
        builder.into_inner().unwrap()
    }

    #[test]
    fn a_bundle_stays_beneath_its_directory_and_holds_only_files() {
        let dir = tempfile::tempdir().unwrap();
        let good = tar_of(&[("module.py", tar::EntryType::Regular)]);
        unpack(&good, dir.path()).unwrap();
        assert!(dir.path().join("module.py").exists());
        for (name, kind, why) in [
            ("../out", tar::EntryType::Regular, "beneath"),
            ("/etc/out", tar::EntryType::Regular, "beneath"),
            ("link", tar::EntryType::Symlink, "files and directories"),
            ("hard", tar::EntryType::Link, "files and directories"),
            ("dev", tar::EntryType::Char, "files and directories"),
        ] {
            let bad = tar_of(&[(name, kind)]);
            let refused = unpack(&bad, dir.path()).unwrap_err().to_string();
            assert!(refused.contains(why) && refused.contains(name), "{refused}");
        }
    }
}
