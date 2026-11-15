// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! gRPC between modules and their host (ARCHITECTURE section 7, everything
//! gRPC): a module's services and its channel to the host are one type,
//! [`GrpcChannel`], a cloneable tower service of gRPC calls. A module serves
//! its generated tonic services as one ([`routes`]); it calls the services
//! of the modules it uses with generated clients over the one its host gives
//! it (`HostContext.channel`), and the host routes each call by its path.

use std::convert::Infallible;

use prost::bytes::{Buf, BufMut};
use tonic::Status;
use tonic::body::Body;
use tonic::codec::{Codec, DecodeBuf, Decoder, EncodeBuf, Encoder};
use tonic::codegen::http::uri::PathAndQuery;
use tonic::codegen::http::{Request, Response};
use tower::util::BoxCloneSyncService;
use tower::{Service, ServiceExt};

use crate::{ModuleError, ModuleErrorKind};

/// An error of a gRPC channel (the transport's, or a routed call's). A
/// type of its own, not a boxed trait object, so a generated client over a
/// [`GrpcChannel`] is `Send` in every async block that holds it.
#[derive(Debug)]
pub struct ChannelError(Box<dyn std::error::Error + Send + Sync>);

impl std::fmt::Display for ChannelError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl std::error::Error for ChannelError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.0.as_ref())
    }
}

/// Why a call through a channel failed, from its error's chain: the
/// status in it; its deadline (the channel keeps a request's
/// `grpc-timeout`) as `DeadlineExceeded`; anything else `Unavailable`,
/// with every cause.
impl From<ChannelError> for ModuleError {
    fn from(error: ChannelError) -> Self {
        let mut causes = Vec::new();
        let mut cause: Option<&(dyn std::error::Error + 'static)> = Some(error.0.as_ref());
        while let Some(e) = cause {
            if let Some(status) = e.downcast_ref::<Status>() {
                return Status::new(status.code(), status.message()).into();
            }
            if e.is::<tonic::TimeoutExpired>() {
                return ModuleError::new(
                    ModuleErrorKind::DeadlineExceeded,
                    "the call's deadline passed before it was answered",
                );
            }
            causes.push(e.to_string());
            cause = e.source();
        }
        ModuleError::new(ModuleErrorKind::Unavailable, causes.join(": "))
    }
}

/// gRPC calls as a tower service: what a module serves, and what it calls
/// its host through. A generated client takes it as its channel.
pub type GrpcChannel = BoxCloneSyncService<Request<Body>, Response<Body>, ChannelError>;

/// `service` as a channel.
pub fn channel<S>(service: S) -> GrpcChannel
where
    S: Service<Request<Body>, Response = Response<Body>> + Clone + Send + Sync + 'static,
    S::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
    S::Future: Send + 'static,
{
    BoxCloneSyncService::new(service.map_err(|e| ChannelError(e.into())))
}

/// A module's generated tonic services as its channel.
#[must_use]
pub fn routes(routes: tonic::service::Routes) -> GrpcChannel {
    channel(routes)
}

/// The answer of a call that fails with `status`.
#[must_use]
pub fn refusal(status: &Status) -> Response<Body> {
    let (parts, ()) = status.clone().into_http::<()>().into_parts();
    Response::from_parts(parts, Body::empty())
}

/// A channel that refuses every call with `status` (a module process whose
/// host gave it no callback socket).
#[must_use]
pub fn refusing(status: Status) -> GrpcChannel {
    channel(tower::service_fn(move |_: Request<Body>| {
        let answer = refusal(&status);
        async move { Ok::<_, Infallible>(answer) }
    }))
}

/// `channel` as the fallback of an axum router: the calls no service of
/// the router takes go to it, and its failure answers as
/// [`ModuleError`]'s from a [`ChannelError`].
pub fn fallback(
    channel: GrpcChannel,
) -> impl Service<
    Request<axum::body::Body>,
    Response = Response<Body>,
    Error = Infallible,
    Future = impl Send + 'static,
> + Clone
+ Send
+ Sync
+ 'static {
    tower::service_fn(move |request: Request<axum::body::Body>| {
        let channel = channel.clone();
        async move {
            Ok::<_, Infallible>(match channel.oneshot(request.map(Body::new)).await {
                Ok(answer) => answer,
                Err(e) => refusal(&Status::from(ModuleError::from(e))),
            })
        }
    })
}

/// `routes` with the calls none of its services take sent to `others`.
#[must_use]
pub fn with_fallback(
    routes: tonic::service::Routes,
    others: GrpcChannel,
) -> tonic::service::Routes {
    tonic::service::Routes::from(routes.into_axum_router().fallback_service(fallback(others)))
}

/// Call `path` through `channel` with `request`, a message's bytes (the
/// caller sets its metadata): the answer's bytes, unread.
///
/// # Errors
/// `Unavailable` when the channel is not ready; the call's status.
pub async fn unary(
    channel: GrpcChannel,
    path: PathAndQuery,
    request: tonic::Request<Vec<u8>>,
) -> Result<Vec<u8>, ModuleError> {
    let mut grpc = tonic::client::Grpc::new(channel);
    grpc.ready().await.map_err(|e| {
        ModuleError::new(
            ModuleErrorKind::Unavailable,
            format!("the channel to the host is not ready for {path}: {e}"),
        )
    })?;
    Ok(grpc.unary(request, path, Raw).await?.into_inner())
}

/// The bytes of a message, unread: a call encodes and decodes with the
/// descriptors or in another language, not with a generated type.
#[derive(Clone, Copy, Default)]
pub struct Raw;

impl Codec for Raw {
    type Encode = Vec<u8>;
    type Decode = Vec<u8>;
    type Encoder = Raw;
    type Decoder = Raw;

    fn encoder(&mut self) -> Self::Encoder {
        Raw
    }

    fn decoder(&mut self) -> Self::Decoder {
        Raw
    }
}

impl Encoder for Raw {
    type Item = Vec<u8>;
    type Error = Status;

    fn encode(&mut self, item: Vec<u8>, dst: &mut EncodeBuf<'_>) -> Result<(), Self::Error> {
        dst.put_slice(&item);
        Ok(())
    }
}

impl Decoder for Raw {
    type Item = Vec<u8>;
    type Error = Status;

    fn decode(&mut self, src: &mut DecodeBuf<'_>) -> Result<Option<Vec<u8>>, Self::Error> {
        Ok(Some(src.copy_to_bytes(src.remaining()).to_vec()))
    }
}

#[cfg(test)]
mod tests;
