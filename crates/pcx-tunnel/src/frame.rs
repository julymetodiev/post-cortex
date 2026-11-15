//! The frames both sides send alike, each side's own frames, and headers
//! on the wire.

use http::{HeaderMap, HeaderName, HeaderValue};
use post_cortex_proto::tunnel::{
    Credit, DaemonFrame, Data, End, GatewayFrame, Head, Header, Open, Ping, Pong, Reset, Welcome,
    daemon_frame, gateway_frame,
};

use crate::TunnelError;

/// A frame either side sends.
pub enum Common {
    /// A body's next bytes.
    Data(Data),
    /// A body's end.
    End(End),
    /// An exchange ends early.
    Reset(Reset),
    /// More of a body may come.
    Credit(Credit),
    /// Liveness.
    Ping(Ping),
    /// The answer to a `Ping`.
    Pong(Pong),
}

/// What one side sends: a [`Common`] frame wrapped as this side's.
pub trait Outgoing: Send + 'static {
    /// `common` as this side's frame.
    fn wrap(common: Common) -> Self;
}

/// A frame of the other side: a [`Common`] one, or one of its own.
pub enum Received<T> {
    /// A frame both sides send.
    Common(Common),
    /// A frame of that side only.
    Own(T),
}

/// What one side receives.
pub trait Inbound: Sized {
    /// The frames only the sending side sends.
    type Own;
    /// What the frame is; `None` for an empty one.
    fn received(self) -> Option<Received<Self::Own>>;
}

/// The daemon's own frames.
pub enum FromDaemon {
    /// A `Hello`: only its first frame is one, read before the session.
    Hello,
    /// An answer begins.
    Head(Head),
}

/// The gateway's own frames.
pub enum FromGateway {
    /// Its first frame.
    Welcome(Welcome),
    /// A request begins.
    Open(Open),
}

impl Outgoing for DaemonFrame {
    fn wrap(common: Common) -> Self {
        use daemon_frame::Frame;
        let frame = match common {
            Common::Data(f) => Frame::Data(f),
            Common::End(f) => Frame::End(f),
            Common::Reset(f) => Frame::Reset(f),
            Common::Credit(f) => Frame::Credit(f),
            Common::Ping(f) => Frame::Ping(f),
            Common::Pong(f) => Frame::Pong(f),
        };
        Self { frame: Some(frame) }
    }
}

/// The gateway answers the daemon's call with a stream of these: a frame,
/// or the failure that ends the call.
impl Outgoing for Result<GatewayFrame, tonic::Status> {
    fn wrap(common: Common) -> Self {
        use gateway_frame::Frame;
        let frame = match common {
            Common::Data(f) => Frame::Data(f),
            Common::End(f) => Frame::End(f),
            Common::Reset(f) => Frame::Reset(f),
            Common::Credit(f) => Frame::Credit(f),
            Common::Ping(f) => Frame::Ping(f),
            Common::Pong(f) => Frame::Pong(f),
        };
        Ok(GatewayFrame { frame: Some(frame) })
    }
}

impl Inbound for DaemonFrame {
    type Own = FromDaemon;

    fn received(self) -> Option<Received<FromDaemon>> {
        use daemon_frame::Frame;
        Some(match self.frame? {
            Frame::Hello(_) => Received::Own(FromDaemon::Hello),
            Frame::Head(f) => Received::Own(FromDaemon::Head(f)),
            Frame::Data(f) => Received::Common(Common::Data(f)),
            Frame::End(f) => Received::Common(Common::End(f)),
            Frame::Reset(f) => Received::Common(Common::Reset(f)),
            Frame::Credit(f) => Received::Common(Common::Credit(f)),
            Frame::Ping(f) => Received::Common(Common::Ping(f)),
            Frame::Pong(f) => Received::Common(Common::Pong(f)),
        })
    }
}

impl Inbound for GatewayFrame {
    type Own = FromGateway;

    fn received(self) -> Option<Received<FromGateway>> {
        use gateway_frame::Frame;
        Some(match self.frame? {
            Frame::Welcome(f) => Received::Own(FromGateway::Welcome(f)),
            Frame::Open(f) => Received::Own(FromGateway::Open(f)),
            Frame::Data(f) => Received::Common(Common::Data(f)),
            Frame::End(f) => Received::Common(Common::End(f)),
            Frame::Reset(f) => Received::Common(Common::Reset(f)),
            Frame::Credit(f) => Received::Common(Common::Credit(f)),
            Frame::Ping(f) => Received::Common(Common::Ping(f)),
            Frame::Pong(f) => Received::Common(Common::Pong(f)),
        })
    }
}

/// `headers` on the wire.
pub fn to_wire(headers: &HeaderMap) -> Vec<Header> {
    headers
        .iter()
        .map(|(name, value)| Header {
            name: name.as_str().to_owned(),
            value: bytes::Bytes::copy_from_slice(value.as_bytes()),
        })
        .collect()
}

/// Headers from the wire.
///
/// # Errors
/// [`TunnelError::Http`] for a name or value HTTP does not allow.
pub fn from_wire(headers: Vec<Header>) -> Result<HeaderMap, TunnelError> {
    let mut map = HeaderMap::with_capacity(headers.len());
    for header in headers {
        let name = HeaderName::from_bytes(header.name.as_bytes())
            .map_err(|e| TunnelError::Http(format!("header {}: {e}", header.name)))?;
        let value = HeaderValue::from_maybe_shared(header.value)
            .map_err(|e| TunnelError::Http(format!("header {name}: {e}")))?;
        map.append(name, value);
    }
    Ok(map)
}
