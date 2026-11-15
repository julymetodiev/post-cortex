// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The board's domain (ARCHITECTURE section 8): boards, tasks, comments and
//! decisions as their documents hold them, and the rules over them. Pure:
//! no host, no memory, no clock.

use std::fmt;

use pcx_module_api::text::clip;
use pcx_team::{Class, Refusal, Stakes};

/// The module's id, and the prefix of its document kinds.
pub const MODULE_ID: &str = "board";

/// How a team's policy names itself as an approver.
pub const POLICY: &str = "policy:";

/// Characters of a comment's first line kept as its title.
const TITLE_CHARS: usize = 120;

/// Characters a task's key may have.
const KEY_CHARS: usize = 200;

/// The kinds of documents the board keeps, `board/<name>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A board: its name and columns.
    Board,
    /// A task on a board.
    Task,
    /// A comment on a task.
    Comment,
    /// A decision on a task.
    Decision,
}

impl Kind {
    /// Its name in the module.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Board => "board",
            Self::Task => "task",
            Self::Comment => "comment",
            Self::Decision => "decision",
        }
    }

    /// Its full name, `board/<name>`, as memory keeps it.
    #[must_use]
    pub fn document(self) -> String {
        format!("{MODULE_ID}/{}", self.name())
    }

    /// Every kind.
    pub const ALL: [Self; 4] = [Self::Board, Self::Task, Self::Comment, Self::Decision];
}

/// The stage of `board`'s `column`. A board written before W26 (a column
/// with no stage given) reads as before: its last column done, the others
/// open; a column the board does not have holds no closed task: open.
#[must_use]
pub fn stage(board: &Board, column: &str) -> Stage {
    board
        .columns
        .iter()
        .position(|c| c == column)
        .map_or(Stage::Open, |at| stage_at(board, at))
}

/// The stage of `board`'s column at `at` (see [`stage`]).
fn stage_at(board: &Board, at: usize) -> Stage {
    match board.stages.get(at).map(|s| Stage::try_from(*s)) {
        Some(Ok(given @ (Stage::Open | Stage::Done | Stage::Canceled))) => given,
        _ if at + 1 == board.columns.len() => Stage::Done,
        _ => Stage::Open,
    }
}

/// `board` with every column's stage given, as a board written before
/// W26 reads.
#[must_use]
pub fn staged(board: Board) -> Board {
    let stages = (0..board.columns.len())
        .map(|at| stage_at(&board, at).into())
        .collect();
    Board { stages, ..board }
}

/// Whether a task put in `board`'s `column` is closed: done or canceled.
#[must_use]
pub fn closes(board: &Board, column: &str) -> bool {
    matches!(stage(board, column), Stage::Done | Stage::Canceled)
}

/// What a decision risks, what a column means for its tasks, a board, what
/// a task links to in git, a task, a comment and a decision: the messages
/// of `pcx.board.v1`.
pub use post_cortex_proto::board::{Board, Comment, Decision, Links, Risk, Stage, Task};

/// An image of a task or a comment: a blob of memory.
pub use post_cortex_proto::pb::Attachment;

/// The rules of a decision (its message is the board's package's).
pub trait DecisionRules {
    /// What it puts at stake, as a team's policy weighs it.
    fn stakes_of(&self) -> Stakes;

    /// Its risk's class, as a team's policy counts it; none for no risk.
    fn class(&self) -> Option<Class>;

    /// Whether a team's policy approved it.
    fn by_policy(&self) -> bool;

    /// Whether it waits for approval, holding its task.
    fn pending(&self) -> bool;
}

impl DecisionRules for Decision {
    fn stakes_of(&self) -> Stakes {
        self.stakes.clone().unwrap_or_default()
    }

    fn class(&self) -> Option<Class> {
        match self.risk() {
            Risk::Unspecified | Risk::None => None,
            Risk::Code => Some(Class::Code),
            Risk::Money => Some(Class::Money),
            Risk::Data => Some(Class::Data),
            Risk::Operations => Some(Class::Operations),
        }
    }

    fn by_policy(&self) -> bool {
        self.approved_by
            .as_deref()
            .is_some_and(|by| by.starts_with(POLICY))
    }

    fn pending(&self) -> bool {
        self.risk() != Risk::None
            && self.approved_by.is_none()
            && self.withdrawn.is_none()
            && self.declined_by.is_none()
    }
}

/// `text` with something in it.
fn said<'a>(field: &'static str, text: &'a str) -> Result<&'a str, Broken> {
    let text = text.trim();
    if text.is_empty() {
        Err(Broken::Empty(field))
    } else {
        Ok(text)
    }
}

/// A new board named `name` with `columns` (each once, in order) at
/// `stages` (each column's; none: the last done, the others open), its
/// tasks' branches in `repo`, its decisions recorded in `session_id`.
///
/// # Errors
/// An empty name, no columns, an empty or repeated column, stages that are
/// not one per column or leave one unspecified.
pub fn board(
    name: &str,
    columns: Vec<String>,
    stages: Vec<i32>,
    repo: Option<String>,
    session_id: String,
) -> Result<Board, Broken> {
    let name = said("name", name)?.to_string();
    if columns.is_empty() {
        return Err(Broken::Columns("needs at least one column".to_string()));
    }
    let mut seen = Vec::with_capacity(columns.len());
    for column in &columns {
        let column = column.trim();
        if column.is_empty() {
            return Err(Broken::Columns("has an empty column".to_string()));
        }
        if seen.contains(&column) {
            return Err(Broken::Columns(format!("names column {column} twice")));
        }
        seen.push(column);
    }
    if !stages.is_empty() && stages.len() != seen.len() {
        return Err(Broken::Columns(format!(
            "names {} stages for {} columns",
            stages.len(),
            seen.len()
        )));
    }
    for (column, stage) in seen.iter().zip(&stages) {
        if !matches!(
            Stage::try_from(*stage),
            Ok(Stage::Open | Stage::Done | Stage::Canceled)
        ) {
            return Err(Broken::Columns(format!(
                "gives column {column} no stage (open, done or canceled)"
            )));
        }
    }
    // No stages: each column's as a board without them reads.
    Ok(staged(Board {
        name,
        columns: seen.into_iter().map(String::from).collect(),
        repo,
        session_id,
        stages,
    }))
}

/// A task's key as the board keeps it: trimmed; none when it says nothing.
///
/// # Errors
/// A key longer than 200 characters.
pub fn key(key: Option<&str>) -> Result<Option<String>, Broken> {
    match key.map(str::trim).filter(|k| !k.is_empty()) {
        Some(k) if k.chars().count() > KEY_CHARS => Err(Broken::LongKey { most: KEY_CHARS }),
        k => Ok(k.map(String::from)),
    }
}

/// `column` of `board`, or its first column when none is named.
///
/// # Errors
/// A column the board does not have.
pub fn column(board: &Board, column: Option<&str>) -> Result<String, Broken> {
    match column {
        None => board
            .columns
            .first()
            .cloned()
            .ok_or_else(|| Broken::Columns("needs at least one column".to_string())),
        Some(column) if board.columns.iter().any(|c| c == column) => Ok(column.to_string()),
        Some(column) => Err(Broken::UnknownColumn {
            column: column.to_string(),
            columns: board.columns.clone(),
        }),
    }
}

/// Whether a task may move: no decision of `decisions` (its own, by id)
/// waits for approval.
///
/// # Errors
/// The first decision that holds it.
pub fn may_move<'a>(
    decisions: impl IntoIterator<Item = (&'a str, &'a Decision)>,
) -> Result<(), Broken> {
    match decisions.into_iter().find(|(_, d)| d.pending()) {
        Some((id, _)) => Err(Broken::Held {
            decision: id.to_string(),
        }),
        None => Ok(()),
    }
}

/// A comment on `task` saying `text` with `attachments`, answering
/// `answers`.
///
/// # Errors
/// An empty text.
pub fn comment(
    task: &str,
    text: &str,
    answers: Option<String>,
    attachments: Vec<Attachment>,
) -> Result<Comment, Broken> {
    let text = said("text", text)?;
    let first = text.lines().next().unwrap_or(text);
    Ok(Comment {
        task: task.to_string(),
        answers,
        title: clip(first, TITLE_CHARS),
        attachments,
    })
}

/// `decided_by`'s decision on `task`: `what`, `why`, risking `risk`; one
/// that risks nothing needs no approval.
///
/// # Errors
/// An empty what or why; no risk named.
pub fn decide(
    task: &str,
    what: &str,
    why: &str,
    decided_by: &str,
    risk: Risk,
    stakes: Stakes,
) -> Result<Decision, Broken> {
    if risk == Risk::Unspecified {
        return Err(Broken::Empty("risk"));
    }
    Ok(Decision {
        task: task.to_string(),
        what: said("what", what)?.to_string(),
        why: said("why", why)?.to_string(),
        decided_by: said("the decider", decided_by)?.to_string(),
        risk: risk.into(),
        approved_by: None,
        stakes: Some(stakes),
        held_because: None,
        approval_note: None,
        withdrawn: None,
        declined_by: None,
        decline_note: None,
    })
}

/// Why a decision with a risk waits for a person.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Hold {
    /// Its author is in no team of the workspace (or is no member of one).
    NoTeam(String),
    /// The author's team's policy does not approve it.
    Policy(Refusal),
}

impl fmt::Display for Hold {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoTeam(author) => {
                write!(
                    f,
                    "{author} is in no team of the workspace: a person approves"
                )
            }
            Self::Policy(refusal) => write!(f, "the policy would not approve it: {refusal}"),
        }
    }
}

/// `decision`, waiting for a person because of `hold`.
#[must_use]
pub fn held(decision: Decision, hold: &Hold) -> Decision {
    Decision {
        held_because: Some(hold.to_string()),
        ..decision
    }
}

/// The approver a team's policy of `version` names.
#[must_use]
pub fn policy(version: u64) -> String {
    format!("{POLICY}{version}")
}

/// `decision` with its approval revoked: it waits again.
///
/// # Errors
/// A decision that is not approved.
pub fn revoke(decision: Decision) -> Result<Decision, Broken> {
    if decision.approved_by.is_none() {
        return Err(Broken::NotApproved);
    }
    Ok(Decision {
        approved_by: None,
        approval_note: None,
        ..decision
    })
}

/// `decision` withdrawn by `by`, its author, because of `why`: it waits
/// for no one and holds its task no more.
///
/// # Errors
/// A decision that waits for no approval; `by` is not its author; an
/// empty why.
pub fn withdraw(decision: Decision, by: &str, why: &str) -> Result<Decision, Broken> {
    if !decision.pending() {
        return Err(Broken::NotPending);
    }
    if decision.decided_by != by {
        return Err(Broken::NotTheDecider {
            decider: decision.decided_by,
        });
    }
    let why = said("why", why)?.to_string();
    Ok(Decision {
        withdrawn: Some(why),
        ..decision
    })
}

/// `decision` declined by `by`, a person, with `note` for the record: it
/// waits for no one and holds its task no more.
///
/// # Errors
/// A decision that waits for no approval; an empty person.
pub fn decline(decision: Decision, by: &str, note: Option<String>) -> Result<Decision, Broken> {
    if !decision.pending() {
        return Err(Broken::NotPending);
    }
    let by = said("who declines", by)?.to_string();
    let note = note.map(|n| n.trim().to_string()).filter(|n| !n.is_empty());
    Ok(Decision {
        declined_by: Some(by),
        decline_note: note,
        ..decision
    })
}

/// `decision` approved by `approver`, with `note` for the record.
///
/// # Errors
/// A decision that waits for no approval; an empty approver.
pub fn approve(
    decision: Decision,
    approver: &str,
    note: Option<String>,
) -> Result<Decision, Broken> {
    if !decision.pending() {
        return Err(Broken::NotPending);
    }
    let approver = said("the approver", approver)?.to_string();
    let note = note.map(|n| n.trim().to_string()).filter(|n| !n.is_empty());
    Ok(Decision {
        approved_by: Some(approver),
        approval_note: note,
        ..decision
    })
}

mod broken;
mod plan;
mod rank;
mod short;

pub use broken::Broken;
pub use plan::{Standing, dependencies, order, place, plan, standing};
pub use short::{SHORT_ID, full, task_id};

#[cfg(test)]
mod tests;
