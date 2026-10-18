// Copyright (c) 2026 Julius ML
// MIT License

//! `Host` header allowlist for the daemon's HTTP endpoints (DNS rebinding
//! protection, RUSTSEC-2026-0189).
//!
//! A web page can point an attacker-controlled DNS name at `127.0.0.1` and
//! then talk to a local daemon from the victim's browser. The browser still
//! sends the attacker's name in `Host`, so rejecting unknown `Host` values
//! closes the hole.
//!
//! The same [`HostPolicy`] feeds rmcp's built-in check on `/mcp` and the
//! [`require_allowed_host`] middleware on the REST `/api/*` routes.
//! `/health` stays open.
//!
//! Entries (`allowed_hosts` in `daemon.toml`, `PC_ALLOWED_HOSTS`):
//! - `host` matches that host on any port (`my-mac.tailnet.ts.net`,
//!   `100.64.0.7`, `::1`, `[fd7a::1]`);
//! - `host:port` matches only that port (`my-mac.tailnet.ts.net:3737`,
//!   `[fd7a::1]:3737`);
//! - `*` disables the check entirely (not recommended).
//!
//! Matching is case-insensitive. Configured entries are added to the
//! defaults: loopback names (`localhost`, `127.0.0.1`, `::1`) plus the bind
//! `host` when it is a specific (non-wildcard) address.

use crate::daemon::DaemonConfig;
use axum::{
    extract::{Request, State},
    http::{StatusCode, header, uri::Authority},
    middleware::Next,
    response::{IntoResponse, Response},
};
use rmcp::transport::streamable_http_server::StreamableHttpServerConfig;
use std::net::IpAddr;
use std::sync::Arc;

/// Hosts always accepted: a rebinding attack never sends these names.
pub const LOOPBACK_HOSTS: &[&str] = &["localhost", "127.0.0.1", "::1"];

/// Entry that turns validation off.
pub const ALLOW_ANY_HOST: &str = "*";

/// Paths reachable with any `Host`: liveness probes only.
const PUBLIC_PATHS: &[&str] = &["/health"];

/// Which `Host` header values the HTTP endpoints accept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostPolicy {
    /// `allowed_hosts` contains `*`: no validation.
    Any,
    /// Only these entries (`host` or `host:port`).
    Only(Vec<String>),
}

impl HostPolicy {
    /// Builds the policy from `host` and `allowed_hosts`.
    ///
    /// # Errors
    /// Returns a message naming the first malformed `allowed_hosts` entry.
    pub fn from_config(config: &DaemonConfig) -> Result<Self, String> {
        let mut entries: Vec<String> = LOOPBACK_HOSTS.iter().map(|h| (*h).to_string()).collect();
        if let Some(bind) = specific_bind_host(&config.host) {
            entries.push(bind);
        }
        for raw in &config.allowed_hosts {
            let entry = raw.trim();
            if entry == ALLOW_ANY_HOST {
                return Ok(Self::Any);
            }
            if parse_entry(entry).is_none() {
                return Err(format!(
                    "invalid allowed_hosts entry {raw:?}: expected host, host:port or \"*\""
                ));
            }
            entries.push(entry.to_string());
        }
        let mut seen = std::collections::HashSet::new();
        entries.retain(|e| seen.insert(e.to_ascii_lowercase()));
        Ok(Self::Only(entries))
    }

    /// Hands the policy to rmcp's Streamable HTTP transport.
    #[must_use]
    pub fn apply(&self, config: StreamableHttpServerConfig) -> StreamableHttpServerConfig {
        match self {
            // rmcp treats an empty list as "allow every host".
            Self::Any => config.disable_allowed_hosts(),
            Self::Only(entries) => config.with_allowed_hosts(entries.iter().cloned()),
        }
    }

    /// Whether a `Host` header value (`host[:port]`) is accepted, using
    /// rmcp's matching rules.
    #[must_use]
    pub fn allows(&self, host_header: &str) -> bool {
        let Self::Only(entries) = self else {
            return true;
        };
        let Ok(authority) = Authority::try_from(host_header.trim()) else {
            return false;
        };
        let host = normalize_host(authority.host());
        let port = authority.port_u16();
        entries
            .iter()
            .filter_map(|e| parse_entry(e))
            .any(|(allowed_host, allowed_port)| {
                allowed_host == host && allowed_port.is_none_or(|p| port == Some(p))
            })
    }
}

/// The bind host as an allowlist entry, unless it is a wildcard address.
fn specific_bind_host(host: &str) -> Option<String> {
    let trimmed = host.trim().trim_start_matches('[').trim_end_matches(']');
    match trimmed.parse::<IpAddr>() {
        Ok(ip) if ip.is_unspecified() => None,
        _ if trimmed.is_empty() => None,
        _ => Some(trimmed.to_string()),
    }
}

/// Parses an entry into a lower-cased host and optional port, the way rmcp
/// does: an authority when it parses as one (`host`, `host:port`,
/// `[v6]:port`), else the raw text as a host (bare IPv6 such as `::1`).
fn parse_entry(entry: &str) -> Option<(String, Option<u16>)> {
    let entry = entry.trim();
    if entry.is_empty() || entry.contains(['/', '@', ' ', '*']) {
        return None;
    }
    if let Ok(authority) = Authority::try_from(entry) {
        return Some((normalize_host(authority.host()), authority.port_u16()));
    }
    entry
        .parse::<std::net::Ipv6Addr>()
        .ok()
        .map(|_| (entry.to_ascii_lowercase(), None))
}

fn normalize_host(host: &str) -> String {
    host.trim_matches(['[', ']']).to_ascii_lowercase()
}

/// axum middleware: `403` for requests whose `Host` is not allowed. Mirrors
/// rmcp's response on `/mcp`.
pub async fn require_allowed_host(
    State(policy): State<Arc<HostPolicy>>,
    req: Request,
    next: Next,
) -> Response {
    if PUBLIC_PATHS.contains(&req.uri().path()) {
        return next.run(req).await;
    }
    // HTTP/2 carries the host in `:authority` rather than `Host`.
    let host = req
        .headers()
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned)
        .or_else(|| req.uri().authority().map(|a| a.as_str().to_owned()));
    match host {
        Some(host) if policy.allows(&host) => next.run(req).await,
        Some(host) => {
            tracing::warn!(
                host,
                "rejected request with disallowed Host header (possible DNS rebinding attempt)"
            );
            (
                StatusCode::FORBIDDEN,
                "Forbidden: Host header is not allowed",
            )
                .into_response()
        }
        None => (StatusCode::BAD_REQUEST, "Bad Request: missing Host header").into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy(host: &str, allowed: &[&str]) -> HostPolicy {
        HostPolicy::from_config(&DaemonConfig {
            host: host.to_string(),
            allowed_hosts: allowed.iter().map(|s| (*s).to_string()).collect(),
            ..Default::default()
        })
        .unwrap()
    }

    #[test]
    fn defaults_are_loopback_plus_specific_bind_host() {
        assert_eq!(
            policy("127.0.0.1", &[]),
            HostPolicy::Only(vec!["localhost".into(), "127.0.0.1".into(), "::1".into()])
        );
        assert_eq!(
            policy("100.64.0.7", &[]),
            HostPolicy::Only(vec![
                "localhost".into(),
                "127.0.0.1".into(),
                "::1".into(),
                "100.64.0.7".into()
            ])
        );
        for wildcard in ["0.0.0.0", "::", "[::]"] {
            assert_eq!(
                policy(wildcard, &[]),
                HostPolicy::Only(vec!["localhost".into(), "127.0.0.1".into(), "::1".into()]),
                "{wildcard}"
            );
        }
    }

    #[test]
    fn loopback_hosts_match_on_any_port() {
        let p = policy("0.0.0.0", &[]);
        for ok in [
            "localhost",
            "localhost:3737",
            "LOCALHOST:1",
            "127.0.0.1:3737",
            "[::1]:3737",
        ] {
            assert!(p.allows(ok), "{ok}");
        }
        for bad in [
            "evil.example",
            "evil.example:3737",
            "127.0.0.2",
            "localhost.evil.example",
        ] {
            assert!(!p.allows(bad), "{bad}");
        }
    }

    #[test]
    fn configured_entries_are_added_with_port_semantics() {
        let p = policy(
            "0.0.0.0",
            &["My-Mac.tailnet.ts.net", "100.64.0.7:3737", "[fd7a::1]:3737"],
        );
        assert!(p.allows("my-mac.tailnet.ts.net"));
        assert!(p.allows("my-mac.tailnet.ts.net:9999"));
        assert!(p.allows("100.64.0.7:3737"));
        assert!(!p.allows("100.64.0.7:80"));
        assert!(!p.allows("100.64.0.7"));
        assert!(p.allows("[fd7a::1]:3737"));
        assert!(!p.allows("[fd7a::1]:1"));
        assert!(p.allows("localhost:3737"), "defaults stay");
        assert!(!p.allows("evil.example"));
    }

    #[test]
    fn star_disables_validation() {
        let p = policy("0.0.0.0", &["example.com", "*"]);
        assert_eq!(p, HostPolicy::Any);
        assert!(p.allows("evil.example"));
    }

    #[test]
    fn malformed_entries_are_rejected() {
        for bad in [
            "",
            "http://example.com",
            "example.com/mcp",
            "user@host",
            "a b",
            "*.example.com",
        ] {
            let config = DaemonConfig {
                allowed_hosts: vec![bad.to_string()],
                ..Default::default()
            };
            assert!(HostPolicy::from_config(&config).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn apply_sets_rmcp_allowlist() {
        let rmcp_cfg = policy("127.0.0.1", &["pc.example:3737"]).apply(Default::default());
        assert_eq!(
            rmcp_cfg.allowed_hosts,
            vec!["localhost", "127.0.0.1", "::1", "pc.example:3737"]
        );
        let open = HostPolicy::Any.apply(Default::default());
        assert!(open.allowed_hosts.is_empty());
    }
}
