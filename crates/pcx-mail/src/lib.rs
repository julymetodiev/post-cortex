// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The `mail` module (docs/modules/mail.md): mail with the organisation's
//! own credentials, through SMTP, Resend or Postmark, each send kept in a
//! log in memory. The domain reads the settings and checks a message, its
//! recipients and the daily limit; the service sends through the
//! `Transport` port and keeps the log; the handlers only map. Its transports
//! and templates are a library too, for the cloud's own mail.

#![forbid(unsafe_code)]

mod adapters;
mod domain;
mod handlers;
mod module;
mod ports;
mod service;
mod specs;

pub use module::MailModule;

// The library the cloud's control plane sends its own mail with
// (docs/modules/mail.md, the cloud's own mail): the transports, a template
// checked and filled, a message checked.
pub use adapters::transport;
pub use domain::template::{Checked, check as check_template};
pub use domain::{Broken, Route, Secured, address as check_address, check as check_message};
pub use ports::{Outgoing, Transport};
