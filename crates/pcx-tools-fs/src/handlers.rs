// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The `pcx.tools.fs.v1.Files` service of `tools-fs` (W23): each RPC's
//! message in, the typed call of the file service, its message out. No
//! logic of their own.

use std::path::PathBuf;

use pcx_module_api::sandbox::{self, SandboxPath};
use post_cortex_proto::tools_fs::files_server::Files as FilesService;
use post_cortex_proto::tools_fs::{
    EditRequest, EditResponse, Entry, EntryKind, GlobRequest, GlobResponse, GrepLine, GrepRequest,
    GrepResponse, ListDirRequest, ListDirResponse, ReadRequest, ReadResponse, WriteRequest,
    WriteResponse,
};
use tonic::{Request, Response, Status};

use crate::files::{self, Files, Start};
use crate::module::{self, Shared};

/// The module's RPCs over its sandbox.
pub struct Handlers {
    /// The running module.
    pub shared: Shared,
}

/// A number on the wire: 0 is not given.
fn given(n: u64) -> Option<usize> {
    (n != 0).then_some(n as usize)
}

/// A count on the wire.
fn n(count: usize) -> u64 {
    count as u64
}

/// Some text, or none when empty.
fn named(text: String) -> Option<String> {
    Some(text).filter(|t| !t.is_empty())
}

fn start(path: String, root: String) -> Start {
    Start {
        root: named(root).map(PathBuf::from),
        dir: PathBuf::from(path),
    }
}

fn kind(kind: sandbox::EntryKind) -> EntryKind {
    match kind {
        sandbox::EntryKind::File => EntryKind::File,
        sandbox::EntryKind::Dir => EntryKind::Dir,
        sandbox::EntryKind::Symlink => EntryKind::Symlink,
        sandbox::EntryKind::Other => EntryKind::Other,
    }
}

#[tonic::async_trait]
impl FilesService for Handlers {
    async fn read(&self, request: Request<ReadRequest>) -> Result<Response<ReadResponse>, Status> {
        let request = request.into_inner();
        let sandbox = module::sandbox(&self.shared).await?;
        let lines = Files::new(sandbox.as_ref())
            .read(
                &SandboxPath::under(&request.path, named(request.root).as_deref()),
                given(request.offset),
                given(request.limit),
            )
            .await?;
        Ok(Response::new(ReadResponse {
            path: request.path,
            first_line: n(lines.first),
            lines: lines.lines,
            total_lines: n(lines.total),
        }))
    }

    async fn write(
        &self,
        request: Request<WriteRequest>,
    ) -> Result<Response<WriteResponse>, Status> {
        let request = request.into_inner();
        let sandbox = module::sandbox(&self.shared).await?;
        let written = Files::new(sandbox.as_ref())
            .write(
                &SandboxPath::under(&request.path, named(request.root).as_deref()),
                request.content,
            )
            .await?;
        Ok(Response::new(WriteResponse {
            path: request.path,
            bytes: n(written.bytes),
            lines: n(written.lines),
        }))
    }

    async fn edit(&self, request: Request<EditRequest>) -> Result<Response<EditResponse>, Status> {
        let request = request.into_inner();
        let sandbox = module::sandbox(&self.shared).await?;
        let line = Files::new(sandbox.as_ref())
            .edit(
                &SandboxPath::under(&request.path, named(request.root).as_deref()),
                &request.old_text,
                &request.new_text,
            )
            .await?;
        Ok(Response::new(EditResponse {
            path: request.path,
            line: n(line),
        }))
    }

    async fn list_dir(
        &self,
        request: Request<ListDirRequest>,
    ) -> Result<Response<ListDirResponse>, Status> {
        let request = request.into_inner();
        let sandbox = module::sandbox(&self.shared).await?;
        let entries = Files::new(sandbox.as_ref())
            .list(&SandboxPath::under(
                &request.path,
                named(request.root).as_deref(),
            ))
            .await?;
        Ok(Response::new(ListDirResponse {
            path: request.path,
            entries: entries
                .into_iter()
                .map(|e| Entry {
                    name: e.name,
                    kind: kind(e.metadata.kind).into(),
                    size: e.metadata.size,
                })
                .collect(),
        }))
    }

    async fn glob(&self, request: Request<GlobRequest>) -> Result<Response<GlobResponse>, Status> {
        let request = request.into_inner();
        let sandbox = module::sandbox(&self.shared).await?;
        let found = Files::new(sandbox.as_ref())
            .glob(&files::GlobRequest {
                start: start(request.path, request.root),
                pattern: request.pattern,
                limit: given(request.limit).unwrap_or(files::GLOB_LIMIT),
            })
            .await?;
        Ok(Response::new(GlobResponse {
            files: found.files,
            total: n(found.total),
            cut: found.cut,
        }))
    }

    async fn grep(&self, request: Request<GrepRequest>) -> Result<Response<GrepResponse>, Status> {
        let request = request.into_inner();
        let sandbox = module::sandbox(&self.shared).await?;
        let found = Files::new(sandbox.as_ref())
            .grep(&files::GrepRequest {
                start: start(request.path, request.root),
                pattern: request.pattern,
                literal: request.literal,
                ignore_case: request.ignore_case,
                glob: named(request.glob),
                context: request.context as usize,
                limit: given(request.limit).unwrap_or(files::GREP_LIMIT),
            })
            .await?;
        Ok(Response::new(GrepResponse {
            lines: found
                .hits
                .into_iter()
                .map(|h| GrepLine {
                    file: h.file,
                    number: n(h.line.number),
                    text: h.line.text,
                    matched: h.line.matched,
                })
                .collect(),
            matches: n(found.matches),
            more: found.more,
            cut: found.cut,
        }))
    }
}
