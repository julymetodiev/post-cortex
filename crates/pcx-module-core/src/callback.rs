// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! A module process's callback socket, the one the host names in
//! `StartRequest.host_socket`: the channel its calls to the host (the
//! modules it uses, the bus, the host's services) go through.

use std::path::PathBuf;

use hyper_util::rt::TokioIo;
use tokio::net::UnixStream;
use tonic::transport::{Channel, Endpoint, Uri};

use crate::grpc::GrpcChannel;
use crate::{ModuleError, ModuleErrorKind};

/// Why there is no channel.
const NO_SOCKET: &str = "the host gives this module no callback socket";

/// The callback socket; none when the host named none.
#[derive(Clone)]
pub struct Callback {
    channel: Option<Channel>,
}

impl Callback {
    /// The socket `socket` (empty: the host offers none). It connects on
    /// its first call.
    #[must_use]
    pub fn new(socket: &str) -> Self {
        if socket.is_empty() {
            return Self { channel: None };
        }
        let path = PathBuf::from(socket);
        // The URI is not used: every connection is the socket.
        let channel = Endpoint::from_static("http://host.sock").connect_with_connector_lazy(
            tower::service_fn(move |_: Uri| {
                let path = path.clone();
                async move { UnixStream::connect(path).await.map(TokioIo::new) }
            }),
        );
        Self {
            channel: Some(channel),
        }
    }

    /// Its gRPC channel (W23): the host routes each call to the module
    /// that serves it; one that refuses every call when the host named no
    /// socket.
    #[must_use]
    pub fn grpc(&self) -> GrpcChannel {
        match &self.channel {
            Some(channel) => crate::grpc::channel(channel.clone()),
            None => crate::grpc::refusing(tonic::Status::failed_precondition(NO_SOCKET)),
        }
    }

    /// Its tonic channel, for a generated client of the host's services.
    ///
    /// # Errors
    /// `FailedPrecondition` when the host named no socket.
    pub fn channel(&self) -> Result<Channel, ModuleError> {
        self.channel
            .clone()
            .ok_or_else(|| ModuleError::new(ModuleErrorKind::FailedPrecondition, NO_SOCKET))
    }
}
