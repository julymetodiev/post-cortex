// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The Gateway's rules (docs/modules/discord.md, what it hears): what each
//! frame of Discord's WebSocket asks next. A connection says hello; the bot
//! identifies, or resumes the session a cut left; it sends a heartbeat at
//! the interval the hello names and expects each acknowledged; events come
//! as dispatches, each with a sequence number a resume names. Pure: the
//! socket and the clock are the service's.

use post_cortex_proto::discord::{ConnectionProperties, Hello, Identify, Ready, Resume};
use serde::Deserialize;
use serde_json::Value;

/// The events it asks for: `GUILDS` (threads), `GUILD_MEMBERS`,
/// `GUILD_MESSAGES`, `MESSAGE_CONTENT`, `AUTO_MODERATION_EXECUTION`.
pub const INTENTS: u32 = 1 | 1 << 1 | 1 << 9 | 1 << 15 | 1 << 21;

/// What the bot calls itself to the Gateway.
const LIBRARY: &str = "pcx";

/// The Gateway's operations.
mod op {
    pub const DISPATCH: u8 = 0;
    pub const HEARTBEAT: u8 = 1;
    pub const IDENTIFY: u8 = 2;
    pub const RESUME: u8 = 6;
    pub const RECONNECT: u8 = 7;
    pub const INVALID_SESSION: u8 = 9;
    pub const HELLO: u8 = 10;
    pub const HEARTBEAT_ACK: u8 = 11;
}

/// A frame the Gateway sends: its envelope, whose `d` is read by the
/// operation and the event it names.
#[derive(Debug, Clone, Deserialize)]
pub struct Frame {
    /// The operation.
    pub op: u8,
    /// What it carries.
    #[serde(default)]
    pub d: Value,
    /// A dispatch's sequence number.
    #[serde(default)]
    pub s: Option<u32>,
    /// A dispatch's event.
    #[serde(default)]
    pub t: Option<String>,
}

/// What the bot sends: an operation and what it carries.
#[derive(Debug, Clone, PartialEq)]
pub struct Sent {
    /// The operation.
    pub op: u8,
    /// What it carries.
    pub d: Value,
}

/// What a frame asks next.
#[derive(Debug, Clone, PartialEq)]
pub enum Next {
    /// Nothing.
    Nothing,
    /// Send this, and from now on a heartbeat every so many milliseconds.
    Begin {
        /// The identify or the resume.
        send: Sent,
        /// Milliseconds between heartbeats.
        every_ms: u32,
    },
    /// Send this.
    Send(Sent),
    /// An event of the server, by its name, with what it carries.
    Event(String, Value),
    /// Close the connection and connect again.
    Reconnect,
}

/// A frame that is not what the Gateway sends.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Malformed(pub String);

impl std::fmt::Display for Malformed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "the Gateway sent what is not its frame: {}", self.0)
    }
}

impl std::error::Error for Malformed {}

/// The bot's session with the Gateway, across connections.
#[derive(Debug, Default)]
pub struct Session {
    /// The session a READY named, with where it is resumed.
    resumable: Option<(String, String)>,
    /// The last dispatch's sequence number.
    seq: Option<u32>,
    /// Whether a heartbeat was sent and not acknowledged yet.
    unanswered: bool,
}

fn carried<T: serde::de::DeserializeOwned>(what: &str, d: Value) -> Result<T, Malformed> {
    pcx_module_api::json::decode_value(d).map_err(|e| Malformed(format!("{what}: {e}")))
}

fn sent<T: serde::Serialize>(op: u8, d: &T) -> Result<Sent, Malformed> {
    serde_json::to_value(d)
        .map(|d| Sent { op, d })
        .map_err(|e| Malformed(e.to_string()))
}

impl Session {
    /// Where the session is resumed; none: there is none to resume, and
    /// the bot connects where the API says the Gateway is.
    #[must_use]
    pub fn resumed_at(&self) -> Option<&str> {
        self.resumable.as_ref().map(|(_, url)| url.as_str())
    }

    /// What `frame` asks next of the bot of `token`.
    ///
    /// # Errors
    /// A frame that is not the Gateway's.
    pub fn on(&mut self, token: &str, frame: Frame) -> Result<Next, Malformed> {
        match frame.op {
            op::HELLO => {
                let hello: Hello = carried("hello", frame.d)?;
                if hello.heartbeat_interval == 0 {
                    return Err(Malformed("a hello names no heartbeat interval".to_string()));
                }
                self.unanswered = false;
                let send = match (&self.resumable, self.seq) {
                    (Some((session_id, _)), Some(seq)) => sent(
                        op::RESUME,
                        &Resume {
                            token: token.to_string(),
                            session_id: session_id.clone(),
                            seq,
                        },
                    )?,
                    _ => sent(
                        op::IDENTIFY,
                        &Identify {
                            token: token.to_string(),
                            intents: INTENTS,
                            properties: Some(ConnectionProperties {
                                os: std::env::consts::OS.to_string(),
                                browser: LIBRARY.to_string(),
                                device: LIBRARY.to_string(),
                            }),
                        },
                    )?,
                };
                Ok(Next::Begin {
                    send,
                    every_ms: hello.heartbeat_interval,
                })
            }
            op::DISPATCH => {
                if frame.s.is_some() {
                    self.seq = frame.s;
                }
                let event = frame
                    .t
                    .ok_or_else(|| Malformed("a dispatch names no event".to_string()))?;
                match event.as_str() {
                    "READY" => {
                        let ready: Ready = carried("READY", frame.d)?;
                        self.resumable = Some((ready.session_id, ready.resume_gateway_url));
                        Ok(Next::Nothing)
                    }
                    "RESUMED" => Ok(Next::Nothing),
                    _ => Ok(Next::Event(event, frame.d)),
                }
            }
            op::HEARTBEAT => Ok(Next::Send(self.heartbeat())),
            op::HEARTBEAT_ACK => {
                self.unanswered = false;
                Ok(Next::Nothing)
            }
            op::RECONNECT => Ok(Next::Reconnect),
            op::INVALID_SESSION => {
                // `d` says whether the session may still be resumed.
                if frame.d != Value::Bool(true) {
                    self.resumable = None;
                    self.seq = None;
                }
                Ok(Next::Reconnect)
            }
            // An operation this version does not know asks nothing.
            _ => Ok(Next::Nothing),
        }
    }

    /// The heartbeat's time came: a heartbeat to send, or, when the last
    /// one was not acknowledged, a connection that is dead.
    pub fn beat(&mut self) -> Next {
        if self.unanswered {
            return Next::Reconnect;
        }
        self.unanswered = true;
        Next::Send(self.heartbeat())
    }

    fn heartbeat(&self) -> Sent {
        Sent {
            op: op::HEARTBEAT,
            d: self.seq.map_or(Value::Null, Value::from),
        }
    }
}

#[cfg(test)]
mod tests;
