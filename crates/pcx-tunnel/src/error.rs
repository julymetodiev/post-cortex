//! How a tunnel, or one exchange of it, fails.

/// A failure of the tunnel or of one exchange.
#[derive(Debug, thiserror::Error)]
pub enum TunnelError {
    /// The tunnel is closed: the daemon left, or the connection broke.
    #[error("the tunnel is closed")]
    Closed,
    /// As many exchanges as allowed are open already.
    #[error("{0} exchanges are open already")]
    Busy(u32),
    /// The other side ended the exchange early.
    #[error("the exchange was reset: {0}")]
    Reset(String),
    /// A method, path, status or header that HTTP does not allow.
    #[error("invalid HTTP: {0}")]
    Http(String),
    /// The other side broke the protocol.
    #[error("the tunnel's protocol was broken: {0}")]
    Protocol(String),
    /// Nothing came from the other side for too long.
    #[error("nothing came over the tunnel for {0} seconds")]
    Silent(u64),
    /// The gateway could not be reached.
    #[error("connecting to the gateway: {0}")]
    Connect(#[from] tonic::transport::Error),
    /// The other side answered the call with a failure.
    #[error("the tunnel's call failed: {0}")]
    Status(Box<tonic::Status>),
}

impl From<tonic::Status> for TunnelError {
    fn from(status: tonic::Status) -> Self {
        Self::Status(Box::new(status))
    }
}
