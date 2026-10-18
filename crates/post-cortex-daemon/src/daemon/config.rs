// Copyright (c) 2025 Julius ML
//
// Permission is hereby granted, free of charge, to any person obtaining a copy
// of this software and associated documentation files (the "Software"), to deal
// in the Software without restriction, including without limitation the rights
// to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
// copies of the Software, and to permit persons to whom the Software is
// furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in all
// copies or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
// SOFTWARE.

//! Daemon configuration file support
//!
//! Loads configuration from TOML file with environment variable overrides.
//! Priority: Environment variables > Config file > Defaults

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

/// Daemon configuration loaded from file and environment
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonConfig {
    /// Host to bind to (default: 127.0.0.1)
    pub host: String,

    /// Port to listen on (default: 3737)
    pub port: u16,

    /// gRPC port (default: 3738, 0 = disabled)
    #[serde(default = "default_grpc_port")]
    pub grpc_port: u16,

    /// Data directory for RocksDB (default: ~/.post-cortex/data)
    pub data_directory: String,

    /// Storage backend: "rocksdb" or "surrealdb" (default: rocksdb)
    #[serde(default = "default_storage_backend")]
    pub storage_backend: String,

    /// SurrealDB endpoint (required if storage_backend = "surrealdb")
    /// Example: "ws://localhost:8000"
    #[serde(default)]
    pub surrealdb_endpoint: Option<String>,

    /// SurrealDB username (optional)
    #[serde(default)]
    pub surrealdb_username: Option<String>,

    /// SurrealDB password (optional)
    #[serde(default)]
    pub surrealdb_password: Option<String>,

    /// SurrealDB namespace (default: "post_cortex")
    #[serde(default = "default_surrealdb_namespace")]
    pub surrealdb_namespace: String,

    /// SurrealDB database (default: "main")
    #[serde(default = "default_surrealdb_database")]
    pub surrealdb_database: String,

    /// Bearer token required on every HTTP (except `/health`) and gRPC call.
    /// Unset = no authentication, only safe on a loopback bind.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_token: Option<String>,

    /// Redact secrets and personal data on every write (default: true)
    #[serde(default = "default_redact")]
    pub redact: bool,

    /// Extra redaction regexes; matches become `[REDACTED:custom]`
    #[serde(default)]
    pub redact_patterns: Vec<String>,

    /// Extra `Host` header values the HTTP endpoints accept (DNS rebinding
    /// protection). Each entry is `host` (any port) or `host:port`; `"*"`
    /// disables the check. Always added to the defaults: `localhost`,
    /// `127.0.0.1`, `::1` and `host` when it is not a wildcard address.
    /// See [`crate::daemon::host_allowlist`].
    #[serde(default)]
    pub allowed_hosts: Vec<String>,
}

fn default_redact() -> bool {
    true
}

fn default_grpc_port() -> u16 {
    3738
}

fn default_storage_backend() -> String {
    "rocksdb".to_string()
}

fn default_surrealdb_namespace() -> String {
    "post_cortex".to_string()
}

fn default_surrealdb_database() -> String {
    "main".to_string()
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".to_string(),
            port: 3737,
            grpc_port: default_grpc_port(),
            data_directory: default_data_dir(),
            storage_backend: default_storage_backend(),
            surrealdb_endpoint: None,
            surrealdb_username: None,
            surrealdb_password: None,
            surrealdb_namespace: default_surrealdb_namespace(),
            surrealdb_database: default_surrealdb_database(),
            auth_token: None,
            redact: default_redact(),
            redact_patterns: Vec::new(),
            allowed_hosts: Vec::new(),
        }
    }
}

impl DaemonConfig {
    /// Load configuration from file with environment variable overrides
    ///
    /// Priority order:
    /// 1. Environment variables (PC_HOST, PC_PORT, PC_DATA_DIR)
    /// 2. Config file (~/.post-cortex/daemon.toml)
    /// 3. Default values
    pub fn load() -> Self {
        let config_path = default_config_path();

        // Start with defaults
        let mut config = Self::default();

        // Try to load config file if it exists
        if config_path.exists() {
            match fs::read_to_string(&config_path) {
                Ok(contents) => match toml::from_str::<DaemonConfig>(&contents) {
                    Ok(file_config) => {
                        // Merge file config into defaults
                        config = file_config;
                        tracing::info!("Loaded configuration from {:?}", config_path);
                    }
                    Err(e) => {
                        tracing::warn!("Failed to parse config file {:?}: {}", config_path, e);
                        tracing::info!("Using default configuration");
                    }
                },
                Err(e) => {
                    tracing::warn!("Failed to read config file {:?}: {}", config_path, e);
                    tracing::info!("Using default configuration");
                }
            }
        } else {
            tracing::debug!("Config file {:?} not found, using defaults", config_path);
        }

        // Apply environment variable overrides (highest priority)
        if let Ok(host) = std::env::var("PC_HOST") {
            config.host = host;
            tracing::debug!("Overriding host from PC_HOST environment variable");
        }

        if let Ok(port_str) = std::env::var("PC_PORT") {
            if let Ok(port) = port_str.parse::<u16>() {
                config.port = port;
                tracing::debug!("Overriding port from PC_PORT environment variable");
            } else {
                tracing::warn!("Invalid PC_PORT value: {}", port_str);
            }
        }

        if let Ok(grpc_port_str) = std::env::var("PC_GRPC_PORT")
            && let Ok(port) = grpc_port_str.parse::<u16>()
        {
            config.grpc_port = port;
            tracing::debug!("Overriding grpc_port from PC_GRPC_PORT environment variable");
        }

        if let Ok(data_dir) = std::env::var("PC_DATA_DIR") {
            config.data_directory = data_dir;
            tracing::debug!("Overriding data_directory from PC_DATA_DIR environment variable");
        }

        // Storage backend overrides
        if let Ok(backend) = std::env::var("PC_STORAGE_BACKEND") {
            config.storage_backend = backend;
            tracing::debug!(
                "Overriding storage_backend from PC_STORAGE_BACKEND environment variable"
            );
        }

        if let Ok(endpoint) = std::env::var("PC_SURREALDB_ENDPOINT") {
            config.surrealdb_endpoint = Some(endpoint);
            tracing::debug!(
                "Overriding surrealdb_endpoint from PC_SURREALDB_ENDPOINT environment variable"
            );
        }

        if let Ok(username) = std::env::var("PC_SURREALDB_USER") {
            config.surrealdb_username = Some(username);
            tracing::debug!(
                "Overriding surrealdb_username from PC_SURREALDB_USER environment variable"
            );
        }

        if let Ok(password) = std::env::var("PC_SURREALDB_PASS") {
            config.surrealdb_password = Some(password);
            tracing::debug!(
                "Overriding surrealdb_password from PC_SURREALDB_PASS environment variable"
            );
        }

        if let Ok(token) = std::env::var("PC_AUTH_TOKEN") {
            config.auth_token = (!token.is_empty()).then_some(token);
            tracing::debug!("Overriding auth_token from PC_AUTH_TOKEN environment variable");
        }

        if let Ok(hosts) = std::env::var("PC_ALLOWED_HOSTS") {
            config.allowed_hosts = parse_host_list(&hosts);
            tracing::debug!("Overriding allowed_hosts from PC_ALLOWED_HOSTS environment variable");
        }

        if let Ok(redact) = std::env::var("PC_REDACT") {
            config.redact = !matches!(redact.as_str(), "0" | "false" | "no" | "off");
            tracing::debug!("Overriding redact from PC_REDACT environment variable");
        }

        config
    }

    /// HTTP client for talking to the daemon: carries the bearer token when
    /// one is configured, so CLI commands and the stdio proxy pass auth.
    pub fn http_client(&self) -> reqwest::Client {
        let mut headers = reqwest::header::HeaderMap::new();
        if let Some(token) = &self.auth_token
            && let Ok(mut value) =
                reqwest::header::HeaderValue::from_str(&format!("Bearer {token}"))
        {
            value.set_sensitive(true);
            headers.insert(reqwest::header::AUTHORIZATION, value);
        }
        reqwest::Client::builder()
            .default_headers(headers)
            .build()
            .unwrap_or_default()
    }

    /// Create example config file at the default location
    ///
    /// Returns path to created file or error message
    pub fn create_example_config() -> Result<PathBuf, String> {
        let config_path = default_config_path();

        if config_path.exists() {
            return Err(format!("Config file already exists at {:?}", config_path));
        }

        // Ensure parent directory exists
        if let Some(parent) = config_path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create config directory: {}", e))?;
        }

        let example_config = DaemonConfig::default();
        let toml_content = toml::to_string_pretty(&example_config)
            .map_err(|e| format!("Failed to serialize config: {}", e))?;

        // Add comments to explain each field
        let commented_toml = format!(
            "# Post-Cortex Daemon Configuration\n\
             # \n\
             # This file configures the HTTP daemon server for multi-client access.\n\
             # Environment variables override these settings:\n\
             #   PC_HOST - Override host\n\
             #   PC_PORT - Override port\n\
             #   PC_DATA_DIR - Override data directory\n\
             #   PC_STORAGE_BACKEND - Storage backend: \"rocksdb\" or \"surrealdb\"\n\
             #   PC_SURREALDB_ENDPOINT - SurrealDB WebSocket endpoint (e.g., \"ws://localhost:8000\")\n\
             #   PC_SURREALDB_USER - SurrealDB username\n\
             #   PC_SURREALDB_PASS - SurrealDB password\n\
             #   PC_AUTH_TOKEN - Bearer token required by HTTP/gRPC clients\n\
             #   PC_REDACT - \"0\" disables redaction of secrets/personal data\n\
             #   PC_ALLOWED_HOSTS - Comma-separated extra allowed Host headers\n\
             # \n\
             # allowed_hosts: Host header values accepted on /mcp and /api/*\n\
             # (DNS rebinding protection). localhost, 127.0.0.1, ::1 and a\n\
             # non-wildcard `host` are always allowed; other requests get 403.\n\
             # When binding to 0.0.0.0, list the names remote clients use:\n\
             #   allowed_hosts = [\"my-mac.tailnet.ts.net\", \"100.64.0.7:3737\"]\n\
             # Entries are host (any port) or host:port; [\"*\"] disables the check.\n\
             # \n\
             # Priority: Environment > Config file > Defaults\n\n\
             {}",
            toml_content
        );

        fs::write(&config_path, commented_toml)
            .map_err(|e| format!("Failed to write config file: {}", e))?;

        Ok(config_path)
    }

    /// Validate configuration values
    pub fn validate(&self) -> Result<(), String> {
        // Validate host
        if self.host.is_empty() {
            return Err("Host cannot be empty".to_string());
        }

        // Validate port range
        if self.port == 0 {
            return Err("Port cannot be 0".to_string());
        }

        // Validate data directory is not empty
        if self.data_directory.is_empty() {
            return Err("Data directory cannot be empty".to_string());
        }

        crate::daemon::host_allowlist::HostPolicy::from_config(self)?;

        Ok(())
    }
}

/// Splits a comma-separated host list, dropping empty items.
fn parse_host_list(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(str::trim)
        .filter(|h| !h.is_empty())
        .map(str::to_string)
        .collect()
}

/// Get default config file path: ~/.post-cortex/daemon.toml
fn default_config_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".post-cortex")
        .join("daemon.toml")
}

/// Get default data directory: ~/.post-cortex/data
fn default_data_dir() -> String {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".post-cortex/data")
        .to_str()
        .unwrap()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;

    #[test]
    fn test_default_config() {
        let config = DaemonConfig::default();
        assert_eq!(config.host, "127.0.0.1");
        assert_eq!(config.port, 3737);
        assert!(config.data_directory.contains(".post-cortex/data"));
    }

    #[test]
    fn test_config_validation() {
        let config = DaemonConfig::default();
        assert!(config.validate().is_ok());

        let invalid = DaemonConfig {
            host: "".to_string(),
            ..Default::default()
        };
        assert!(invalid.validate().is_err());

        let invalid_port = DaemonConfig {
            port: 0,
            ..Default::default()
        };
        assert!(invalid_port.validate().is_err());
    }

    #[test]
    fn test_env_override() {
        // SAFETY: This test runs in isolation and only modifies env vars
        // for the duration of the test. No other tests depend on these env vars.
        #[allow(unsafe_code)]
        unsafe {
            env::set_var("PC_HOST", "0.0.0.0");
            env::set_var("PC_PORT", "8080");
            env::set_var("PC_DATA_DIR", "/tmp/test-data");
        }

        let config = DaemonConfig::load();

        assert_eq!(config.host, "0.0.0.0");
        assert_eq!(config.port, 8080);
        assert_eq!(config.data_directory, "/tmp/test-data");

        // SAFETY: Cleanup of env vars set above
        #[allow(unsafe_code)]
        unsafe {
            env::remove_var("PC_HOST");
            env::remove_var("PC_PORT");
            env::remove_var("PC_DATA_DIR");
        }
    }

    #[test]
    fn test_host_list_parsing() {
        assert_eq!(
            parse_host_list(" a.example , b.example:3737,,"),
            vec!["a.example", "b.example:3737"]
        );
        assert!(parse_host_list("").is_empty());
    }

    #[test]
    fn test_allowed_hosts_toml() {
        let parsed: DaemonConfig = toml::from_str(
            "host = \"0.0.0.0\"\nport = 3737\ndata_directory = \"/d\"\n\
             allowed_hosts = [\"my-mac.tailnet.ts.net\", \"100.64.0.7:3737\"]\n",
        )
        .unwrap();
        assert_eq!(
            parsed.allowed_hosts,
            vec!["my-mac.tailnet.ts.net", "100.64.0.7:3737"]
        );
        assert!(parsed.validate().is_ok());

        let invalid = DaemonConfig {
            allowed_hosts: vec!["http://x".into()],
            ..Default::default()
        };
        assert!(invalid.validate().is_err());
    }

    #[test]
    fn test_toml_serialization() {
        let config = DaemonConfig::default();
        let toml_str = toml::to_string(&config).unwrap();
        let parsed: DaemonConfig = toml::from_str(&toml_str).unwrap();

        assert_eq!(config.host, parsed.host);
        assert_eq!(config.port, parsed.port);
        assert_eq!(config.data_directory, parsed.data_directory);
    }
}
