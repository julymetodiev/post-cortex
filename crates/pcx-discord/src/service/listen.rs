// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! One connection to the Gateway, from its hello to its end: the frames
//! answered as the session's rules say, a heartbeat at the interval the
//! hello names, each event of the server heard.

use std::time::Duration;

use pcx_module_api::{ModuleError, ModuleErrorKind};
use tokio::time::{Instant, Interval, MissedTickBehavior, interval_at};
use tokio_util::sync::CancellationToken;

use crate::domain::gateway::{Next, Session};
use crate::ports::Gateway;
use crate::service::Service;

/// The next heartbeat's time; never, before the hello.
async fn tick(beat: &mut Option<Interval>) {
    match beat {
        Some(beat) => {
            beat.tick().await;
        }
        None => std::future::pending().await,
    }
}

impl Service<'_> {
    /// Listen on one connection of `gateway`, going on with `session`,
    /// until `stop`, or the Gateway ends the connection or asks for a new
    /// one: then it returns, and the next call connects again.
    ///
    /// # Errors
    /// The Gateway could not be reached, the connection broke, or a frame
    /// is not the Gateway's.
    pub async fn listen(
        &self,
        gateway: &dyn Gateway,
        session: &mut Session,
        stop: &CancellationToken,
    ) -> Result<(), ModuleError> {
        let url = match session.resumed_at() {
            Some(url) => url.to_string(),
            None => self.rest.gateway().await?,
        };
        let mut link = gateway.connect(&url).await?;
        let mut beat: Option<Interval> = None;
        loop {
            let next = tokio::select! {
                () = stop.cancelled() => return Ok(()),
                () = tick(&mut beat) => session.beat(),
                frame = link.next() => match frame? {
                    Some(frame) => session
                        .on(&self.config.bot_token, frame)
                        .map_err(|e| ModuleError::new(ModuleErrorKind::Unavailable, e.to_string()))?,
                    None => return Ok(()),
                },
            };
            match next {
                Next::Nothing => {}
                Next::Begin { send, every_ms } => {
                    link.send(&send).await?;
                    let every = Duration::from_millis(u64::from(every_ms));
                    let mut ticks = interval_at(Instant::now() + every, every);
                    ticks.set_missed_tick_behavior(MissedTickBehavior::Delay);
                    beat = Some(ticks);
                }
                Next::Send(sent) => link.send(&sent).await?,
                Next::Event(event, d) => {
                    // One event that cannot be read or heard does not end
                    // the connection.
                    if let Err(e) = self.heard(&event, d).await {
                        tracing::warn!(event = %event, error = %e, "an event of Discord was not heard");
                    }
                }
                Next::Reconnect => return Ok(()),
            }
        }
    }
}
