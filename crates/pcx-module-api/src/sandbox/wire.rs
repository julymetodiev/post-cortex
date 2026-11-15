// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The sandbox's types on the wire (`pcx.module.v1.Sandbox`), both ways;
//! a value the wire cannot hold, or one it holds malformed, is an error.

use std::path::PathBuf;
use std::time::{Duration, UNIX_EPOCH};

use post_cortex_proto::module as wire;

use crate::sandbox::{
    DirEntry, EntryKind, ExecEvent, ExecExit, ExecSpec, Metadata, Policy, Root, SandboxPath,
};
use crate::{ModuleError, ModuleErrorKind};

fn malformed(what: &str) -> ModuleError {
    ModuleError::new(
        ModuleErrorKind::InvalidArgument,
        format!("malformed sandbox message: {what}"),
    )
}

/// A path as text, or `InvalidArgument` when it is not UTF-8.
fn text(path: &std::path::Path) -> Result<String, ModuleError> {
    path.to_str()
        .map(str::to_string)
        .ok_or_else(|| malformed("a path is not UTF-8"))
}

impl Policy {
    /// The wire form.
    ///
    /// # Errors
    /// `InvalidArgument` for a root that is not UTF-8.
    pub fn to_wire(&self) -> Result<wire::SandboxPolicyResponse, ModuleError> {
        Ok(wire::SandboxPolicyResponse {
            roots: self
                .roots
                .iter()
                .map(|r| {
                    Ok(wire::SandboxRoot {
                        path: text(&r.path)?,
                        write: r.write,
                    })
                })
                .collect::<Result<_, ModuleError>>()?,
            network: self.network,
            spawn: self.spawn,
            deny: self.deny.clone(),
            error: None,
        })
    }

    /// The policy of a wire answer.
    ///
    /// # Errors
    /// The answer's error.
    pub fn from_wire(answer: wire::SandboxPolicyResponse) -> Result<Self, ModuleError> {
        if let Some(error) = answer.error {
            return Err(error.into());
        }
        Ok(Self {
            roots: answer
                .roots
                .into_iter()
                .map(|r| Root {
                    path: PathBuf::from(r.path),
                    write: r.write,
                })
                .collect(),
            network: answer.network,
            spawn: answer.spawn,
            deny: answer.deny,
            // Who may call the module is the host's to check, not the
            // module's: it is not on the wire.
            local_only: false,
        })
    }
}

impl SandboxPath {
    /// The wire form.
    ///
    /// # Errors
    /// `InvalidArgument` for a path that is not UTF-8.
    pub fn to_wire(&self) -> Result<wire::SandboxPath, ModuleError> {
        Ok(wire::SandboxPath {
            root: self
                .root
                .as_deref()
                .map(text)
                .transpose()?
                .unwrap_or_default(),
            path: text(&self.path)?,
        })
    }

    /// The path of a wire message (an empty root: the first).
    #[must_use]
    pub fn from_wire(path: wire::SandboxPath) -> Self {
        Self {
            root: (!path.root.is_empty()).then(|| PathBuf::from(path.root)),
            path: PathBuf::from(path.path),
        }
    }

    /// The path a request field holds.
    ///
    /// # Errors
    /// `InvalidArgument` when the field is unset.
    pub fn required(path: Option<wire::SandboxPath>, field: &str) -> Result<Self, ModuleError> {
        path.map(Self::from_wire)
            .ok_or_else(|| malformed(&format!("{field} is unset")))
    }
}

impl Metadata {
    /// The wire form.
    #[must_use]
    pub fn to_wire(&self) -> wire::FileMetadata {
        let kind = match self.kind {
            EntryKind::File => wire::EntryKind::File,
            EntryKind::Dir => wire::EntryKind::Dir,
            EntryKind::Symlink => wire::EntryKind::Symlink,
            EntryKind::Other => wire::EntryKind::Other,
        };
        wire::FileMetadata {
            kind: kind.into(),
            size: self.size,
            modified: self
                .modified
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .and_then(|d| {
                    Some(pbjson_types::Timestamp {
                        seconds: i64::try_from(d.as_secs()).ok()?,
                        nanos: i32::try_from(d.subsec_nanos()).ok()?,
                    })
                }),
        }
    }

    /// The metadata of a wire message.
    ///
    /// # Errors
    /// `InvalidArgument` for an unknown kind or time.
    pub fn from_wire(metadata: wire::FileMetadata) -> Result<Self, ModuleError> {
        let kind = match wire::EntryKind::try_from(metadata.kind) {
            Ok(wire::EntryKind::File) => EntryKind::File,
            Ok(wire::EntryKind::Dir) => EntryKind::Dir,
            Ok(wire::EntryKind::Symlink) => EntryKind::Symlink,
            Ok(wire::EntryKind::Other) => EntryKind::Other,
            Ok(wire::EntryKind::Unspecified) | Err(_) => return Err(malformed("an entry kind")),
        };
        let modified = match metadata.modified {
            None => None,
            Some(t) => {
                let secs = u64::try_from(t.seconds).map_err(|_| malformed("a time"))?;
                let nanos = u32::try_from(t.nanos).map_err(|_| malformed("a time"))?;
                Some(UNIX_EPOCH + Duration::new(secs, nanos))
            }
        };
        Ok(Self {
            kind,
            size: metadata.size,
            modified,
        })
    }

    /// The metadata a response field holds.
    ///
    /// # Errors
    /// `InvalidArgument` when it is unset or malformed.
    pub fn required(metadata: Option<wire::FileMetadata>) -> Result<Self, ModuleError> {
        Self::from_wire(metadata.ok_or_else(|| malformed("metadata is unset"))?)
    }
}

impl DirEntry {
    /// The wire form.
    #[must_use]
    pub fn to_wire(&self) -> wire::DirEntry {
        wire::DirEntry {
            name: self.name.clone(),
            metadata: Some(self.metadata.to_wire()),
        }
    }

    /// The entry of a wire message.
    ///
    /// # Errors
    /// As [`Metadata::required`].
    pub fn from_wire(entry: wire::DirEntry) -> Result<Self, ModuleError> {
        Ok(Self {
            name: entry.name,
            metadata: Metadata::required(entry.metadata)?,
        })
    }
}

impl ExecSpec {
    /// The wire form (a timeout past `u32::MAX` milliseconds, ~49 days,
    /// saturates).
    ///
    /// # Errors
    /// `InvalidArgument` for a working directory that is not UTF-8.
    pub fn to_wire(&self) -> Result<wire::ExecRequest, ModuleError> {
        Ok(wire::ExecRequest {
            program: self.program.clone(),
            args: self.args.clone(),
            cwd: self.cwd.as_ref().map(SandboxPath::to_wire).transpose()?,
            env: self
                .env
                .iter()
                .map(|(name, value)| wire::EnvVar {
                    name: name.clone(),
                    value: value.clone(),
                })
                .collect(),
            stdin: self.stdin.clone(),
            timeout_ms: u32::try_from(self.timeout.as_millis()).unwrap_or(u32::MAX),
            output_limit: u64::try_from(self.output_limit).unwrap_or(u64::MAX),
        })
    }

    /// The spec of a wire request.
    ///
    /// # Errors
    /// `InvalidArgument` for an output limit this platform cannot hold.
    pub fn from_wire(request: wire::ExecRequest) -> Result<Self, ModuleError> {
        Ok(Self {
            program: request.program,
            args: request.args,
            cwd: request.cwd.map(SandboxPath::from_wire),
            env: request.env.into_iter().map(|v| (v.name, v.value)).collect(),
            stdin: request.stdin,
            timeout: Duration::from_millis(u64::from(request.timeout_ms)),
            output_limit: usize::try_from(request.output_limit)
                .map_err(|_| malformed("an output limit"))?,
        })
    }
}

impl ExecEvent {
    /// The wire form.
    #[must_use]
    pub fn to_wire(self) -> wire::ExecReply {
        use wire::exec_exit::How;
        use wire::exec_reply::Event;
        let event = match self {
            Self::Stdout(bytes) => Event::Stdout(bytes),
            Self::Stderr(bytes) => Event::Stderr(bytes),
            Self::Exit(exit) => Event::Exit(wire::ExecExit {
                how: Some(match exit {
                    ExecExit::Code(code) => How::Code(code),
                    ExecExit::Signal => How::Signal(true),
                    ExecExit::TimedOut => How::TimedOut(true),
                    ExecExit::Cancelled => How::Cancelled(true),
                }),
            }),
        };
        wire::ExecReply { event: Some(event) }
    }

    /// The event of a wire reply.
    ///
    /// # Errors
    /// The reply's error; `InvalidArgument` for an empty one.
    pub fn from_wire(reply: wire::ExecReply) -> Result<Self, ModuleError> {
        use wire::exec_exit::How;
        use wire::exec_reply::Event;
        match reply.event {
            Some(Event::Stdout(bytes)) => Ok(Self::Stdout(bytes)),
            Some(Event::Stderr(bytes)) => Ok(Self::Stderr(bytes)),
            Some(Event::Exit(exit)) => Ok(Self::Exit(match exit.how {
                Some(How::Code(code)) => ExecExit::Code(code),
                Some(How::Signal(_)) => ExecExit::Signal,
                Some(How::TimedOut(_)) => ExecExit::TimedOut,
                Some(How::Cancelled(_)) => ExecExit::Cancelled,
                None => return Err(malformed("an exit without how")),
            })),
            Some(Event::Error(error)) => Err(error.into()),
            None => Err(malformed("an empty exec reply")),
        }
    }
}
