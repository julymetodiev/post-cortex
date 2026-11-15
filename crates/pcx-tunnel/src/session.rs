//! Reading the other side's frames while a tunnel lives: liveness, the
//! exchanges' frames, and the side's own frames.

use std::sync::Arc;
use std::time::Duration;

use futures::{Stream, StreamExt};
use post_cortex_proto::tunnel::{Ping, Pong};
use tokio::time::{Instant, MissedTickBehavior};
use tokio_util::sync::CancellationToken;

use crate::TunnelError;
use crate::exchanges::Exchanges;
use crate::frame::{Common, Inbound, Outgoing, Received};

/// A `Ping` each way this often.
const PING: Duration = Duration::from_secs(20);

/// Three pings without a frame: the tunnel is dead.
const SILENCE: Duration = Duration::from_secs(60);

/// Read `inbound` until it ends, `ct` is canceled, or the other side is
/// silent too long; the exchanges' frames go to `exchanges`, the side's own
/// to `own`.
///
/// # Errors
/// The call's failure, a silent other side, or what `own` fails with.
pub async fn read<I, O, S>(
    mut inbound: S,
    exchanges: Arc<Exchanges<O>>,
    mut own: impl FnMut(I::Own) -> Result<(), TunnelError>,
    ct: CancellationToken,
) -> Result<(), TunnelError>
where
    I: Inbound,
    O: Outgoing,
    S: Stream<Item = Result<I, tonic::Status>> + Unpin,
{
    let mut ping = tokio::time::interval_at(Instant::now() + PING, PING);
    ping.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let mut last = Instant::now();
    let mut nonce = 0u64;
    loop {
        let next = tokio::select! {
            () = ct.cancelled() => return Ok(()),
            _ = ping.tick() => {
                if last.elapsed() > SILENCE {
                    return Err(TunnelError::Silent(SILENCE.as_secs()));
                }
                nonce += 1;
                exchanges.send(Common::Ping(Ping { nonce }));
                continue;
            }
            next = inbound.next() => next,
        };
        let frame = match next {
            None => return Err(TunnelError::Closed),
            Some(Err(status)) => return Err(status.into()),
            Some(Ok(frame)) => frame,
        };
        last = Instant::now();
        match frame.received() {
            None => return Err(TunnelError::Protocol("an empty frame".into())),
            Some(Received::Common(Common::Ping(Ping { nonce }))) => {
                exchanges.send(Common::Pong(Pong { nonce }));
            }
            Some(Received::Common(Common::Pong(_))) => {}
            Some(Received::Common(common)) => exchanges.receive(common),
            Some(Received::Own(frame)) => own(frame)?,
        }
    }
}
