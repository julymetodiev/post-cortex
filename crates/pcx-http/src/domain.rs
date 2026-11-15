// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The http module's rules (docs/modules/http.md), pure: its settings read
//! and checked, a request planned under them (`request`), the addresses it
//! may reach (`address`), a redirect followed (`redirect`), a response read
//! and judged (`answer`), a preset checked (`preset`), the days of its log.

use std::fmt;
use std::time::Duration;

use pcx_module_api::ModuleErrorKind;
use pcx_module_api::name::is_slug;
use post_cortex_proto::http::{Auth as AuthKind, HttpConfig};
use url::Url;

pub mod address;
pub mod answer;
pub mod preset;
pub mod redirect;
pub mod request;

/// The module's id.
pub const MODULE_ID: &str = "http";

/// The kind of its log's documents in the module.
pub const CALL_KIND: &str = "call";

/// The kind of its presets' documents in the module.
pub const PRESET_KIND: &str = "preset";

/// Requests a day when the settings name none.
const DEFAULT_DAILY: u32 = 5_000;

/// Requests a day at most.
const MAX_DAILY: u32 = 100_000;

/// A request's time limit when neither it nor its connection names one.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// A request's time limit at most.
pub const MAX_TIMEOUT: Duration = Duration::from_secs(300);

/// Days the log keeps an entry.
pub const KEEP_DAYS: u64 = 30;

/// Entries of the log a page has when the request names no limit.
pub const LOG_PAGE: usize = 50;

/// Entries of the log a page has at most.
pub const MAX_LOG_PAGE: usize = 200;

/// Requests of the organisation in flight at once.
pub const IN_FLIGHT: usize = 16;

/// Its log's kind as memory keeps it, `http/call`.
#[must_use]
pub fn call_kind() -> String {
    format!("{MODULE_ID}/{CALL_KIND}")
}

/// Its presets' kind as memory keeps it, `http/preset`.
#[must_use]
pub fn preset_kind() -> String {
    format!("{MODULE_ID}/{PRESET_KIND}")
}

/// A rule a setting, a request or a preset breaks: how the call fails,
/// the field and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Broken {
    /// How a call that breaks it fails: `InvalidArgument`, or
    /// `PermissionDenied` for a host or an address it may not reach.
    pub kind: ModuleErrorKind,
    /// The field at fault, as the wire names it (`connections[1].base_url`,
    /// `path`).
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
    /// `field` is not valid: `reason`.
    pub fn new(field: &str, reason: impl Into<String>) -> Self {
        Self {
            kind: ModuleErrorKind::InvalidArgument,
            field: field.to_string(),
            reason: reason.into(),
        }
    }

    /// `field` names what the module may not reach: `reason`.
    pub fn denied(field: &str, reason: impl Into<String>) -> Self {
        Self {
            kind: ModuleErrorKind::Denied,
            ..Self::new(field, reason)
        }
    }
}

/// How a connection signs in, with its secret.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Auth {
    /// It does not.
    None,
    /// `Authorization: Bearer <token>`.
    Bearer(String),
    /// `Authorization: Basic` of a user and a password.
    Basic {
        /// The user.
        user: String,
        /// The password.
        password: String,
    },
    /// The secret in a header.
    Header {
        /// The header's name, in lower case.
        name: String,
        /// The secret.
        value: String,
    },
    /// The secret in a query parameter.
    Query {
        /// The parameter's name.
        name: String,
        /// The secret.
        value: String,
    },
}

/// A connection, read and checked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Connection {
    /// Its name.
    pub name: String,
    /// What it is.
    pub description: String,
    /// Its base URL, its path without a trailing `/`.
    pub base: Url,
    /// How it signs in.
    pub auth: Auth,
    /// The headers it sends, names in lower case.
    pub headers: Vec<(String, String)>,
    /// The header of a workflow's idempotency key, in lower case; none:
    /// none is sent.
    pub idempotency_header: Option<String>,
    /// A request's time limit.
    pub timeout: Duration,
    /// What its test calls.
    pub test_path: String,
}

impl Connection {
    /// Its secret, when it has one.
    #[must_use]
    pub fn secret(&self) -> Option<&str> {
        match &self.auth {
            Auth::None => None,
            Auth::Bearer(token) => Some(token),
            Auth::Basic { password, .. } => Some(password),
            Auth::Header { value, .. } | Auth::Query { value, .. } => Some(value),
        }
    }
}

/// The module's settings, read and checked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    /// The workspace its presets and log are kept in.
    pub workspace: String,
    /// Its connections, by name.
    pub connections: Vec<Connection>,
    /// The hosts a request without a connection may reach (`host` or
    /// `*.domain`), in lower case.
    pub allow: Vec<String>,
    /// Whether private addresses may be reached.
    pub private_networks: bool,
    /// Requests a day.
    pub daily_limit: u32,
}

impl Settings {
    /// Connection `name`.
    ///
    /// # Errors
    /// There is none of that name.
    pub fn connection(&self, name: &str) -> Result<&Connection, Broken> {
        self.connections
            .iter()
            .find(|c| c.name == name)
            .ok_or_else(|| {
                Broken::new(
                    "connection",
                    format!("{name:?} is no connection of the settings"),
                )
            })
    }

    /// Whether `allow` names `host` (`*.domain` names its subdomains, not
    /// the domain).
    #[must_use]
    pub fn allows(&self, host: &str) -> bool {
        let host = host.to_ascii_lowercase();
        self.allow.iter().any(|a| match a.strip_prefix("*.") {
            Some(domain) => host
                .strip_suffix(domain)
                .is_some_and(|rest| rest.ends_with('.') && rest.len() > 1),
            None => *a == host,
        })
    }
}

/// `config` read and checked; `public_only` (the cloud's) refuses
/// private networks.
///
/// # Errors
/// The first setting at fault and why.
pub fn settings(config: HttpConfig, public_only: bool) -> Result<Settings, Broken> {
    if public_only && config.private_networks {
        return Err(Broken::new(
            "private_networks",
            "is refused here: this host reaches public addresses only",
        ));
    }
    let daily_limit = match config.daily_limit {
        0 => DEFAULT_DAILY,
        n if n > MAX_DAILY => {
            return Err(Broken::new(
                "daily_limit",
                format!("is at most {MAX_DAILY}"),
            ));
        }
        n => n,
    };
    let mut connections: Vec<Connection> = Vec::with_capacity(config.connections.len());
    for (i, c) in config.connections.into_iter().enumerate() {
        let read = connection(c, config.private_networks).map_err(|b| Broken {
            field: format!("connections[{i}].{}", b.field),
            ..b
        })?;
        if connections.iter().any(|c| c.name == read.name) {
            return Err(Broken::new(
                &format!("connections[{i}].name"),
                format!("{} names two connections", read.name),
            ));
        }
        connections.push(read);
    }
    let mut allow = Vec::with_capacity(config.allow.len());
    for (i, host) in config.allow.iter().enumerate() {
        let host = host.trim().to_ascii_lowercase();
        let bare = host.strip_prefix("*.").unwrap_or(&host);
        if bare.is_empty() || !bare.contains('.') || bare.contains(['/', ':', '*', '@', ' ']) {
            return Err(Broken::new(
                &format!("allow[{i}]"),
                "is a host or *.domain, without a scheme or a port",
            ));
        }
        allow.push(host);
    }
    Ok(Settings {
        workspace: config.workspace_id,
        connections,
        allow,
        private_networks: config.private_networks,
        daily_limit,
    })
}

fn connection(
    c: post_cortex_proto::http::Connection,
    private_networks: bool,
) -> Result<Connection, Broken> {
    if !is_slug(&c.name) {
        return Err(Broken::new("name", "is 1 to 64 of a-z, 0-9 and '-'"));
    }
    let base =
        base_url(&c.base_url, private_networks).map_err(|why| Broken::new("base_url", why))?;
    let secret = || {
        if c.secret.is_empty() {
            Err(Broken::new("secret", "is required by its authentication"))
        } else {
            Ok(c.secret.clone())
        }
    };
    // A header's name in lower case; a query parameter's as given.
    let named = |lower: bool| match c.auth_name.trim() {
        "" => Err(Broken::new(
            "auth_name",
            "is required by its authentication",
        )),
        name if lower => Ok(name.to_ascii_lowercase()),
        name => Ok(name.to_string()),
    };
    let auth = match AuthKind::try_from(c.auth) {
        Ok(AuthKind::Unspecified) => Auth::None,
        Ok(AuthKind::Bearer) => Auth::Bearer(secret()?),
        Ok(AuthKind::Basic) => Auth::Basic {
            user: c.user.clone(),
            password: secret()?,
        },
        Ok(AuthKind::Header) => {
            let name = named(true)?;
            request::headers::header_name(&name).map_err(|why| Broken::new("auth_name", why))?;
            if request::headers::reserved(&name) {
                return Err(Broken::new(
                    "auth_name",
                    format!("{name} is set by the module"),
                ));
            }
            Auth::Header {
                name,
                value: secret()?,
            }
        }
        Ok(AuthKind::Query) => Auth::Query {
            name: named(false)?,
            value: secret()?,
        },
        Err(_) => {
            return Err(Broken::new(
                "auth",
                format!("{} is no authentication", c.auth),
            ));
        }
    };
    let mut headers = Vec::with_capacity(c.headers.len());
    for (i, h) in c.headers.iter().enumerate() {
        let name = h.name.trim().to_ascii_lowercase();
        request::headers::header(&name, &h.value)
            .map_err(|why| Broken::new(&format!("headers[{i}]"), why))?;
        headers.push((name, h.value.clone()));
    }
    let idempotency_header = match c.idempotency_header.trim() {
        "" => None,
        name => {
            let name = name.to_ascii_lowercase();
            request::headers::header_name(&name)
                .map_err(|why| Broken::new("idempotency_header", why))?;
            Some(name)
        }
    };
    let timeout = match c.timeout_seconds {
        0 => DEFAULT_TIMEOUT,
        s if u64::from(s) > MAX_TIMEOUT.as_secs() => {
            return Err(Broken::new(
                "timeout_seconds",
                format!("is at most {}", MAX_TIMEOUT.as_secs()),
            ));
        }
        s => Duration::from_secs(s.into()),
    };
    let test_path = match c.test_path.as_str() {
        "" => "/".to_string(),
        p if p.starts_with('/') => p.to_string(),
        _ => return Err(Broken::new("test_path", "begins with /")),
    };
    Ok(Connection {
        name: c.name,
        description: c.description,
        base,
        auth,
        headers,
        idempotency_header,
        timeout,
        test_path,
    })
}

/// `text` as a connection's base URL: `https://` (or `http://` to a
/// loopback host with private networks) with a host, no user, query or
/// fragment, its path without a trailing `/`.
///
/// # Errors
/// Why it is not one.
pub fn base_url(text: &str, private_networks: bool) -> Result<Url, String> {
    let mut url = Url::parse(text).map_err(|e| format!("is no URL: {e}"))?;
    secure(&url, private_networks)?;
    if !url.username().is_empty() || url.password().is_some() {
        return Err("carries no user or password: the secret does".to_string());
    }
    if url.query().is_some() || url.fragment().is_some() {
        return Err("has no query or fragment".to_string());
    }
    let path = url.path().trim_end_matches('/').to_string();
    url.set_path(&path);
    Ok(url)
}

/// Whether `url` may be reached at all: `https` with a host, or `http` to
/// a loopback host with private networks.
///
/// # Errors
/// Why it may not.
pub fn secure(url: &Url, private_networks: bool) -> Result<(), String> {
    let Some(host) = url.host() else {
        return Err("has no host".to_string());
    };
    match url.scheme() {
        "https" => Ok(()),
        "http" if private_networks && address::loopback_host(&host) => Ok(()),
        "http" => Err("is https (http only to a loopback host, with private networks)".to_string()),
        other => Err(format!("is https, not {other}")),
    }
}

#[cfg(test)]
mod tests;
