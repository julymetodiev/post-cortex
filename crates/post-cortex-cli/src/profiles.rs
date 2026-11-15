// Copyright (c) 2026 Julius ML
// MIT License

//! Backend profiles: which post-cortex the CLI, the stdio proxy, the hooks
//! and `pcx setup` talk to. These files are the client-side config shared
//! with the native plugins (TypeScript, Python) that call the REST API
//! without pcx, so the rules below are the contract they implement too.
//! `daemon.toml` stays the daemon's own config.
//!
//! # Files
//!
//! The directory is `$PC_CONFIG_DIR` when set and not empty, else
//! `~/.post-cortex`.
//!
//! `config.json`:
//!
//! ```json
//! {
//!   "active": "cloud",
//!   "profiles": {
//!     "cloud": { "url": "https://api.pcx.is" },
//!     "self": { "url": "http://127.0.0.1:3737" }
//!   },
//!   "hosts": { "<client author name>": {} }
//! }
//! ```
//!
//! Every key is optional. `cloud` (URL [`CLOUD_URL`]) and `self` (the daemon
//! in `daemon.toml`, `http://<host>:<port>`, a wildcard host read as
//! `127.0.0.1`) always exist; a `profiles` entry for either overrides its
//! URL. `hosts` holds per-client overrides, keyed by the client's author
//! name: `{"enabled": bool, "capture": bool, "workspace": "<name>"}`
//! (enabled by default, capture off by default, workspace: the workspace
//! name a new project is resolved into, see `project.rs`). Keys this
//! module does not know (`hosts` included) are kept as they are when it
//! saves the file. URLs are stored without a trailing slash.
//!
//! `credentials.json`, mode 0600 (created so; a looser mode gets a warning):
//!
//! ```json
//! { "cloud": { "token": "..." } }
//! ```
//!
//! # Resolution
//!
//! - Profile: the one asked for (`--profile`), else `$PC_PROFILE`, else
//!   `active`, else `cloud`.
//! - URL: `$PC_URL` when set (wins over the profile), else the profile's
//!   `url`, else the built-in default (`cloud`, `self`). An unknown profile
//!   without `$PC_URL` is an error.
//! - Token: `$PC_AUTH_TOKEN`, else `credentials.json` for the profile, else,
//!   for `self` only, `auth_token` in `daemon.toml`. Empty or blank values
//!   count as unset everywhere.
//!
//! Tokens are never written into a client's config: remote clients run
//! `pcx mcp --profile`, which resolves the token itself. No keychain: it
//! must work on a headless Linux box.
//!
//! Every function works on a [`Store`] rooted at a directory with the
//! environment passed in, so tests use a temp dir and never touch the real
//! `~/.post-cortex`.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

mod commands;
mod store;

pub use commands::{handle_profile_action, login, login_to, logout};
pub use store::Store;

/// The hosted post-cortex service.
pub const CLOUD_URL: &str = "https://api.pcx.is";
/// Where to create an API key for the hosted service (the app: Connections, API keys).
pub const KEYS_URL: &str = "https://app.pcx.is";
/// The hosted service's profile, active by default.
pub const CLOUD: &str = "cloud";
/// The self-hosted daemon's profile (URL from `daemon.toml`).
pub const SELF_HOSTED: &str = "self";

const CONFIG_FILE: &str = "config.json";
const CREDENTIALS_FILE: &str = "credentials.json";

/// A resolved backend: where it is and the bearer token to send, if any.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Profile {
    pub name: String,
    /// Base URL without a trailing slash (`https://api.pcx.is`).
    pub url: String,
    pub token: Option<String>,
}

impl Profile {
    /// Whether clients reach this backend through `pcx mcp --profile`
    /// (every profile but `self`, whose clients talk to the daemon over
    /// HTTP directly, as before profiles).
    pub fn remote(&self) -> bool {
        self.name != SELF_HOSTED
    }

    /// The backend's MCP endpoint.
    pub fn mcp_url(&self) -> String {
        format!("{}/mcp", self.url)
    }
}

/// `config.json` as written on disk: only what was set, plus every key this
/// module does not know (kept on save).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ConfigFile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub profiles: BTreeMap<String, ProfileEntry>,
    #[serde(flatten)]
    pub other: Map<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProfileEntry {
    pub url: String,
    #[serde(flatten)]
    pub other: Map<String, Value>,
}

/// `hosts.<author>` of `config.json`: per-client overrides.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostSettings {
    /// The client's global hooks run (default true).
    pub enabled: bool,
    /// Conversation capture for clients whose hooks ask (default false).
    pub capture: bool,
    /// Workspace name a new project key is resolved into.
    pub workspace: Option<String>,
}

/// The environment the resolution reads (`PC_PROFILE`, `PC_URL`,
/// `PC_AUTH_TOKEN`); blank values are `None`.
#[derive(Debug, Clone, Default)]
pub struct Env {
    pub profile: Option<String>,
    pub url: Option<String>,
    pub token: Option<String>,
}

impl Env {
    /// The process environment.
    ///
    /// # Errors
    /// A variable that is set but not valid UTF-8.
    pub fn from_process() -> Result<Self, String> {
        use crate::local::env_text;
        Ok(Self {
            profile: env_text("PC_PROFILE")?,
            url: env_text("PC_URL")?,
            token: env_text("PC_AUTH_TOKEN")?,
        }
        .cleaned())
    }

    fn cleaned(self) -> Self {
        Self {
            profile: clean(self.profile),
            url: clean(self.url),
            token: clean(self.token),
        }
    }
}

fn clean(value: Option<String>) -> Option<String> {
    value
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

/// An http(s) base URL without a trailing slash. It can end up in a shell
/// command (hooks), so anything but URL characters is refused.
pub fn validate_url(url: &str) -> Result<String, String> {
    let url = url.trim().trim_end_matches('/');
    let ok = (url.starts_with("http://") || url.starts_with("https://"))
        && url.len() > "https://".len()
        && url
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-._~:/[]%".contains(c));
    if ok {
        Ok(url.to_string())
    } else {
        Err(format!(
            "expected an http(s) URL such as http://host:3737, got '{url}'"
        ))
    }
}

#[cfg(test)]
mod tests;
