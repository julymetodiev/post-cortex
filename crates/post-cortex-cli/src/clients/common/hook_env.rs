// Copyright (c) 2026 Julius ML
// MIT License

//! The hook side of the clients: the backend calls a hook makes and the
//! texts it puts in front of the agent.

use std::path::PathBuf;

use serde_json::{Value, json};

use crate::project::{Ids, Resolver, http_error};

/// What a hook call knows: the backend, the bearer token (resolved from the
/// profile by the caller), the author name and the project's session and
/// workspace. A project mode hook passes the ids (`pcx hook ... --session
/// --workspace`); a global one resolves them on first use from the
/// directory the client runs in (see `project.rs`).
pub struct HookEnv {
    pub daemon_url: String,
    pub author: String,
    client: reqwest::Client,
    source: IdSource,
    resolved: tokio::sync::OnceCell<Option<Ids>>,
    /// Whether `capture` stores turns. Always for a project-mode hook (its
    /// capture hooks are installed only with `--capture`); for a global hook,
    /// the client's `hosts.<author>.capture` in config.json.
    capture: bool,
}

/// What the agent is doing with a file whose memory a hook asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileMoment {
    /// It read the file (a post-tool event).
    Read,
    /// It is about to change the file (a pre-tool event).
    Edit,
}

impl FileMoment {
    fn event_name(self) -> &'static str {
        match self {
            Self::Read => "PostToolUse",
            Self::Edit => "PreToolUse",
        }
    }
}

/// A file's memory as the memory module answered a hook.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileMemory {
    /// Add it to what the agent reads.
    Added(String),
    /// Hold the edit once with it as the reason the agent reads; the same
    /// edit sent again goes through.
    Held(String),
}

impl FileMemory {
    /// The text, for a client that can only add context.
    pub fn into_text(self) -> String {
        match self {
            Self::Added(text) | Self::Held(text) => text,
        }
    }
}

/// The context an answer of `/api/events` adds for the agent.
fn added_context(answer: &Value) -> Option<String> {
    answer["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .filter(|text| !text.is_empty())
        .map(str::to_string)
}

/// Why an answer of `/api/events` blocks the call, when it does.
fn block_reason(answer: &Value) -> Option<String> {
    (answer["decision"].as_str() == Some("block")).then(|| {
        answer["reason"]
            .as_str()
            .filter(|reason| !reason.is_empty())
            .unwrap_or("blocked by a post-cortex module")
            .to_string()
    })
}

/// Where a hook's session and workspace come from.
enum IdSource {
    Fixed(Ids),
    Project(ProjectSource),
}

/// The project a global hook resolves its ids from.
pub struct ProjectSource {
    /// The directory the client runs in.
    pub dir: PathBuf,
    /// The config directory (the cache's home).
    pub config_dir: PathBuf,
    /// `hosts.<author>.workspace`: the workspace name a new key goes into.
    pub workspace: Option<String>,
    /// The [`crate::project::identity`] of the hook's bearer token.
    pub identity: Option<String>,
}

/// One conversation turn to capture.
pub struct Turn {
    /// The client's id for the turn; captures are idempotent on it.
    pub turn_id: String,
    /// `user` or `assistant`.
    pub role: &'static str,
    pub text: String,
    /// The client's own conversation id.
    pub source_session: Option<String>,
    pub tools: Vec<String>,
    pub files: Vec<String>,
}

impl HookEnv {
    /// A hook with the project's ids given.
    pub fn new(
        daemon_url: &str,
        client: reqwest::Client,
        session_id: &str,
        workspace_id: &str,
        author: &str,
    ) -> Self {
        Self::with_source(
            daemon_url,
            client,
            author,
            IdSource::Fixed(Ids {
                session_id: session_id.to_string(),
                workspace_id: workspace_id.to_string(),
            }),
        )
    }

    /// A global hook: the ids of the project `source.dir` is in, resolved
    /// on first use (cache in `source.config_dir`, a new key resolved into
    /// `source.workspace`).
    pub fn for_project(
        daemon_url: &str,
        client: reqwest::Client,
        author: &str,
        source: ProjectSource,
        capture: bool,
    ) -> Self {
        let mut env = Self::with_source(daemon_url, client, author, IdSource::Project(source));
        env.capture = capture;
        env
    }

    fn with_source(
        daemon_url: &str,
        client: reqwest::Client,
        author: &str,
        source: IdSource,
    ) -> Self {
        Self {
            daemon_url: daemon_url.trim_end_matches('/').to_string(),
            author: author.to_string(),
            client,
            source,
            resolved: tokio::sync::OnceCell::new(),
            capture: true,
        }
    }

    /// Whether the ids are resolved rather than given (the agent then has
    /// no instructions file naming them, so the session-start text does).
    fn resolving(&self) -> bool {
        matches!(self.source, IdSource::Project(_))
    }

    /// The resolution of a global hook's ids.
    fn resolver<'a>(&'a self, source: &'a ProjectSource) -> Resolver<'a> {
        Resolver {
            client: &self.client,
            url: &self.daemon_url,
            author: &self.author,
            workspace: source.workspace.as_deref(),
            config_dir: &source.config_dir,
            identity: source.identity.as_deref(),
            instruction_files: &["AGENTS.md"],
        }
    }

    /// The backend refused a request naming the resolved ids (HTTP 403 or
    /// 404): they are dropped from the cache when it holds them, so the next
    /// hook resolves the project again.
    fn forget_refused(&self) {
        let IdSource::Project(source) = &self.source else {
            return;
        };
        if let Some(Some(ids)) = self.resolved.get()
            && self.resolver(source).forget(&source.dir, ids)
        {
            eprintln!(
                "pcx hook: the backend refused the cached ids of {}; the next hook resolves them again",
                source.dir.display()
            );
        }
    }

    /// The project's session and workspace; `None` when they cannot be
    /// resolved (the hook then has nothing to say).
    pub async fn ids(&self) -> Option<&Ids> {
        let source = match &self.source {
            IdSource::Fixed(ids) => return Some(ids),
            IdSource::Project(source) => source,
        };
        self.resolved
            .get_or_init(|| async {
                match self.resolver(source).ids(&source.dir).await {
                    Ok(ids) => Some(ids),
                    Err(e) => {
                        eprintln!(
                            "pcx hook: cannot resolve the project of {}: {e}",
                            source.dir.display()
                        );
                        None
                    }
                }
            })
            .await
            .as_ref()
    }

    /// Send a hook `event` (Claude Code's hook JSON) with `scope` to the
    /// backend's module host (`POST /api/events`); the merged answer of its
    /// modules (`decision`, `reason`, `hookSpecificOutput`).
    async fn event(&self, scope: Value, event: Value) -> Result<Value, String> {
        let mut scope = scope;
        scope["author"] = Value::String(self.author.clone());
        let response = self
            .client
            .post(format!("{}/api/events", self.daemon_url))
            .json(&json!({ "scope": scope, "event": event }))
            .send()
            .await
            .map_err(|e| e.to_string())?;
        let status = response.status();
        if !status.is_success() {
            if matches!(
                status,
                reqwest::StatusCode::FORBIDDEN | reqwest::StatusCode::NOT_FOUND
            ) {
                self.forget_refused();
            }
            return Err(http_error("/api/events", response).await);
        }
        response.json().await.map_err(|e| e.to_string())
    }

    /// The user card and the project card, as the memory module answers a
    /// session start (with no project card yet, how to write one), after
    /// the ids when this hook resolved them. `None` when the backend cannot
    /// be reached.
    pub async fn cards_text(&self) -> Option<String> {
        let ids = self.ids().await?;
        let scope = json!({ "workspace_id": ids.workspace_id });
        let answer = self
            .event(scope, json!({ "hook_event_name": "SessionStart" }))
            .await
            .ok()?;
        let cards = added_context(&answer)?;
        Some(if self.resolving() {
            format!("{}\n\n{cards}", reminder(Some(ids)))
        } else {
            cards
        })
    }

    /// What the memory says about `paths` (past problems, decisions,
    /// dependents) to the agent session `session` (the client's own id;
    /// `None`: a client that names none, told on every edit) at `moment`, as
    /// the memory module answers it: once per session and file (docs/memory.md,
    /// a file's memory). `None` when nothing is to be said or the backend
    /// cannot be reached.
    pub async fn file_memory(
        &self,
        paths: &[String],
        moment: FileMoment,
        session: Option<&str>,
    ) -> Option<FileMemory> {
        let workspace_id = &self.ids().await?.workspace_id;
        let mut held = Vec::new();
        let mut added = Vec::new();
        for path in paths {
            let event = json!({
                "hook_event_name": moment.event_name(),
                "session_id": session.unwrap_or_default(),
                "tool_input": { "file_path": path },
            });
            let Ok(answer) = self
                .event(json!({ "workspace_id": workspace_id }), event)
                .await
            else {
                continue;
            };
            if let Some(reason) = block_reason(&answer) {
                held.push(reason);
            } else if let Some(text) = added_context(&answer) {
                added.push(text);
            }
        }
        match (held.is_empty(), added.is_empty()) {
            (false, _) => Some(FileMemory::Held(held.join("\n\n"))),
            (true, false) => Some(FileMemory::Added(added.join("\n\n"))),
            (true, true) => None,
        }
    }

    /// [`Self::capture`] for a hook, which fails open: a failure is
    /// reported on stderr and the agent goes on.
    pub async fn record(&self, turns: Vec<Turn>) {
        if let Err(e) = self.capture(turns).await {
            eprintln!("pcx hook: capture failed: {e}");
        }
    }

    /// Store turns in the session's message stream (the memory module's
    /// answer to a turn-end event); turns already stored are skipped.
    pub async fn capture(&self, turns: Vec<Turn>) -> Result<(), String> {
        if !self.capture {
            return Ok(());
        }
        let turns: Vec<Turn> = turns
            .into_iter()
            .filter(|t| !t.text.trim().is_empty())
            .collect();
        if turns.is_empty() {
            return Ok(());
        }
        let session_id = &self
            .ids()
            .await
            .ok_or("the project's session is not known")?
            .session_id;
        let messages: Vec<Value> = turns
            .into_iter()
            .map(|t| {
                serde_json::json!({
                    "turn_id": t.turn_id,
                    "role": t.role,
                    "author": if t.role == "assistant" { Some(self.author.clone()) } else { None },
                    "source_session": t.source_session,
                    "text": t.text,
                    "tools": t.tools,
                    "files": t.files,
                })
            })
            .collect();
        let event = json!({
            "hook_event_name": "Stop",
            "payload": { "messages": messages },
        });
        self.event(json!({ "session_id": session_id }), event)
            .await
            .map(|_| ())
    }

    /// The reminder a prompt hook can put before every turn (as the Claude
    /// Code `UserPromptSubmit` banner does); without the ids when they
    /// cannot be resolved.
    pub async fn rules_reminder(&self) -> String {
        reminder(self.ids().await)
    }
}

fn reminder(ids: Option<&Ids>) -> String {
    let rules = "Rules: 1) Search post-cortex before answering code/architecture questions \
                 2) Log decisions, fixes and changes with update_conversation_context after";
    match ids {
        Some(ids) => format!(
            "[PCX] Session: {} | Workspace: {} | {rules}",
            ids.session_id, ids.workspace_id
        ),
        None => format!("[PCX] {rules}"),
    }
}

/// The directory a hook payload says the client runs in: `cwd` (Claude
/// Code style payloads: Codex, Gemini CLI, VS Code, Cline), else the first
/// of `workspace_roots` (Cursor).
pub fn payload_dir(input: &Value) -> Option<PathBuf> {
    input["cwd"]
        .as_str()
        .or_else(|| input["workspace_roots"][0].as_str())
        .map(str::trim)
        .filter(|d| !d.is_empty())
        .map(PathBuf::from)
}

/// `hookSpecificOutput` with `additionalContext`, the reply the
/// Claude-style hook formats (Codex, Gemini CLI, VS Code) take to put
/// `text` in front of the agent.
pub fn context_output(event: &str, text: String) -> String {
    json!({
        "hookSpecificOutput": { "hookEventName": event, "additionalContext": text }
    })
    .to_string()
}

/// A file's memory in the Claude-style hook formats (Codex, VS Code): a held
/// edit as a denied `PreToolUse` whose reason the agent reads, else the text
/// as context for `event`.
pub fn file_memory_output(event: &str, memory: FileMemory) -> String {
    match memory {
        FileMemory::Added(text) => context_output(event, text),
        FileMemory::Held(reason) => json!({
            "hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "permissionDecision": "deny",
                "permissionDecisionReason": reason,
            }
        })
        .to_string(),
    }
}

/// Percent-encode a query value.
pub fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// A stable id for a turn without one: the client's conversation id and a
/// hash of the text (FNV-1a), so a resent turn is recognised.
pub fn turn_id(conversation: &str, role: &str, text: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in conversation.bytes().chain(role.bytes()).chain(text.bytes()) {
        hash ^= u64::from(b);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{conversation}:{role}:{hash:016x}")
}
