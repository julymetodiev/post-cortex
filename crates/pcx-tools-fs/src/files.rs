// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The file service of `tools-fs`: typed requests over the module's
//! sandbox, the application layer between the tool handlers (JSON) and the
//! `Sandbox` port.

use pcx_module_api::ModuleError;
use pcx_module_api::sandbox::{DirEntry, EntryKind, Sandbox, SandboxPath};

mod search;
pub mod text;

pub use search::{GlobRequest, GrepRequest, Start};

/// The longest file read or edited.
pub const MAX_FILE_BYTES: usize = 8 * 1024 * 1024;

/// Lines a read answers at most, unless it asks for fewer.
pub const MAX_LINES: usize = 2000;

/// Bytes of lines a read answers at most (at least one line).
pub const MAX_BYTES: usize = 50 * 1024;

/// Files a glob answers when it asks for no limit.
pub const GLOB_LIMIT: usize = 1000;

/// Matches a grep answers when it asks for no limit.
pub const GREP_LIMIT: usize = 100;

/// Lines of a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lines {
    /// The number of the first (1-based).
    pub first: usize,
    /// The lines.
    pub lines: Vec<String>,
    /// Lines of the whole file.
    pub total: usize,
}

/// A file written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Written {
    /// Its bytes.
    pub bytes: usize,
    /// Its lines.
    pub lines: usize,
}

/// The files of one module, through its sandbox.
pub struct Files<'a> {
    sandbox: &'a dyn Sandbox,
}

impl<'a> Files<'a> {
    /// The files `sandbox` lets the module reach.
    pub fn new(sandbox: &'a dyn Sandbox) -> Self {
        Self { sandbox }
    }

    /// The lines of `file` from line `offset` (1-based), at most `limit`
    /// (none: [`MAX_LINES`]) and [`MAX_BYTES`] of them.
    ///
    /// # Errors
    /// As the sandbox's read; `InvalidArgument` for a file that is not
    /// UTF-8.
    pub async fn read(
        &self,
        file: &SandboxPath,
        offset: Option<usize>,
        limit: Option<usize>,
    ) -> Result<Lines, ModuleError> {
        let content = self.text(file).await?;
        let (first, lines, total) = text::slice(&content, offset, Some(limit.unwrap_or(MAX_LINES)));
        let mut bytes = 0;
        let kept = lines
            .into_iter()
            .enumerate()
            .take_while(|(i, line)| {
                bytes += line.len() + 1;
                *i == 0 || bytes <= MAX_BYTES
            })
            .map(|(_, line)| line.to_string())
            .collect();
        Ok(Lines {
            first,
            lines: kept,
            total,
        })
    }

    /// Replace `file` with `content`.
    ///
    /// # Errors
    /// As the sandbox's write.
    pub async fn write(&self, file: &SandboxPath, content: String) -> Result<Written, ModuleError> {
        let written = Written {
            bytes: content.len(),
            lines: content.lines().count(),
        };
        self.sandbox.write(file, content.into_bytes()).await?;
        Ok(written)
    }

    /// Replace the one `old` in `file` by `new`; the line it starts on.
    ///
    /// # Errors
    /// As [`Files::read`] and the sandbox's write; `InvalidArgument` when
    /// `old` is not there once.
    pub async fn edit(
        &self,
        file: &SandboxPath,
        old: &str,
        new: &str,
    ) -> Result<usize, ModuleError> {
        let content = self.text(file).await?;
        let (changed, line) = text::replace_once(&content, old, new)?;
        self.sandbox.write(file, changed.into_bytes()).await?;
        Ok(line)
    }

    /// The entries of `dir`, directories first, each group by name.
    ///
    /// # Errors
    /// As the sandbox's list.
    pub async fn list(&self, dir: &SandboxPath) -> Result<Vec<DirEntry>, ModuleError> {
        let mut entries = self.sandbox.list(dir).await?;
        entries.sort_by_key(|e| (e.metadata.kind != EntryKind::Dir, e.name.clone()));
        Ok(entries)
    }

    /// The UTF-8 text of `file`.
    async fn text(&self, file: &SandboxPath) -> Result<String, ModuleError> {
        let bytes = self.sandbox.read(file, MAX_FILE_BYTES).await?;
        text::utf8(&file.path.display().to_string(), bytes)
    }
}
