// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The local daemon as the `pcx` binary places it (from `daemon.toml` and
//! the environment), and the HTTP client every command talks to a backend
//! with.

use std::path::PathBuf;

/// The local daemon: where it listens, its token, its data directory.
#[derive(Debug, Clone)]
pub struct Local {
    /// Host it binds.
    pub host: String,
    /// HTTP port.
    pub port: u16,
    /// gRPC port.
    pub grpc_port: u16,
    /// Bearer token it requires, if any.
    pub auth_token: Option<String>,
    /// Its data directory (passed on when `pcx` starts it).
    pub data_directory: PathBuf,
    /// The directory of its `daemon.toml`, where `pcx connect` keeps the
    /// pairing (`connect.json`).
    pub config_dir: PathBuf,
    /// Days it keeps captured turns (0: forever), for `pcx setup` to say.
    pub message_retention_days: u32,
}

impl Local {
    /// Its base URL, the one hooks and clients use (a wildcard bind is
    /// reached on loopback; an IPv6 host in brackets).
    #[must_use]
    pub fn url(&self) -> String {
        let host = match self.host.as_str() {
            "0.0.0.0" | "::" | "" => "127.0.0.1",
            host => host,
        };
        if host.contains(':') {
            format!("http://[{}]:{}", host, self.port)
        } else {
            format!("http://{}:{}", host, self.port)
        }
    }

    /// Its gRPC URL (as [`Local::url`], on its gRPC port).
    #[must_use]
    pub fn grpc_url(&self) -> String {
        let host = match self.host.as_str() {
            "0.0.0.0" | "::" | "" => "127.0.0.1",
            host => host,
        };
        if host.contains(':') {
            format!("http://[{}]:{}", host, self.grpc_port)
        } else {
            format!("http://{}:{}", host, self.grpc_port)
        }
    }

    /// An HTTP client for it: its token, and `PC_AUTHOR` naming the writes.
    ///
    /// # Errors
    /// See [`bearer_client`].
    pub fn http_client(&self) -> Result<reqwest::Client, ClientError> {
        let author = env_text("PC_AUTHOR").map_err(ClientError::Env)?;
        bearer_client(self.auth_token.as_deref(), author.as_deref(), None)
    }
}

/// Why an HTTP client for a backend could not be made.
#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    /// The API key cannot be sent as a header; nothing is sent without it.
    #[error("the API key is not a valid HTTP header value")]
    InvalidToken,
    /// The author cannot be sent as a header.
    #[error("the author {0:?} is not a valid HTTP header value")]
    InvalidAuthor(String),
    /// The TLS backend or the system configuration refused the client.
    #[error("cannot build the HTTP client: {0}")]
    Build(#[from] reqwest::Error),
    /// An environment variable it reads is not text.
    #[error("{0}")]
    Env(String),
}

/// The value of the environment variable `name`: `None` when it is not
/// set.
///
/// # Errors
/// It is set but not valid UTF-8 (never read as unset).
pub fn env_text(name: &str) -> Result<Option<String>, String> {
    match std::env::var(name) {
        Ok(value) => Ok(Some(value)),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(std::env::VarError::NotUnicode(_)) => Err(format!("{name} is not valid UTF-8")),
    }
}

/// HTTP client for a post-cortex backend (the local daemon, a self-hosted
/// one or the hosted cloud): `Authorization: Bearer <token>` when a token is
/// given and `X-PCX-Author` naming the caller as the author of its writes,
/// every request bounded by `timeout` when one is given. Empty values are
/// left out.
///
/// # Errors
/// A token or author that is not a valid header value, or a client the
/// system refuses: never a client that silently drops the key.
pub fn bearer_client(
    token: Option<&str>,
    author: Option<&str>,
    timeout: Option<std::time::Duration>,
) -> Result<reqwest::Client, ClientError> {
    let mut headers = reqwest::header::HeaderMap::new();
    if let Some(token) = token.map(str::trim).filter(|t| !t.is_empty()) {
        let mut value = reqwest::header::HeaderValue::from_str(&format!("Bearer {token}"))
            .map_err(|_| ClientError::InvalidToken)?;
        value.set_sensitive(true);
        headers.insert(reqwest::header::AUTHORIZATION, value);
    }
    if let Some(author) = author.map(str::trim).filter(|a| !a.is_empty()) {
        let value = reqwest::header::HeaderValue::from_str(author)
            .map_err(|_| ClientError::InvalidAuthor(author.to_string()))?;
        headers.insert(post_cortex_proto::AUTHOR_HEADER, value);
    }
    let mut builder = reqwest::Client::builder().default_headers(headers);
    if let Some(timeout) = timeout {
        builder = builder.timeout(timeout);
    }
    Ok(builder.build()?)
}

#[cfg(test)]
mod tests;
