// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use super::*;

/// An error that wraps `cause`, as a transport's does.
#[derive(Debug)]
struct Wrapped(Box<dyn std::error::Error + Send + Sync>);

impl std::fmt::Display for Wrapped {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("transport error")
    }
}

impl std::error::Error for Wrapped {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.0.as_ref())
    }
}

fn failed(cause: impl std::error::Error + Send + Sync + 'static) -> ModuleError {
    ChannelError(Box::new(Wrapped(Box::new(cause)))).into()
}

#[test]
fn a_channel_failure_is_read_from_its_causes() {
    // The channel's own timeout (its request's grpc-timeout) is the deadline.
    assert_eq!(
        failed(tonic::TimeoutExpired(())).kind,
        ModuleErrorKind::DeadlineExceeded
    );
    // A status in the chain is the module's answer.
    let denied = failed(Status::permission_denied("not yours"));
    assert_eq!(denied.kind, ModuleErrorKind::Denied);
    assert_eq!(denied.message, "not yours");
    // Anything else: unavailable, with every cause.
    let broken = failed(std::io::Error::other("connection reset"));
    assert_eq!(broken.kind, ModuleErrorKind::Unavailable);
    assert_eq!(broken.message, "transport error: connection reset");
}
