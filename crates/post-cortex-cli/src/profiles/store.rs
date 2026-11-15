// Copyright (c) 2025, 2026 Julius ML
// MIT License

//! The profiles on disk: `config.json` (profiles, the active one, client
//! settings) and `credentials.json` (keys, owner-only).

use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use crate::local::Local;
use crate::profiles::{
    CLOUD, CLOUD_URL, CONFIG_FILE, CREDENTIALS_FILE, ConfigFile, Env, HostSettings, Profile,
    ProfileEntry, SELF_HOSTED, clean, validate_url,
};

/// The config and credential files under one directory, plus what the
/// `self` profile and the resolution take from outside it.
pub struct Store {
    dir: PathBuf,
    /// Base URL of the daemon in `daemon.toml` (the `self` default).
    daemon_url: String,
    /// `auth_token` of `daemon.toml` (the last fallback for `self`).
    daemon_token: Option<String>,
    env: Env,
}

impl Store {
    /// `$PC_CONFIG_DIR` or `~/.post-cortex`, with `self` from `daemon.toml`
    /// and the process environment.
    pub fn open(local: &Local) -> Result<Self, String> {
        let dir = match clean(std::env::var("PC_CONFIG_DIR").ok()) {
            Some(dir) => PathBuf::from(dir),
            None => dirs::home_dir()
                .ok_or_else(|| "Cannot determine home directory".to_string())?
                .join(".post-cortex"),
        };
        Ok(Self::at(
            dir,
            local.url(),
            local.auth_token.clone(),
            Env::from_process()?,
        ))
    }

    /// A store rooted at `dir` (tests).
    pub fn at(dir: PathBuf, daemon_url: String, daemon_token: Option<String>, env: Env) -> Self {
        Self {
            dir,
            daemon_url,
            daemon_token: clean(daemon_token),
            env: env.cleaned(),
        }
    }

    /// The directory holding the files (also `projects.json`, the project
    /// cache of `project.rs`).
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn config_path(&self) -> PathBuf {
        self.dir.join(CONFIG_FILE)
    }

    /// `hosts.<author>` of `config.json` (defaults when absent or not an
    /// object).
    pub fn host(&self, author: &str) -> Result<HostSettings, String> {
        let file = self.load()?;
        let host = &file.other.get("hosts").unwrap_or(&Value::Null)[author];
        Ok(HostSettings {
            enabled: host["enabled"].as_bool() != Some(false),
            capture: host["capture"].as_bool() == Some(true),
            workspace: clean(host["workspace"].as_str().map(String::from)),
        })
    }

    /// Set `hosts.<author>.<key>` in `config.json`, keeping everything else.
    pub fn set_host(&self, author: &str, key: &str, value: Value) -> Result<(), String> {
        let mut file = self.load()?;
        let hosts = file
            .other
            .entry("hosts")
            .or_insert_with(|| Value::Object(Map::new()));
        if !hosts.is_object() {
            *hosts = Value::Object(Map::new());
        }
        let host = &mut hosts[author];
        if !host.is_object() {
            *host = Value::Object(Map::new());
        }
        host[key] = value;
        self.save(&file)
    }

    fn credentials_path(&self) -> PathBuf {
        self.dir.join(CREDENTIALS_FILE)
    }

    /// `config.json`; empty when the file is missing.
    pub fn load(&self) -> Result<ConfigFile, String> {
        let path = self.config_path();
        match std::fs::read_to_string(&path) {
            Ok(text) if text.trim().is_empty() => Ok(ConfigFile::default()),
            Ok(text) => serde_json::from_str(&text)
                .map_err(|e| format!("Failed to parse {}: {e}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(ConfigFile::default()),
            Err(e) => Err(format!("Failed to read {}: {e}", path.display())),
        }
    }

    pub fn save(&self, file: &ConfigFile) -> Result<(), String> {
        let text = format!(
            "{}\n",
            serde_json::to_string_pretty(file).map_err(|e| e.to_string())?
        );
        let path = self.config_path();
        std::fs::create_dir_all(&self.dir)
            .map_err(|e| format!("Failed to create {}: {e}", self.dir.display()))?;
        std::fs::write(&path, text).map_err(|e| format!("Failed to write {}: {e}", path.display()))
    }

    /// Every profile name with its URL (`$PC_URL` not applied), the
    /// built-in ones included, sorted.
    pub fn list(&self) -> Result<Vec<(String, String)>, String> {
        let file = self.load()?;
        let mut names: Vec<String> = [CLOUD, SELF_HOSTED].map(String::from).to_vec();
        names.extend(file.profiles.keys().cloned());
        names.sort();
        names.dedup();
        Ok(names
            .into_iter()
            .filter_map(|name| self.url_of(&file, &name).map(|url| (name, url)))
            .collect())
    }

    /// The lines of `pcx profile list`: the active profile marked `*`, the
    /// URL actually used (`$PC_URL` when set, the profile's own after it),
    /// whether a key is stored, and `(PC_PROFILE)` when the environment
    /// picked the active profile. An active profile with no entry (named
    /// by `$PC_PROFILE` or `active`) gets a line of its own.
    pub fn list_lines(&self) -> Result<Vec<String>, String> {
        let file = self.load()?;
        let active = self.active_in(&file);
        let by_env = self.env.profile.is_some();
        let mut rows: Vec<(String, Option<String>)> = self
            .list()?
            .into_iter()
            .map(|(name, url)| (name, Some(url)))
            .collect();
        if !rows.iter().any(|(name, _)| *name == active) {
            rows.push((active.clone(), None));
        }
        let mut lines = Vec::with_capacity(rows.len());
        for (name, own) in rows {
            let is_active = name == active;
            let mark = if is_active { "*" } else { " " };
            let env_url = self.env.url.as_deref().map(|u| u.trim_end_matches('/'));
            let url = match (env_url, own) {
                (Some(env), Some(own)) => format!("{env} (PC_URL; profile: {own})"),
                (Some(env), None) => format!("{env} (PC_URL; no such profile)"),
                (None, Some(own)) => own,
                (None, None) => "(no such profile)".to_string(),
            };
            let key = if self.stored_token(&name)?.is_some() {
                "  (logged in)"
            } else {
                ""
            };
            let picked = if is_active && by_env {
                "  (PC_PROFILE)"
            } else {
                ""
            };
            lines.push(format!("{mark} {name:<12} {url}{key}{picked}"));
        }
        Ok(lines)
    }

    /// The active profile's name: `$PC_PROFILE`, else `active`, else
    /// `cloud`.
    pub fn active(&self) -> Result<String, String> {
        let file = self.load()?;
        Ok(self.active_in(&file))
    }

    fn active_in(&self, file: &ConfigFile) -> String {
        self.env
            .profile
            .clone()
            .or_else(|| file.active.clone())
            .unwrap_or_else(|| CLOUD.to_string())
    }

    fn url_of(&self, file: &ConfigFile, name: &str) -> Option<String> {
        match (file.profiles.get(name), name) {
            (Some(entry), _) => Some(entry.url.trim_end_matches('/').to_string()),
            (None, CLOUD) => Some(CLOUD_URL.to_string()),
            (None, SELF_HOSTED) => Some(self.daemon_url.trim_end_matches('/').to_string()),
            (None, _) => None,
        }
    }

    /// The profile `name` (the active one when `None`) with its URL and
    /// token, the environment applied (see the module docs).
    pub fn resolve(&self, name: Option<&str>) -> Result<Profile, String> {
        let file = self.load()?;
        let name = match name {
            Some(name) => name.to_string(),
            None => self.active_in(&file),
        };
        let url = match &self.env.url {
            Some(url) => url.trim_end_matches('/').to_string(),
            None => self.url_of(&file, &name).ok_or_else(|| {
                format!(
                    "No profile '{name}' (pcx profile list shows them, pcx profile add creates one)"
                )
            })?,
        };
        let token = self.token(&name)?;
        Ok(Profile { name, url, token })
    }

    /// The bearer token for `profile`: `$PC_AUTH_TOKEN`, else the profile's
    /// credentials, else (for `self`) `auth_token` in `daemon.toml`.
    pub fn token(&self, profile: &str) -> Result<Option<String>, String> {
        if let Some(token) = &self.env.token {
            return Ok(Some(token.clone()));
        }
        if let Some(token) = self.stored_token(profile)? {
            return Ok(Some(token));
        }
        Ok((profile == SELF_HOSTED)
            .then(|| self.daemon_token.clone())
            .flatten())
    }

    /// Make `name` the active profile.
    pub fn use_profile(&self, name: &str) -> Result<(), String> {
        let mut file = self.load()?;
        if self.url_of(&file, name).is_none() {
            return Err(format!(
                "No profile '{name}' (pcx profile add {name} --url <URL>)"
            ));
        }
        file.active = Some(name.to_string());
        self.save(&file)
    }

    /// Add `name` at `url`, or change its URL (a built-in profile's URL is
    /// overridden). Other keys of an existing entry are kept.
    pub fn add(&self, name: &str, url: &str) -> Result<(), String> {
        let name = name.trim();
        if name.is_empty()
            || !name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
        {
            return Err(format!(
                "Profile names take letters, digits, '-' and '_', got '{name}'"
            ));
        }
        let url = validate_url(url)?;
        let mut file = self.load()?;
        file.profiles
            .entry(name.to_string())
            .and_modify(|entry| entry.url = url.clone())
            .or_insert(ProfileEntry {
                url,
                other: Map::new(),
            });
        self.save(&file)
    }

    /// Remove `name` and its credentials. For `cloud` and `self` only the
    /// URL override goes (they always exist). The active profile falls back
    /// to `cloud` when it was `name` and is gone.
    pub fn remove(&self, name: &str) -> Result<(), String> {
        let mut file = self.load()?;
        let builtin = matches!(name, CLOUD | SELF_HOSTED);
        if file.profiles.remove(name).is_none() && !builtin {
            return Err(format!("No profile '{name}'"));
        }
        if !builtin && file.active.as_deref() == Some(name) {
            file.active = None;
        }
        self.save(&file)?;
        if !builtin {
            self.forget_token(name)?;
        }
        Ok(())
    }

    // ------------------------------------------------------- credentials

    /// `credentials.json` as a JSON object (unknown keys kept on save).
    fn load_credentials(&self) -> Result<Map<String, Value>, String> {
        let path = self.credentials_path();
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Map::new()),
            Err(e) => return Err(format!("Failed to read {}: {e}", path.display())),
        };
        if let Some(mode) = loose_mode(&path) {
            eprintln!(
                "warning: {} is readable by others (mode {mode:o}); run chmod 600 on it",
                path.display()
            );
        }
        if text.trim().is_empty() {
            return Ok(Map::new());
        }
        serde_json::from_str(&text).map_err(|e| format!("Failed to parse {}: {e}", path.display()))
    }

    fn save_credentials(&self, creds: &Map<String, Value>) -> Result<(), String> {
        let text = format!(
            "{}\n",
            serde_json::to_string_pretty(creds).map_err(|e| e.to_string())?
        );
        std::fs::create_dir_all(&self.dir)
            .map_err(|e| format!("Failed to create {}: {e}", self.dir.display()))?;
        write_private(&self.credentials_path(), &text)
    }

    /// The token `pcx login` stored for `profile`.
    pub fn stored_token(&self, profile: &str) -> Result<Option<String>, String> {
        Ok(clean(
            self.load_credentials()?
                .get(profile)
                .and_then(|c| c["token"].as_str())
                .map(String::from),
        ))
    }

    /// Store `token` for `profile` (credentials file, mode 0600).
    pub fn store_token(&self, profile: &str, token: &str) -> Result<(), String> {
        let mut creds = self.load_credentials()?;
        let entry = creds
            .entry(profile.to_string())
            .or_insert_with(|| Value::Object(Map::new()));
        if !entry.is_object() {
            *entry = Value::Object(Map::new());
        }
        entry["token"] = Value::String(token.trim().to_string());
        self.save_credentials(&creds)
    }

    /// Drop the stored token of `profile`; whether there was one.
    pub fn forget_token(&self, profile: &str) -> Result<bool, String> {
        let mut creds = self.load_credentials()?;
        if creds.remove(profile).is_none() {
            return Ok(false);
        }
        self.save_credentials(&creds)?;
        Ok(true)
    }

    /// Where the credentials live, for messages.
    pub fn credentials_display(&self) -> String {
        self.credentials_path().display().to_string()
    }
}
/// Write `text` to `path` readable by the owner only: a temp file created
/// with mode 0600, then renamed over `path`.
fn write_private(path: &Path, text: &str) -> Result<(), String> {
    use std::io::Write;
    let tmp = path.with_extension("json.tmp");
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(&tmp)
        .map_err(|e| format!("Failed to write {}: {e}", tmp.display()))?;
    #[cfg(unix)]
    {
        // A leftover temp file keeps its old mode; open does not change it.
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))
            .map_err(|e| format!("Failed to chmod {}: {e}", tmp.display()))?;
    }
    file.write_all(text.as_bytes())
        .and_then(|_| file.sync_all())
        .map_err(|e| format!("Failed to write {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("Failed to write {}: {e}", path.display()))
}

/// The file's mode when group or others can read or write it.
#[cfg(unix)]
pub fn loose_mode(path: &Path) -> Option<u32> {
    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::metadata(path).ok()?.permissions().mode() & 0o777;
    (mode & 0o077 != 0).then_some(mode)
}

#[cfg(not(unix))]
pub fn loose_mode(_path: &Path) -> Option<u32> {
    None
}
