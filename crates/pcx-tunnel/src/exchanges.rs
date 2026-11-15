//! The exchanges open on one tunnel, for either side: the other side's
//! body as an [`Incoming`] body, this side's body sent within the other
//! side's grants ([`send_body`]), and what each frame of the other side
//! does to them (`docs/connect.md` section 2, flow control).

use std::collections::HashMap;
use std::fmt::Display;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use bytes::Bytes;
use http::HeaderMap;
use http_body::{Body, Frame};
use parking_lot::Mutex;
use post_cortex_proto::tunnel::{Credit, Data, End, Head, Reset};
use tokio::sync::{OwnedSemaphorePermit, Semaphore, mpsc, oneshot};
use tokio::task::AbortHandle;
use tracing::debug;

use crate::TunnelError;
use crate::frame::{Common, Outgoing, from_wire, to_wire};

/// The most body bytes in one `Data` frame.
const CHUNK: u32 = 64 * 1024;

/// What an [`Incoming`] body dropped before its end does to its exchange.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unread {
    /// End the exchange: nobody wants the answer any more (the gateway's
    /// side, whose caller went away).
    Reset,
    /// Take the rest of the body and throw it away, so the answer goes on
    /// (the daemon's side, whose router answered without reading the whole
    /// request).
    Drain,
}

/// What the other side sent of its body.
enum Piece {
    Data(Bytes),
    End(HeaderMap),
    Reset(String),
}

/// One open exchange.
struct Entry {
    /// Where the other side's body goes; `None` once it ended or is drained.
    body: Option<mpsc::UnboundedSender<Piece>>,
    /// The other side's body is thrown away as it comes.
    draining: bool,
    /// Bytes of the other side's body received and not yet consumed.
    unconsumed: usize,
    /// The grants for this side's body.
    credit: Arc<Semaphore>,
    /// The answer's head, awaited by the gateway's request.
    head: Option<oneshot::Sender<Result<Head, TunnelError>>>,
    /// This side's half: the daemon's call and its answer's body, the
    /// gateway's request's body.
    task: Option<AbortHandle>,
    /// Halves still open: the other side's body and this side's.
    open: u8,
}

/// The open exchanges of a tunnel; `O` is this side's frame.
pub struct Exchanges<O> {
    entries: Mutex<HashMap<u64, Entry>>,
    /// The frames to the other side; `None` once the tunnel closed.
    out: Mutex<Option<mpsc::UnboundedSender<O>>>,
    window: u32,
    unread: Unread,
}

impl<O: Outgoing> Exchanges<O> {
    /// No exchange yet, frames going to `out`, `window` bytes of a body
    /// sent before a grant, an unread body doing as `unread` says.
    pub fn new(out: mpsc::UnboundedSender<O>, window: u32, unread: Unread) -> Arc<Self> {
        Arc::new(Self {
            entries: Mutex::new(HashMap::new()),
            out: Mutex::new(Some(out)),
            window,
            unread,
        })
    }

    /// Send `frame` to the other side; false once the tunnel is closed.
    pub fn send_frame(&self, frame: O) -> bool {
        self.out
            .lock()
            .as_ref()
            .is_some_and(|out| out.send(frame).is_ok())
    }

    /// Send a frame both sides send.
    pub fn send(&self, common: Common) -> bool {
        self.send_frame(O::wrap(common))
    }

    /// Open `exchange`: the other side's body and the grants for this
    /// side's. `hold` is kept until the body ends (the gateway's slot).
    ///
    /// # Errors
    /// [`TunnelError::Protocol`] when it is open already.
    pub fn open(
        self: &Arc<Self>,
        exchange: u64,
        hold: Option<OwnedSemaphorePermit>,
    ) -> Result<(Incoming<O>, Arc<Semaphore>), TunnelError> {
        let (tx, rx) = mpsc::unbounded_channel();
        let credit = Arc::new(Semaphore::new(self.window as usize));
        let mut entries = self.entries.lock();
        if entries.contains_key(&exchange) {
            return Err(TunnelError::Protocol(format!(
                "exchange {exchange} is open already"
            )));
        }
        entries.insert(
            exchange,
            Entry {
                body: Some(tx),
                draining: false,
                unconsumed: 0,
                credit: Arc::clone(&credit),
                head: None,
                task: None,
                open: 2,
            },
        );
        let body = Incoming {
            exchange,
            rx,
            exchanges: Arc::clone(self),
            done: false,
            _hold: hold,
        };
        Ok((body, credit))
    }

    /// The answer's head of `exchange`, when it comes.
    pub fn expect_head(&self, exchange: u64) -> oneshot::Receiver<Result<Head, TunnelError>> {
        let (tx, rx) = oneshot::channel();
        if let Some(entry) = self.entries.lock().get_mut(&exchange) {
            entry.head = Some(tx);
        }
        rx
    }

    /// The other side's answer begins.
    pub fn head(&self, head: Head) {
        let waiting = self
            .entries
            .lock()
            .get_mut(&head.exchange)
            .and_then(|e| e.head.take());
        match waiting {
            Some(tx) => {
                if tx.send(Ok(head)).is_err() {
                    debug!("an answer came for a request nobody waits for");
                }
            }
            None => debug!(exchange = head.exchange, "a head for no request"),
        }
    }

    /// This side's half of `exchange` runs as `task`, aborted when the
    /// exchange is reset.
    pub fn track(&self, exchange: u64, task: AbortHandle) {
        match self.entries.lock().get_mut(&exchange) {
            Some(entry) => entry.task = Some(task),
            // Reset already.
            None => task.abort(),
        }
    }

    /// This side's body of `exchange` was sent whole.
    fn sent(&self, exchange: u64) {
        self.finish_half(exchange);
    }

    /// One half of `exchange` ended; it is gone once both have.
    fn finish_half(&self, exchange: u64) {
        let mut entries = self.entries.lock();
        if let Some(entry) = entries.get_mut(&exchange) {
            entry.open = entry.open.saturating_sub(1);
            if entry.open == 0 {
                entries.remove(&exchange);
            }
        }
    }

    /// End `exchange` at once: its task is aborted, its body fails with
    /// `reason`, and the other side is told when `tell`.
    pub fn reset(&self, exchange: u64, reason: &str, tell: bool) {
        let entry = self.entries.lock().remove(&exchange);
        if let Some(entry) = entry {
            fail(entry, || TunnelError::Reset(reason.to_owned()));
        }
        if tell {
            self.send(Common::Reset(Reset {
                exchange,
                reason: reason.to_owned(),
            }));
        }
    }

    /// The other side's frame of an exchange.
    pub fn receive(&self, common: Common) {
        match common {
            Common::Data(Data { exchange, data }) => self.data(exchange, data),
            Common::End(End { exchange, trailers }) => self.end(exchange, trailers),
            Common::Reset(Reset { exchange, reason }) => self.reset(exchange, &reason, false),
            Common::Credit(Credit { exchange, bytes }) => {
                if let Some(entry) = self.entries.lock().get(&exchange) {
                    entry.credit.add_permits(bytes as usize);
                }
            }
            // Liveness is the session's.
            Common::Ping(_) | Common::Pong(_) => {}
        }
    }

    fn data(&self, exchange: u64, data: Bytes) {
        let len = data.len();
        let mut entries = self.entries.lock();
        // An exchange reset or ended already: its late bytes are dropped.
        let Some(entry) = entries.get_mut(&exchange) else {
            return;
        };
        let within = entry
            .unconsumed
            .checked_add(len)
            .filter(|n| *n <= self.window as usize);
        if entry.draining && within.is_some() {
            drop(entries);
            // Within the window, so it fits.
            self.send(Common::Credit(Credit {
                exchange,
                bytes: len as u32,
            }));
            return;
        }
        match (within, &entry.body) {
            (Some(n), Some(body)) => {
                entry.unconsumed = n;
                if body.send(Piece::Data(data)).is_err() {
                    debug!(exchange, "bytes for a body dropped already");
                }
            }
            (None, _) => {
                drop(entries);
                self.reset(exchange, "more body bytes than granted", true);
            }
            (_, None) => {
                drop(entries);
                self.reset(exchange, "body bytes after the body's end", true);
            }
        }
    }

    fn end(&self, exchange: u64, trailers: Vec<post_cortex_proto::tunnel::Header>) {
        let piece = match from_wire(trailers) {
            Ok(trailers) => Piece::End(trailers),
            Err(e) => {
                self.reset(exchange, &e.to_string(), true);
                return;
            }
        };
        let body = self.entries.lock().get_mut(&exchange).and_then(|entry| {
            entry.draining = false;
            entry.body.take()
        });
        if let Some(body) = body
            && body.send(piece).is_err()
        {
            debug!(exchange, "the end of a body dropped already");
        }
        self.finish_half(exchange);
    }

    /// `bytes` of `exchange`'s body were consumed: the other side may
    /// send that much more.
    fn consumed(&self, exchange: u64, bytes: usize) {
        if let Some(entry) = self.entries.lock().get_mut(&exchange) {
            entry.unconsumed = entry.unconsumed.saturating_sub(bytes);
        }
        // A piece is never more than the window, so it fits.
        self.send(Common::Credit(Credit {
            exchange,
            bytes: bytes as u32,
        }));
    }

    /// The body of `exchange` was dropped before its end.
    fn unread(&self, exchange: u64) {
        match self.unread {
            Unread::Reset => self.reset(exchange, "nobody reads the answer", true),
            Unread::Drain => {
                let unconsumed = self.entries.lock().get_mut(&exchange).and_then(|entry| {
                    entry.body.take()?;
                    entry.draining = true;
                    Some(std::mem::take(&mut entry.unconsumed))
                });
                // Never more than the window, so it fits.
                if let Some(bytes) = unconsumed.filter(|b| *b > 0) {
                    self.send(Common::Credit(Credit {
                        exchange,
                        bytes: bytes as u32,
                    }));
                }
            }
        }
    }

    /// The tunnel closed: every exchange fails and nothing more is sent.
    pub fn close(&self) {
        self.out.lock().take();
        let entries: Vec<Entry> = self.entries.lock().drain().map(|(_, e)| e).collect();
        for entry in entries {
            fail(entry, || TunnelError::Closed);
        }
    }
}

/// End an exchange removed from the table: its body and its head fail
/// with `error`, its task stops.
fn fail(entry: Entry, error: impl Fn() -> TunnelError) {
    if let Some(body) = entry.body {
        let why = match error() {
            TunnelError::Reset(reason) => reason,
            other => other.to_string(),
        };
        if body.send(Piece::Reset(why)).is_err() {
            debug!("a failed exchange's body was dropped already");
        }
    }
    if let Some(head) = entry.head
        && head.send(Err(error())).is_err()
    {
        debug!("a failed exchange's request was dropped already");
    }
    if let Some(task) = entry.task {
        task.abort();
    }
    entry.credit.close();
}

/// Send `body` as `exchange`'s, within the other side's grants (`credit`).
pub async fn send_body<O, B>(
    exchanges: Arc<Exchanges<O>>,
    exchange: u64,
    credit: Arc<Semaphore>,
    body: B,
) where
    O: Outgoing,
    B: Body<Data = Bytes>,
    B::Error: Display,
{
    let chunk = CHUNK.min(exchanges.window).max(1);
    let mut body = std::pin::pin!(body);
    loop {
        let frame = match std::future::poll_fn(|cx| body.as_mut().poll_frame(cx)).await {
            None => {
                exchanges.send(Common::End(End {
                    exchange,
                    trailers: Vec::new(),
                }));
                exchanges.sent(exchange);
                return;
            }
            Some(Err(e)) => {
                exchanges.reset(exchange, &e.to_string(), true);
                return;
            }
            Some(Ok(frame)) => frame,
        };
        match frame.into_data() {
            Ok(mut data) => {
                while !data.is_empty() {
                    let n = if data.len() < chunk as usize {
                        data.len() as u32
                    } else {
                        chunk
                    };
                    match credit.acquire_many(n).await {
                        Ok(granted) => granted.forget(),
                        // The exchange ended.
                        Err(_closed) => return,
                    }
                    let piece = data.split_to(n as usize);
                    if !exchanges.send(Common::Data(Data {
                        exchange,
                        data: piece,
                    })) {
                        return;
                    }
                }
            }
            Err(frame) => {
                if let Ok(trailers) = frame.into_trailers() {
                    exchanges.send(Common::End(End {
                        exchange,
                        trailers: to_wire(&trailers),
                    }));
                    exchanges.sent(exchange);
                    return;
                }
            }
        }
    }
}

/// The other side's body of an exchange: a request's on the daemon, an
/// answer's on the gateway.
pub struct Incoming<O: Outgoing> {
    exchange: u64,
    rx: mpsc::UnboundedReceiver<Piece>,
    exchanges: Arc<Exchanges<O>>,
    done: bool,
    /// Kept while the body is (the gateway's slot of the exchange).
    _hold: Option<OwnedSemaphorePermit>,
}

impl<O: Outgoing> std::fmt::Debug for Incoming<O> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Incoming")
            .field("exchange", &self.exchange)
            .field("done", &self.done)
            .finish_non_exhaustive()
    }
}

impl<O: Outgoing> Body for Incoming<O> {
    type Data = Bytes;
    type Error = TunnelError;

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, TunnelError>>> {
        let this = self.get_mut();
        if this.done {
            return Poll::Ready(None);
        }
        let piece = match this.rx.poll_recv(cx) {
            Poll::Pending => return Poll::Pending,
            Poll::Ready(piece) => piece,
        };
        Poll::Ready(match piece {
            Some(Piece::Data(data)) => {
                this.exchanges.consumed(this.exchange, data.len());
                Some(Ok(Frame::data(data)))
            }
            Some(Piece::End(trailers)) => {
                this.done = true;
                (!trailers.is_empty()).then(|| Ok(Frame::trailers(trailers)))
            }
            Some(Piece::Reset(why)) => {
                this.done = true;
                Some(Err(TunnelError::Reset(why)))
            }
            None => {
                this.done = true;
                Some(Err(TunnelError::Closed))
            }
        })
    }

    fn is_end_stream(&self) -> bool {
        self.done
    }
}

impl<O: Outgoing> Drop for Incoming<O> {
    fn drop(&mut self) {
        if !self.done {
            self.exchanges.unread(self.exchange);
        }
    }
}
