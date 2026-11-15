// Copyright (c) 2026 Julius ML
// MIT License

//! Project resolution: which post-cortex session and workspace a directory
//! belongs to, for hooks installed globally (`pcx hook` without
//! `--session` / `--workspace`) and for `pcx setup --project`.
//!
//! The same rules are implemented in Python by `templates/pcx_resolve.py`
//! (the Claude Code hooks) and by the native plugins; the test vectors in
//! `integrations/testdata/project-keys.json` are asserted by all of them.
//!
//! # Project key
//!
//! 1. The git remote `origin` of the repository the directory is in,
//!    normalised ([`normalize_remote`]): no scheme, user or port, no `.git`,
//!    `git@host:a/b` as `host/a/b`, the host lowercased, e.g.
//!    `github.com/julymetodiev/post-cortex`. Teammates cloning the same
//!    remote share the key, hence the memory.
//! 2. No such remote: `local:<sha256 of the absolute git root, 16 hex>`.
//! 3. Not a git repository: the same with the directory.
//!
//! The name (the workspace and session name of a new project) is the last
//! path segment of the remote, else the root directory's name.
//!
//! # Ids, first hit wins
//!
//! 1. `.post-cortex.json` `{"session_id","workspace_id"}` at the root,
//!    written by `pcx setup --project` (fixed ids in the repository win);
//! 2. the ids in the instructions file a project mode setup wrote before
//!    (`AGENTS.md` for the pcx hooks, `CLAUDE.md` for the Claude Code ones);
//! 3. the cache `projects.json` in the config directory
//!    (`{"<key>":{"workspace_id","session_id","url"}}`, an entry counts only
//!    for the same backend URL);
//! 4. `POST /api/projects/resolve` `{key, name, client, workspace}` (the
//!    workspace from `hosts.<author>.workspace` of `config.json`), whose
//!    answer is cached.

use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

/// The file with a project's fixed ids, at its root.
pub const PINNED_FILE: &str = ".post-cortex.json";
const CACHE_FILE: &str = "projects.json";

/// A project as resolution sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Project {
    pub key: String,
    pub name: String,
    /// The git root, else the directory itself (canonical path).
    pub root: PathBuf,
}

/// A project's session and workspace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ids {
    pub session_id: String,
    pub workspace_id: String,
}

/// `host/path` of a git remote URL, or `None` when it names no host (a
/// local path, `file://`, an empty string).
pub fn normalize_remote(url: &str) -> Option<String> {
    let url = url.trim();
    if url.is_empty() || url.contains('\\') {
        return None;
    }
    let (host, path) = match url.split_once("://") {
        Some((scheme, rest)) if is_scheme(scheme) => {
            if scheme.eq_ignore_ascii_case("file") {
                return None;
            }
            let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
            let host = authority.rsplit('@').next().unwrap_or("");
            let host = match host.strip_prefix('[') {
                Some(v6) => format!("[{}]", v6.split(']').next().unwrap_or("")),
                None => host.split(':').next().unwrap_or("").to_string(),
            };
            (host, path)
        }
        _ => {
            // scp-like `[user@]host:path`: a colon before any slash.
            let colon = url.find(':')?;
            if colon == 0 || url.find('/').is_some_and(|slash| slash < colon) {
                return None;
            }
            let host = url[..colon].rsplit('@').next().unwrap_or("");
            if host.chars().count() < 2 {
                return None; // a Windows drive letter
            }
            (host.to_string(), &url[colon + 1..])
        }
    };
    let host = host.to_lowercase();
    let mut path = path.trim_matches('/');
    if let Some(stripped) = path.strip_suffix(".git") {
        path = stripped.trim_end_matches('/');
    }
    if host.is_empty() || path.is_empty() {
        return None;
    }
    Some(format!("{host}/{path}"))
}

fn is_scheme(s: &str) -> bool {
    let mut chars = s.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || "+.-".contains(c))
}

/// `local:<first 16 hex of sha256(root)>`.
pub fn local_key(root: &str) -> String {
    let digest = Sha256::digest(root.as_bytes());
    let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    format!("local:{}", &hex[..16])
}

/// The nearest directory at or above `dir` holding `.git` (a directory, or
/// the file of a worktree or submodule).
pub fn git_root(dir: &Path) -> Option<PathBuf> {
    dir.ancestors()
        .find(|d| d.join(".git").exists())
        .map(Path::to_path_buf)
}

/// `remote.origin.url` of the repository at `root`, through `git`.
fn git_origin(root: &Path) -> Option<String> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["config", "--get", "remote.origin.url"])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    let url = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (out.status.success() && !url.is_empty()).then_some(url)
}

/// The root of the project `dir` is in: its git root, else `dir`
/// (canonical paths).
pub fn project_root(dir: &Path) -> PathBuf {
    let dir = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
    git_root(&dir).unwrap_or(dir)
}

/// The project `dir` is in (see the module docs).
pub fn project(dir: &Path) -> Project {
    let dir = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
    let root = git_root(&dir);
    if let Some(root) = &root
        && let Some(key) = git_origin(root).as_deref().and_then(normalize_remote)
    {
        let name = key.rsplit('/').next().unwrap_or(&key).to_string();
        return Project {
            key,
            name,
            root: root.clone(),
        };
    }
    let root = root.unwrap_or(dir);
    let name = root
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| root.display().to_string());
    Project {
        key: local_key(&root.to_string_lossy()),
        name,
        root,
    }
}

fn read_json(path: &Path) -> Value {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .filter(Value::is_object)
        .unwrap_or_else(|| Value::Object(Map::new()))
}

fn ids_of(value: &Value) -> Option<Ids> {
    let get = |key: &str| {
        value[key]
            .as_str()
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(String::from)
    };
    Some(Ids {
        session_id: get("session_id")?,
        workspace_id: get("workspace_id")?,
    })
}

/// The first UUID after `label` in `text` (`Session ID`, `Workspace ID`,
/// with the markdown of the setup templates around it).
fn uuid_after(text: &str, label: &str) -> Option<String> {
    text.match_indices(label).find_map(|(start, _)| {
        let rest = text[start + label.len()..]
            .trim_start_matches(|c: char| !c.is_ascii_alphanumeric() && c != '_');
        let candidate: String = rest.chars().take(36).collect();
        (candidate.len() == 36 && uuid::Uuid::parse_str(&candidate).is_ok()).then_some(candidate)
    })
}

/// The ids fixed in the repository at `root`: `.post-cortex.json`, else
/// the first of `files` (instructions files) naming both ids.
pub fn pinned_ids(root: &Path, files: &[&str]) -> Option<Ids> {
    if let Some(ids) = ids_of(&read_json(&root.join(PINNED_FILE))) {
        return Some(ids);
    }
    files.iter().find_map(|name| {
        let text = std::fs::read_to_string(root.join(name)).ok()?;
        Some(Ids {
            session_id: uuid_after(&text, "Session ID")?,
            workspace_id: uuid_after(&text, "Workspace ID")?,
        })
    })
}

/// Write `.post-cortex.json` at `root` (other keys in it are kept).
pub fn write_pinned(root: &Path, ids: &Ids) -> Result<crate::clients::common::Change, String> {
    crate::clients::common::merge_json(
        &root.join(PINNED_FILE),
        &json!({ "session_id": ids.session_id, "workspace_id": ids.workspace_id }),
    )
}

/// The cached ids of `key` for the backend at `url`.
pub fn cached_ids(config_dir: &Path, key: &str, url: &str) -> Option<Ids> {
    let cache = read_json(&config_dir.join(CACHE_FILE));
    let entry = &cache[key];
    let same = entry["url"].as_str()?.trim_end_matches('/') == url.trim_end_matches('/');
    same.then(|| ids_of(entry)).flatten()
}

/// Remember the ids of `key` for the backend at `url` (best effort: a
/// cache that cannot be written only costs a request next time).
pub fn cache_ids(config_dir: &Path, key: &str, url: &str, ids: &Ids) {
    let path = config_dir.join(CACHE_FILE);
    let mut cache = read_json(&path);
    cache[key] = json!({
        "workspace_id": ids.workspace_id,
        "session_id": ids.session_id,
        "url": url.trim_end_matches('/'),
    });
    let tmp = path.with_extension(format!("json.{}.tmp", std::process::id()));
    let written = serde_json::to_string_pretty(&cache)
        .map_err(std::io::Error::from)
        .and_then(|text| {
            std::fs::create_dir_all(config_dir)?;
            std::fs::write(&tmp, format!("{text}\n"))?;
            std::fs::rename(&tmp, &path)
        });
    if let Err(e) = written {
        eprintln!(
            "pcx: could not cache project ids in {}: {e}",
            path.display()
        );
    }
}

/// Where and as whom a resolution asks.
pub struct Resolver<'a> {
    /// The bearer client of the backend.
    pub client: &'a reqwest::Client,
    /// The backend's base URL, without a trailing slash.
    pub url: &'a str,
    /// `X-PCX-Author` / `client` of the request.
    pub author: &'a str,
    /// `hosts.<author>.workspace`: the workspace name a new key goes into.
    pub workspace: Option<&'a str>,
    /// The config directory (the cache's home).
    pub config_dir: &'a Path,
    /// Instructions files whose ids count as fixed (see [`pinned_ids`]).
    pub instruction_files: &'a [&'a str],
}

impl Resolver<'_> {
    /// The ids of the project `dir` is in (see the module docs).
    pub async fn ids(&self, dir: &Path) -> Result<Ids, String> {
        let project = project(dir);
        if let Some(ids) = pinned_ids(&project.root, self.instruction_files) {
            return Ok(ids);
        }
        if let Some(ids) = cached_ids(self.config_dir, &project.key, self.url) {
            return Ok(ids);
        }
        let ids = self.resolve(&project).await?;
        cache_ids(self.config_dir, &project.key, self.url, &ids);
        Ok(ids)
    }

    /// `POST /api/projects/resolve` for `project`.
    pub async fn resolve(&self, project: &Project) -> Result<Ids, String> {
        let mut body = json!({
            "key": project.key,
            "name": project.name,
            "client": self.author,
        });
        if let Some(workspace) = self.workspace {
            body["workspace"] = json!(workspace);
        }
        let response = self
            .client
            .post(format!("{}/api/projects/resolve", self.url))
            .header("X-PCX-Author", self.author)
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("{}: {e}", self.url))?;
        if !response.status().is_success() {
            return Err(http_error("/api/projects/resolve", response).await);
        }
        let answer: Value = response.json().await.map_err(|e| e.to_string())?;
        ids_of(&answer).ok_or_else(|| "/api/projects/resolve: no ids in the answer".to_string())
    }
}

/// Longest part of an error answer quoted in a message.
const MAX_ERROR_BODY_CHARS: usize = 500;

/// `<what>: HTTP <status>`, with the server's own message after a colon
/// when the answer has one (a gateway's 403 says why the key is refused).
pub async fn http_error(what: &str, response: reqwest::Response) -> String {
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    let body: String = body.trim().chars().take(MAX_ERROR_BODY_CHARS).collect();
    if body.is_empty() {
        format!("{what}: HTTP {status}")
    } else {
        format!("{what}: HTTP {status}: {body}")
    }
}

#[cfg(test)]
mod tests;
