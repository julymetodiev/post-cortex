// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The mail module's rules (docs/modules/mail.md), pure: its settings read
//! and checked, an address, whom `allow` lets it mail, a message's limits,
//! the day the daily limit counts and how long the log keeps an entry.

use std::fmt;
use std::net::IpAddr;

use post_cortex_proto::mail::{MailConfig, Mailbox, Security, SendRequest, Transport};

/// The module's id.
pub const MODULE_ID: &str = "mail";

/// The kind of its log's documents in the module.
pub const SENT_KIND: &str = "sent";

/// Recipients of one message at most, to, cc and bcc together.
const MAX_RECIPIENTS: usize = 50;

/// Characters of a subject at most: an SMTP line.
const MAX_SUBJECT: usize = 998;

/// Bytes of a body at most.
const MAX_BODY: usize = 1 << 20;

/// Messages a day when the settings name none.
const DEFAULT_DAILY: u32 = 200;

/// Messages a day at most.
const MAX_DAILY: u32 = 10_000;

/// Days the log keeps an entry.
pub const KEEP_DAYS: u64 = 90;

/// Characters of an address at most (RFC 5321).
const MAX_ADDRESS: usize = 254;

/// Entries of the log a page has when the request names no limit.
pub const LOG_PAGE: usize = 50;

/// Entries of the log a page has at most.
pub const MAX_LOG_PAGE: usize = 200;

/// Its log's kind as memory keeps it, `mail/sent`.
#[must_use]
pub fn sent_kind() -> String {
    format!("{MODULE_ID}/{SENT_KIND}")
}

/// A rule a setting or a message breaks: the field and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Broken {
    /// The field at fault, as the wire names it (`smtp.host`, `to`).
    pub field: String,
    /// What is wrong.
    pub reason: String,
}

impl fmt::Display for Broken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}", self.field, self.reason)
    }
}

impl Broken {
    /// `field` breaks a rule: `reason`.
    pub fn new(field: &str, reason: impl Into<String>) -> Self {
        Self {
            field: field.to_string(),
            reason: reason.into(),
        }
    }
}

/// How an SMTP connection is secured.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Secured {
    /// STARTTLS, which must succeed.
    StartTls,
    /// TLS from the start.
    Tls,
    /// None (a loopback host only).
    Plain,
}

/// How the module sends, with its credentials.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Route {
    /// An SMTP relay.
    Smtp {
        /// Its host.
        host: String,
        /// Its port.
        port: u16,
        /// How the connection is secured.
        secured: Secured,
        /// The user and password it signs in with; none: no sign-in.
        credentials: Option<(String, String)>,
    },
    /// Resend's API.
    Resend {
        /// Its URL, without a trailing `/`.
        url: String,
        /// The API key.
        key: String,
    },
    /// Postmark's API.
    Postmark {
        /// Its URL, without a trailing `/`.
        url: String,
        /// The server's token.
        token: String,
        /// The message stream.
        stream: String,
    },
}

impl Route {
    /// Its name for a person.
    #[must_use]
    pub fn name(&self) -> &'static str {
        match self {
            Self::Smtp { .. } => "SMTP",
            Self::Resend { .. } => "Resend",
            Self::Postmark { .. } => "Postmark",
        }
    }
}

/// The settings, read and checked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    /// The workspace its log is kept in.
    pub workspace: String,
    /// Who the mail is from.
    pub from: Mailbox,
    /// Where answers go; none: to `from`.
    pub reply_to: Option<String>,
    /// How it sends.
    pub route: Route,
    /// The recipients it may send to, lowercase; empty: any.
    pub allow: Vec<String>,
    /// Messages a day.
    pub daily_limit: u32,
}

/// The settings of `config`.
///
/// # Errors
/// The first rule a setting breaks: no workspace, an address that is not
/// one, a name or address with a line break, no transport or none of its
/// settings, a plain SMTP connection to a host that is not the loopback,
/// an entry of `allow` that is no address or `@domain`, a daily limit over
/// 10,000.
pub fn settings(config: MailConfig) -> Result<Settings, Broken> {
    if config.workspace_id.trim().is_empty() {
        return Err(Broken::new(
            "workspace_id",
            "is required: where the log is kept",
        ));
    }
    let transport = config.transport();
    let from = config
        .from
        .ok_or_else(|| Broken::new("from", "is required"))?;
    address("from.address", &from.address)?;
    one_line("from.name", &from.name)?;
    let reply_to = Some(config.reply_to).filter(|r| !r.is_empty());
    if let Some(reply_to) = &reply_to {
        address("reply_to", reply_to)?;
    }
    let route = match transport {
        Transport::Unspecified => return Err(Broken::new("transport", "is required")),
        Transport::Smtp => smtp(config.smtp)?,
        Transport::Resend => {
            let resend = config
                .resend
                .ok_or_else(|| Broken::new("resend", "is required with transport Resend"))?;
            Route::Resend {
                url: url(&resend.api_url, "https://api.resend.com"),
                key: secret("resend.api_key", resend.api_key)?,
            }
        }
        Transport::Postmark => {
            let postmark = config
                .postmark
                .ok_or_else(|| Broken::new("postmark", "is required with transport Postmark"))?;
            Route::Postmark {
                url: url(&postmark.api_url, "https://api.postmarkapp.com"),
                token: secret("postmark.server_token", postmark.server_token)?,
                stream: Some(postmark.message_stream)
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| "outbound".to_string()),
            }
        }
    };
    let mut allow = Vec::with_capacity(config.allow.len());
    for entry in &config.allow {
        let domain = entry.strip_prefix('@');
        let well_formed = match domain {
            Some(domain) => domain_name(domain),
            None => address("allow", entry).is_ok(),
        };
        if !well_formed {
            return Err(Broken::new(
                "allow",
                format!("has {entry:?}, neither an address nor @domain"),
            ));
        }
        allow.push(entry.to_ascii_lowercase());
    }
    let daily_limit = match config.daily_limit {
        0 => DEFAULT_DAILY,
        n if n > MAX_DAILY => {
            return Err(Broken::new(
                "daily_limit",
                format!("is at most {MAX_DAILY} a day"),
            ));
        }
        n => n,
    };
    Ok(Settings {
        workspace: config.workspace_id,
        from,
        reply_to,
        route,
        allow,
        daily_limit,
    })
}

fn smtp(smtp: Option<post_cortex_proto::mail::Smtp>) -> Result<Route, Broken> {
    let smtp = smtp.ok_or_else(|| Broken::new("smtp", "is required with transport SMTP"))?;
    let host = smtp.host.trim().to_string();
    if host.is_empty() {
        return Err(Broken::new("smtp.host", "is required"));
    }
    let secured = match smtp.security() {
        Security::Unspecified | Security::Starttls => Secured::StartTls,
        Security::Tls => Secured::Tls,
        Security::None => Secured::Plain,
    };
    if secured == Secured::Plain && !loopback(&host) {
        return Err(Broken::new(
            "smtp.security",
            "none is allowed only to a loopback host",
        ));
    }
    let port = match (smtp.port, secured) {
        (0, Secured::StartTls) => 587,
        (0, Secured::Tls) => 465,
        (0, Secured::Plain) => 25,
        (port, _) => {
            u16::try_from(port).map_err(|_| Broken::new("smtp.port", "is at most 65535"))?
        }
    };
    let credentials = match (smtp.user.is_empty(), smtp.password.is_empty()) {
        (true, true) => None,
        (false, false) => Some((smtp.user, smtp.password)),
        (true, false) => return Err(Broken::new("smtp.user", "is required with a password")),
        (false, true) => return Err(Broken::new("smtp.password", "is required with a user")),
    };
    Ok(Route::Smtp {
        host,
        port,
        secured,
        credentials,
    })
}

fn url(given: &str, default: &str) -> String {
    let given = given.trim().trim_end_matches('/');
    if given.is_empty() {
        default.to_string()
    } else {
        given.to_string()
    }
}

fn secret(field: &str, value: String) -> Result<String, Broken> {
    if value.trim().is_empty() {
        return Err(Broken::new(field, "is required"));
    }
    Ok(value)
}

fn loopback(host: &str) -> bool {
    host.eq_ignore_ascii_case("localhost")
        || host
            .trim_start_matches('[')
            .trim_end_matches(']')
            .parse::<IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

/// Whether `domain` reads as a domain name: dot-separated labels of
/// letters, digits and `-`, at least two.
fn domain_name(domain: &str) -> bool {
    let labels: Vec<&str> = domain.split('.').collect();
    labels.len() >= 2
        && labels.iter().all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
}

/// Check that `value` is an address: a local part and a domain name,
/// with no space, line break, angle bracket, comma or quote.
///
/// # Errors
/// It is not one.
pub fn address(field: &str, value: &str) -> Result<(), Broken> {
    let refused = || Broken::new(field, format!("has {value:?}, which is no address"));
    if value.len() > MAX_ADDRESS {
        return Err(refused());
    }
    let (local, domain) = value.rsplit_once('@').ok_or_else(refused)?;
    let local_ok = !local.is_empty()
        && local.len() <= 64
        && local
            .chars()
            .all(|c| !c.is_whitespace() && !c.is_control() && !"<>,;:\"()[]\\@".contains(c));
    if !local_ok || !domain_name(domain) {
        return Err(refused());
    }
    Ok(())
}

fn one_line(field: &str, value: &str) -> Result<(), Broken> {
    if value.contains(['\r', '\n']) {
        return Err(Broken::new(field, "has a line break"));
    }
    Ok(())
}

/// Whether `allow` (lowercase entries, empty: any) lets the module mail
/// `address`.
#[must_use]
pub fn allowed(allow: &[String], address: &str) -> bool {
    if allow.is_empty() {
        return true;
    }
    let address = address.to_ascii_lowercase();
    let domain = address.rsplit_once('@').map(|(_, d)| d);
    allow.iter().any(|entry| match entry.strip_prefix('@') {
        Some(allowed) => domain == Some(allowed),
        None => *entry == address,
    })
}

/// Every recipient of a message or a log entry, `to`, `cc` then `bcc`.
pub fn recipients<'a>(
    to: &'a [String],
    cc: &'a [String],
    bcc: &'a [String],
) -> impl Iterator<Item = &'a String> {
    to.iter().chain(cc).chain(bcc)
}

/// Check `message`: at least one recipient in `to`, at most 50 in all,
/// each an address; a subject of one line of at most 998 characters; a
/// text body, and each body at most 1 MiB.
///
/// # Errors
/// The first rule it breaks.
pub fn check(message: &SendRequest) -> Result<(), Broken> {
    if message.to.is_empty() {
        return Err(Broken::new("to", "needs at least one recipient"));
    }
    if recipients(&message.to, &message.cc, &message.bcc).count() > MAX_RECIPIENTS {
        return Err(Broken::new(
            "to",
            format!("and cc and bcc have at most {MAX_RECIPIENTS} recipients"),
        ));
    }
    for (field, list) in [
        ("to", &message.to),
        ("cc", &message.cc),
        ("bcc", &message.bcc),
    ] {
        for recipient in list {
            address(field, recipient)?;
        }
    }
    if message.subject.trim().is_empty() {
        return Err(Broken::new("subject", "is required"));
    }
    one_line("subject", &message.subject)?;
    if message.subject.chars().count() > MAX_SUBJECT {
        return Err(Broken::new(
            "subject",
            format!("is at most {MAX_SUBJECT} characters"),
        ));
    }
    if message.text.trim().is_empty() {
        return Err(Broken::new(
            "text",
            "is required: every message has a text body",
        ));
    }
    for (field, body) in [("text", &message.text), ("html", &message.html)] {
        if body.len() > MAX_BODY {
            return Err(Broken::new(field, "is at most 1 MiB"));
        }
    }
    Ok(())
}

/// `mailbox` as a header writes it: `"Name" <address>`, or the address
/// when it has no name.
#[must_use]
pub fn mailbox_text(mailbox: &Mailbox) -> String {
    if mailbox.name.is_empty() {
        return mailbox.address.clone();
    }
    let name = mailbox.name.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{name}\" <{}>", mailbox.address)
}

pub mod template;

#[cfg(test)]
mod tests;
