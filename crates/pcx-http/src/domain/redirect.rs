// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! A redirect followed (docs/modules/http.md, the network rules): at most
//! five, each one's URL checked again; a connection's credentials go only
//! to its own origin, and another host must be one `allow` names; a 303,
//! or a 301 or 302 of a POST, goes on as a GET without a body.

use url::Url;

use crate::domain::request::{Method, Planned};
use crate::domain::{Broken, Settings, secure};

/// Redirects a call follows at most.
pub const MAX_REDIRECTS: u32 = 5;

/// Whether `status` redirects.
#[must_use]
pub fn redirects(status: u16) -> bool {
    matches!(status, 301 | 302 | 303 | 307 | 308)
}

/// The request `planned` goes on as after `status` sent it to
/// `location`.
///
/// # Errors
/// `PermissionDenied` for a URL the module may not reach (not https, or a
/// host `allow` does not name outside the connection's origin);
/// `InvalidArgument` for a location that is no URL.
pub fn follow(
    planned: &Planned,
    status: u16,
    location: &str,
    settings: &Settings,
) -> Result<Planned, Broken> {
    let mut url: Url = planned
        .url
        .join(location)
        .map_err(|e| Broken::new("location", format!("{location:?} is no URL: {e}")))?;
    secure(&url, settings.private_networks)
        .map_err(|why| Broken::denied("location", format!("{url}: {why}")))?;
    if !url.username().is_empty() || url.password().is_some() {
        return Err(Broken::denied("location", "carries a user or a password"));
    }
    url.set_fragment(None);
    let mut next = planned.clone();
    let home = planned
        .credentials
        .as_ref()
        .is_some_and(|c| c.origin == url.origin());
    if !home {
        let Some(host) = url.host_str() else {
            return Err(Broken::denied("location", "has no host"));
        };
        if !settings.allows(host) {
            return Err(Broken::denied(
                "location",
                format!(
                    "{host} is outside the connection and not among the hosts the settings allow"
                ),
            ));
        }
        if let Some(held) = next.credentials.take() {
            next.headers
                .retain(|(name, _)| !held.headers.contains(name));
            if let Some(param) = held.query {
                let kept: Vec<(String, String)> = url
                    .query_pairs()
                    .filter(|(name, _)| *name != param)
                    .map(|(n, v)| (n.into_owned(), v.into_owned()))
                    .collect();
                url.set_query(None);
                if !kept.is_empty() {
                    url.query_pairs_mut().extend_pairs(kept);
                }
            }
        }
    }
    let becomes_get = (status == 303 && planned.method != Method::Head)
        || (matches!(status, 301 | 302) && planned.method == Method::Post);
    if becomes_get {
        next.method = Method::Get;
        next.body = None;
    }
    next.url = url;
    Ok(next)
}

#[cfg(test)]
mod tests;
