// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Nomad's HTTP API as pcx's modules call it (ARCHITECTURE section 8,
//! operations): `observe` reads it and `ops` changes it, each over
//! `pcx-kube`'s calls, since Nomad takes its ACL token as a bearer's and
//! its TLS is a CA file and a server name. What is Nomad's own is here:
//! the names that may go into its paths, the paths and what an
//! environment's settings must be (its JSON, whose nulls are fields left
//! out, is read by `pcx_module_api::json::decode`).

#![forbid(unsafe_code)]

/// Characters of a job's or a task's name.
const NAME_CHARS: usize = 128;

/// Characters of an allocation's id, and the fewest that may stand for it.
const ID_CHARS: (usize, usize) = (8, 36);

/// Whether `name` may name a job or a task in a path: letters, digits,
/// '-', '_' and '.', starting with a letter or a digit. A launch of a
/// periodic job (`job/periodic-1`) is none: it is read from the lists.
#[must_use]
pub fn name(name: &str) -> bool {
    name.len() <= NAME_CHARS
        && name.starts_with(|c: char| c.is_ascii_alphanumeric())
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
}

/// Whether `id` may be an allocation's id or the start of one: 8 to 36
/// hexadecimal digits and '-'.
#[must_use]
pub fn allocation(id: &str) -> bool {
    (ID_CHARS.0..=ID_CHARS.1).contains(&id.len())
        && id.chars().all(|c| c.is_ascii_hexdigit() || c == '-')
}

/// What Nomad is asked for the allocations whose id starts with `id`: it
/// looks an id up by an even number of its digits, so an odd one loses its
/// last (the answer then holds every allocation that starts so, of which
/// those that start with `id` are the ones asked).
#[must_use]
pub fn prefix(id: &str) -> &str {
    let id = id.trim_end_matches('-');
    let digits = id.chars().filter(char::is_ascii_hexdigit).count();
    if digits % 2 == 0 {
        id
    } else {
        id.strip_suffix(|c: char| c.is_ascii_hexdigit())
            .unwrap_or(id)
            .trim_end_matches('-')
    }
}

/// Whether an environment's Nomad settings may run: an https URL, its
/// token's file, and a TLS server name that has its CA file.
///
/// # Errors
/// The setting that is wrong, and why.
pub fn settings(
    api: &str,
    token_file: &str,
    ca_file: &str,
    tls_server_name: &str,
) -> Result<(), (&'static str, &'static str)> {
    if !api.trim().starts_with("https://") {
        return Err(("nomad_api", "is no https URL"));
    }
    if token_file.trim().is_empty() {
        return Err(("nomad_token_file", "Nomad needs its token's file"));
    }
    pcx_kube::tls_server_name(tls_server_name, ca_file)
        .map_err(|reason| ("nomad_tls_server_name", reason))
}

/// The API's paths; each name in one is checked before ([`name`],
/// [`allocation`]).
pub mod paths {
    /// The jobs.
    pub const JOBS: &str = "/v1/jobs";

    /// The allocations; with the query `prefix`, those whose id starts so.
    pub const ALLOCATIONS: &str = "/v1/allocations";

    /// The nodes.
    pub const NODES: &str = "/v1/nodes";

    /// A node's host statistics; the query `node_id` names the node.
    pub const STATS: &str = "/v1/client/stats";

    /// A job.
    #[must_use]
    pub fn job(name: &str) -> String {
        format!("/v1/job/{name}")
    }

    /// The versions of a job Nomad keeps.
    #[must_use]
    pub fn versions(name: &str) -> String {
        format!("{}/versions", job(name))
    }

    /// A job's latest deployment.
    #[must_use]
    pub fn deployment(name: &str) -> String {
        format!("{}/deployment", job(name))
    }

    /// A job's summary.
    #[must_use]
    pub fn summary(name: &str) -> String {
        format!("{}/summary", job(name))
    }

    /// Where a job is reverted (POST).
    #[must_use]
    pub fn revert(name: &str) -> String {
        format!("{}/revert", job(name))
    }

    /// Where a periodic job is launched now (POST).
    #[must_use]
    pub fn launch(name: &str) -> String {
        format!("{}/periodic/force", job(name))
    }

    /// Where an allocation is stopped (POST).
    #[must_use]
    pub fn stop(allocation: &str) -> String {
        format!("/v1/allocation/{allocation}/stop")
    }

    /// A task's log in an allocation; the queries `task`, `type`,
    /// `origin`, `offset` and `plain` say which and how much.
    #[must_use]
    pub fn log(allocation: &str) -> String {
        format!("/v1/client/fs/logs/{allocation}")
    }
}

#[cfg(test)]
mod tests;
