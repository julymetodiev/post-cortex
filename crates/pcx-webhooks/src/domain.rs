// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The webhooks' domain (ARCHITECTURE section 8, operations, W27): what a
//! source's request is allowed to be, and what an alert group becomes on
//! the board: its task, the comments when it fires again or resolves,
//! whether its task closes, and the incident memory keeps. Pure: no host,
//! no memory, no clock.

use std::collections::HashMap;
use std::fmt;

use post_cortex_proto::board::{AddTaskRequest, Board, Stage, TaskRecord, Thread};
use post_cortex_proto::webhooks::{AlertmanagerWebhook, Source, SourceKind, WebhooksConfig};
use sha2::{Digest, Sha256};

/// The module's id.
pub const MODULE_ID: &str = "webhooks";

/// The label a member puts on a task whose diagnosis found nothing to do:
/// the task closes when its group resolves.
pub const NO_ACTION: &str = "no-action";

/// The label of every task an alert adds.
const ALERT: &str = "alert";

/// Hex digits of the group key's hash kept in a task's key.
const KEY_DIGITS: usize = 32;

/// A request or a configuration the module refuses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refused {
    /// The request's token is not the module's.
    Token,
    /// No source of that name.
    UnknownSource(String),
    /// The body is not what the source's kind sends.
    Body(String),
    /// A setting is wrong.
    Setting {
        /// The setting, as a path of field names.
        field: String,
        /// What is wrong.
        reason: String,
    },
}

impl fmt::Display for Refused {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Token => f.write_str("the request's bearer token is not the webhooks' token"),
            Self::UnknownSource(name) => write!(f, "no webhook source is named {name}"),
            Self::Body(reason) => write!(f, "the body is not what the source sends: {reason}"),
            Self::Setting { field, reason } => write!(f, "{field}: {reason}"),
        }
    }
}

impl std::error::Error for Refused {}

fn setting(field: String, reason: impl Into<String>) -> Refused {
    Refused::Setting {
        field,
        reason: reason.into(),
    }
}

/// Whether `config` may run: a token, and sources each named once (1 to
/// 64 of a-z, 0-9, '-'), of a kind, with a workspace and either a board or
/// a workflow (a JSON source only a workflow).
///
/// # Errors
/// The first setting that is wrong.
pub fn check(config: &WebhooksConfig) -> Result<(), Refused> {
    if config.token.trim().is_empty() {
        return Err(setting("token".to_string(), "is empty"));
    }
    let mut seen: Vec<&str> = Vec::with_capacity(config.sources.len());
    for (i, source) in config.sources.iter().enumerate() {
        let at = |field: &str| format!("sources.{i}.{field}");
        let name = source.name.as_str();
        if !pcx_module_api::name::is_slug(name) {
            return Err(setting(at("name"), "is 1 to 64 of a-z, 0-9 and '-'"));
        }
        if seen.contains(&name) {
            return Err(setting(at("name"), format!("names source {name} twice")));
        }
        seen.push(name);
        if source.kind() == SourceKind::Unspecified {
            return Err(setting(at("kind"), "is not given"));
        }
        if source.workspace_id.trim().is_empty() {
            return Err(setting(at("workspace_id"), "is empty"));
        }
        match (
            source.board.trim().is_empty(),
            source.workflow.trim().is_empty(),
        ) {
            (true, true) => {
                return Err(setting(at("board"), "is empty, and no workflow is named"));
            }
            (false, false) => {
                return Err(setting(at("workflow"), "is named beside a board; name one"));
            }
            (false, true) if source.kind() == SourceKind::Json => {
                return Err(setting(
                    at("workflow"),
                    "is empty: a JSON source runs a workflow",
                ));
            }
            (true, false) if !pcx_module_api::name::is_slug(&source.workflow) => {
                return Err(setting(at("workflow"), "is 1 to 64 of a-z, 0-9 and '-'"));
            }
            _ => {}
        }
    }
    Ok(())
}

/// Whether `given` is `expected`, compared in constant time for tokens of
/// one length.
#[must_use]
pub fn authorized(expected: &str, given: &str) -> bool {
    let (expected, given) = (expected.as_bytes(), given.as_bytes());
    expected.len() == given.len()
        && expected
            .iter()
            .zip(given)
            .fold(0u8, |differ, (a, b)| differ | (a ^ b))
            == 0
}

/// The source of `config` named `name`.
///
/// # Errors
/// No source of that name.
pub fn source<'a>(config: &'a WebhooksConfig, name: &str) -> Result<&'a Source, Refused> {
    config
        .sources
        .iter()
        .find(|s| s.name == name)
        .ok_or_else(|| Refused::UnknownSource(name.to_string()))
}

/// Whether a group fires or has resolved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// It fires.
    Firing,
    /// Every alert of it resolved.
    Resolved,
}

impl Status {
    /// The status Alertmanager names.
    ///
    /// # Errors
    /// Anything but `firing` and `resolved`.
    pub fn parse(status: &str) -> Result<Self, Refused> {
        match status {
            "firing" => Ok(Self::Firing),
            "resolved" => Ok(Self::Resolved),
            other => Err(Refused::Body(format!(
                "status {other:?} is neither firing nor resolved"
            ))),
        }
    }
}

/// The group's status, after checking that `hook` names its group.
///
/// # Errors
/// No group key, or a status Alertmanager does not send.
pub fn status(hook: &AlertmanagerWebhook) -> Result<Status, Refused> {
    if hook.group_key.is_empty() {
        return Err(Refused::Body("it names no groupKey".to_string()));
    }
    Status::parse(&hook.status)
}

/// Whether `hook` is a heartbeat of `source`: every alert of its group is
/// one the source only acknowledges (`heartbeats`, by alert name), so the
/// request proves the path and adds no task.
#[must_use]
pub fn heartbeat(source: &Source, hook: &AlertmanagerWebhook) -> bool {
    !hook.alerts.is_empty()
        && hook.alerts.iter().all(|alert| {
            alert
                .labels
                .get("alertname")
                .is_some_and(|name| source.heartbeats.contains(name))
        })
}

/// What the board keys of source `source`'s group `group_key` start with:
/// the source and a hash of the group key (one is long and has quotes).
/// Alertmanager keeps a group's key across its incidents, so each incident
/// (from the group firing with no open task until it resolves) has its
/// own key: the prefix and its number ([`episode_key`]).
#[must_use]
pub fn group_prefix(source: &str, group_key: &str) -> String {
    let hash = Sha256::digest(group_key.as_bytes());
    let hex: String = hash.iter().map(|b| format!("{b:02x}")).collect();
    format!("{source}:{}:", &hex[..KEY_DIGITS])
}

/// The key of the group's incident after `before` earlier ones.
#[must_use]
pub fn episode_key(prefix: &str, before: usize) -> String {
    format!("{prefix}{}", before + 1)
}

/// Whether `task` is one of the incidents whose keys start with `prefix`.
#[must_use]
pub fn of_group(task: &TaskRecord, prefix: &str) -> bool {
    task.task
        .as_ref()
        .is_some_and(|t| t.key.starts_with(prefix))
}

/// What was found and done on a task, from its `thread`: its decisions,
/// then the comments of others than `source` (whose own are what fired),
/// oldest first.
#[must_use]
pub fn findings(thread: &Thread, source: &str) -> Vec<String> {
    let decided = thread
        .decisions
        .iter()
        .filter_map(|d| d.decision.as_ref())
        .map(|d| format!("- Decided: {} ({})", d.what, d.why));
    let said = thread
        .comments
        .iter()
        .filter(|c| c.author.as_deref() != Some(source))
        .map(|c| format!("- {}: {}", c.author.as_deref().unwrap_or("someone"), c.text));
    decided.chain(said).collect()
}

/// Whether `board`'s `column` holds open work (its stages as the board
/// answers them, every column's given; a column it does not have is
/// open).
#[must_use]
pub fn open(board: &Board, column: &str) -> bool {
    board
        .columns
        .iter()
        .zip(board.stages.iter())
        .find(|(c, _)| c.as_str() == column)
        .is_none_or(|(_, s)| *s == i32::from(Stage::Open))
}

/// The alert's name: its group's, else the one every alert has.
fn alert_name(hook: &AlertmanagerWebhook) -> &str {
    label(hook, "alertname").unwrap_or("Alert")
}

/// The value of label `name` of the group, else of every alert.
fn label<'a>(hook: &'a AlertmanagerWebhook, name: &str) -> Option<&'a str> {
    hook.group_labels
        .get(name)
        .or_else(|| hook.common_labels.get(name))
        .map(String::as_str)
}

/// What the group is about: its labels but the alert's name, sorted.
fn targets(hook: &AlertmanagerWebhook) -> Vec<String> {
    let mut targets: Vec<String> = hook
        .group_labels
        .iter()
        .filter(|(k, _)| k.as_str() != "alertname")
        .map(|(_, v)| v.clone())
        .collect();
    targets.sort();
    targets
}

/// The board priority of a severity.
fn priority(severity: &str) -> Option<&'static str> {
    match severity {
        "critical" | "page" => Some("P0"),
        "error" | "high" => Some("P1"),
        "warning" => Some("P2"),
        "info" | "low" => Some("P3"),
        _ => None,
    }
}

/// `labels` as `` `k=v` `` items, sorted.
fn listed(labels: &HashMap<String, String>) -> String {
    let mut items: Vec<String> = labels.iter().map(|(k, v)| format!("`{k}={v}`")).collect();
    items.sort();
    items.join(", ")
}

/// The task with `key` a firing group of `source` adds: its title (the
/// alert and its targets), its text (what the alert says, its labels, its
/// links), labels (`alert`, its severity, the source), priority by
/// severity and the source's assignee.
#[must_use]
pub fn new_task(source: &Source, hook: &AlertmanagerWebhook, key: String) -> AddTaskRequest {
    let targets = targets(hook);
    let title = if targets.is_empty() {
        alert_name(hook).to_string()
    } else {
        format!("{} · {}", alert_name(hook), targets.join(", "))
    };
    let severity = label(hook, "severity").unwrap_or_default();
    let note = |name: &str| hook.common_annotations.get(name).map(String::as_str);
    let mut text = Vec::new();
    if let Some(summary) = note("summary") {
        text.push(summary.to_string());
    }
    if let Some(description) = note("description") {
        text.push(description.to_string());
    }
    let mut facts = vec![format!("- Alerts: {}", hook.alerts.len())];
    if hook.truncated_alerts > 0 {
        facts.push(format!("- Left out: {}", hook.truncated_alerts));
    }
    facts.push(format!("- Labels: {}", listed(&hook.common_labels)));
    if let Some(runbook) = note("runbook_url") {
        facts.push(format!("- Runbook: {runbook}"));
    }
    if let Some(rule) = hook
        .alerts
        .iter()
        .map(|a| a.generator_url.as_str())
        .find(|u| !u.is_empty())
    {
        facts.push(format!("- [The rule]({rule})"));
    }
    if !hook.external_url.is_empty() {
        facts.push(format!("- [Alertmanager]({})", hook.external_url));
    }
    text.push(facts.join("\n"));
    let mut labels = vec![ALERT.to_string()];
    if !severity.is_empty() {
        labels.push(severity.to_string());
    }
    labels.push(source.name.clone());
    AddTaskRequest {
        workspace_id: source.workspace_id.clone(),
        board: source.board.clone(),
        title,
        text: text.join("\n\n"),
        assignee: Some(source.assignee.clone()).filter(|a| !a.is_empty()),
        labels,
        priority: priority(severity).map(String::from),
        key: Some(key),
        ..AddTaskRequest::default()
    }
}

/// What the source's destination is told of a new incident: its severity,
/// its task's title and what the alert says.
#[must_use]
pub fn told(task: &AddTaskRequest, hook: &AlertmanagerWebhook) -> String {
    let severity = label(hook, "severity").unwrap_or("alert");
    let mut text = format!("{severity}: {}", task.title);
    if let Some(summary) = hook.common_annotations.get("summary") {
        text.push('\n');
        text.push_str(summary);
    }
    text
}

/// The comment when the group of an open task fires again.
#[must_use]
pub fn firing_again(hook: &AlertmanagerWebhook) -> String {
    let firing = hook.alerts.iter().filter(|a| a.status == "firing").count();
    format!("Fires again: {firing} of {} alerts.", hook.alerts.len())
}

/// How long the group fired: from its first alert's start to its last
/// alert's end, in seconds; none when the alerts say no times.
#[must_use]
pub fn fired_for(hook: &AlertmanagerWebhook) -> Option<i64> {
    let start = hook
        .alerts
        .iter()
        .filter_map(|a| a.starts_at.as_ref().map(|t| t.seconds))
        .min()?;
    let end = hook
        .alerts
        .iter()
        .filter_map(|a| a.ends_at.as_ref().map(|t| t.seconds))
        .max()?;
    (end >= start).then_some(end - start)
}

/// `seconds` for a person: `45 s`, `12 min`, `3 h 5 min`, `2 d 4 h`.
#[must_use]
pub fn lasted(seconds: i64) -> String {
    let (d, h, m) = (
        seconds / 86_400,
        seconds % 86_400 / 3_600,
        seconds % 3_600 / 60,
    );
    match (d, h, m) {
        (0, 0, 0) => format!("{seconds} s"),
        (0, 0, m) => format!("{m} min"),
        (0, h, m) => format!("{h} h {m} min"),
        (d, h, _) => format!("{d} d {h} h"),
    }
}

/// The comment when the group resolves.
#[must_use]
pub fn resolved(hook: &AlertmanagerWebhook) -> String {
    match fired_for(hook) {
        Some(seconds) => format!("Resolved after {}.", lasted(seconds)),
        None => "Resolved.".to_string(),
    }
}

/// Whether a task with `labels`, held by a decision or not, closes when its
/// group resolves: its member found nothing to do.
#[must_use]
pub fn closes(labels: &[String], held: bool) -> bool {
    !held && labels.iter().any(|l| l == NO_ACTION)
}

/// The board's first `done` column (its stages as the board answers them,
/// every column's given).
#[must_use]
pub fn done_column(board: &Board) -> Option<&str> {
    board
        .columns
        .iter()
        .zip(board.stages.iter())
        .find(|(_, s)| **s == i32::from(Stage::Done))
        .map(|(c, _)| c.as_str())
}

/// An incident as memory keeps it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Incident {
    /// What happened, in a line.
    pub title: String,
    /// How long, where, and what was found and done.
    pub description: String,
    /// The alert's name, an entity.
    pub alert: String,
    /// What it was about, entities.
    pub targets: Vec<String>,
}

/// The incident of a resolved group whose task was `task` (its title) and
/// whose diagnosis is `found` (the decisions and the comments of others
/// than the source, oldest first).
#[must_use]
pub fn incident(hook: &AlertmanagerWebhook, task: &str, found: &[String]) -> Incident {
    let targets = targets(hook);
    let mut description = vec![format!("Task: {task}.")];
    if let Some(seconds) = fired_for(hook) {
        description.push(format!("Fired for {}.", lasted(seconds)));
    }
    description.push(format!("Labels: {}.", listed(&hook.common_labels)));
    if !found.is_empty() {
        description.push(format!("Found and done:\n{}", found.join("\n")));
    }
    Incident {
        title: format!("Incident: {} resolved", alert_name(hook)),
        description: description.join("\n"),
        alert: alert_name(hook).to_string(),
        targets,
    }
}

#[cfg(test)]
mod tests;
