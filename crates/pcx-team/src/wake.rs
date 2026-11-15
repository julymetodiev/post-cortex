// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! When a member works (ARCHITECTURE section 8): which members of the
//! harness a change of the board wakes, what their turn is told, and what
//! it answered. Pure.

use pcx_module_api::name::NAME_CHARS;

use crate::domain::{Agent, Authority};

/// A change of the board, as the team reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    /// A task's version: who wrote it, its assignee now and before.
    Task {
        /// The task.
        task: String,
        /// Who wrote this version.
        author: Option<String>,
        /// Its assignee now.
        assignee: Option<String>,
        /// Its assignee in the version before (none: a new task).
        was: Option<String>,
    },
    /// A new comment: who wrote it, its text, whose comment it answers.
    Comment {
        /// Its task.
        task: String,
        /// The comment.
        comment: String,
        /// Who wrote it.
        author: Option<String>,
        /// Its text.
        text: String,
        /// The author of the comment it answers.
        answers: Option<String>,
    },
}

/// Why a member is woken.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Why {
    /// The task was given to it.
    Assigned,
    /// A comment names it (`@name`).
    Mentioned,
    /// A comment answers its comment.
    Answered,
    /// Its turn was cut short when the team stopped, and goes on.
    Resumed,
}

/// A member to wake for a task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Wake {
    /// The member.
    pub member: String,
    /// The task.
    pub task: String,
    /// The comment its answer answers (none: a comment of its own).
    pub answers: Option<String>,
    /// Why.
    pub why: Why,
    /// The reach of the write that woke the member (docs/connect.md 4a):
    /// its turn acts within it; none: every workspace.
    pub reach: Option<Vec<String>>,
}

/// A task's thread, as the board gives it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Thread {
    /// Its title.
    pub title: String,
    /// Its column.
    pub column: String,
    /// Its text.
    pub text: String,
    /// Its comments, oldest first: who wrote what.
    pub comments: Vec<(String, String)>,
    /// Its decisions, oldest first: who decided what, and whether it waits
    /// for approval.
    pub decisions: Vec<(String, String, bool)>,
}

impl Thread {
    /// The thread as a turn reads it.
    #[must_use]
    pub fn text(&self) -> String {
        let mut text = format!("# {} [{}]\n{}", self.title, self.column, self.text);
        if !self.comments.is_empty() {
            text.push_str("\n\n## Comments");
            for (author, said) in &self.comments {
                text.push_str(&format!("\n- {author}: {said}"));
            }
        }
        if !self.decisions.is_empty() {
            text.push_str("\n\n## Decisions");
            for (by, what, pending) in &self.decisions {
                let state = if *pending {
                    " (waits for approval)"
                } else {
                    ""
                };
                text.push_str(&format!("\n- {by}: {what}{state}"));
            }
        }
        text
    }
}

/// The names `text` mentions (`@name`), each once, in order.
#[must_use]
pub fn mentions(text: &str) -> Vec<String> {
    let mut named: Vec<String> = Vec::new();
    for (at, _) in text.match_indices('@') {
        // A mention starts a word.
        if text[..at]
            .chars()
            .next_back()
            .is_some_and(|c| c.is_alphanumeric())
        {
            continue;
        }
        let name: String = text[at + 1..]
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
            .take(NAME_CHARS)
            .collect();
        // A sentence's full stop is not part of the name.
        let name = name.trim_end_matches('.').to_string();
        if !name.is_empty() && !named.contains(&name) {
            named.push(name);
        }
    }
    named
}

/// The members of the harness among `members` that `change` wakes: the
/// one it assigns the task to, those its comment mentions, and the author
/// of the comment it answers when a person answers (a member's answer
/// does not wake the one it answers, so two members do not keep waking
/// each other); never its own author.
#[must_use]
pub fn woken(change: &Change, members: &[Agent]) -> Vec<Wake> {
    let harness = |name: &str| members.iter().any(|m| m.harness && m.name == name);
    let mut woken: Vec<Wake> = Vec::new();
    let mut wake =
        |member: &str, task: &str, answers: Option<&str>, why: Why, author: &Option<String>| {
            let own = author.as_deref() == Some(member);
            if harness(member) && !own && !woken.iter().any(|w| w.member == member) {
                woken.push(Wake {
                    member: member.to_string(),
                    task: task.to_string(),
                    answers: answers.map(str::to_string),
                    why,
                    // The write's, set by whoever read it (the runner).
                    reach: None,
                });
            }
        };
    match change {
        Change::Task {
            task,
            author,
            assignee,
            was,
        } => {
            if let Some(assignee) = assignee.as_deref().filter(|a| was.as_deref() != Some(*a)) {
                wake(assignee, task, None, Why::Assigned, author);
            }
        }
        Change::Comment {
            task,
            comment,
            author,
            text,
            answers,
        } => {
            for named in mentions(text) {
                wake(&named, task, Some(comment), Why::Mentioned, author);
            }
            let by_member = author.as_deref().is_some_and(harness);
            if let Some(answered) = answers.as_deref().filter(|_| !by_member) {
                wake(answered, task, Some(comment), Why::Answered, author);
            }
        }
    }
    woken
}

/// How a member writes in a thread: who it wakes, its language, and how it
/// cites memory (a link the app opens, not a raw id).
const THREAD_RULES: &str = "Mention a member (@name) only when you need them to act: a mention wakes them. Answer in the language the task, or the comment you answer, is written in. When you cite a record of memory, link it by its title, [its title](pcx:record/<entry_id>), and show no other ids.";

/// The board module: a member that may use it writes in threads.
const BOARD: &str = "board";

/// What a member's turn adds to the agent's system prompt: its role, for
/// one that may use the board its authority and how it writes in a thread,
/// then each of its skills' instructions.
#[must_use]
pub fn instructions(member: &Agent, skills: &[(String, String)]) -> String {
    // What its authority lets it do on the board, so it does not try what
    // the board refuses.
    let may = match member.authority() {
        Authority::Unspecified | Authority::Discuss => {
            "comment on tasks and move them between columns; recording a decision or closing a task (moving it into a done or canceled column) is a member's with more authority"
        }
        Authority::Decide => {
            "comment, move tasks between columns and record decisions; closing a task (moving it into a done or canceled column) is a member's who may close"
        }
        Authority::Close => {
            "comment, move tasks, record decisions and close a task (move it into a done or canceled column)"
        }
    };
    let mut text = format!(
        "You are {}, a member of the team: {}.",
        member.name, member.role
    );
    // One held to modules without the board (the agent of a community)
    // writes in no thread: what it says leaves the app.
    if member.modules.is_empty() || member.modules.iter().any(|m| m == BOARD) {
        text.push_str(&format!(" On the board you may {may}. {THREAD_RULES}"));
    }
    for (name, instructions) in skills {
        text.push_str(&format!("\n\n## Skill: {name}\n{instructions}"));
    }
    text
}

/// What a member's talk adds to its instructions: the workspace it runs
/// in, which a search of memory names (a turn held to some workspaces is
/// refused one that names none, docs/connect.md section 4a).
#[must_use]
pub fn talking_in(workspace: &str) -> String {
    format!("You work in workspace {workspace}: name it (workspace_id) in every search of memory.")
}

/// What a member's turn is asked: why it works, where (the task's
/// workspace, which its board and team tools work in already), then the
/// task's thread.
#[must_use]
pub fn prompt(why: Why, task: &str, workspace: &str, thread: &str) -> String {
    let reason = match why {
        Why::Assigned => "The task below was assigned to you.",
        Why::Mentioned => "A comment on the task below mentions you.",
        Why::Answered => "A comment on the task below answers yours.",
        Why::Resumed => {
            "Your turn on the task below was cut short when the system restarted: go on from where you stopped. The last tool call you made may or may not have finished, so look before you repeat it."
        }
    };
    format!(
        "{reason} Work on it with your tools (task id {task}). Your board and team tools work in the task's workspace already; it is {workspace}, the scope_id for a search of its memory. Your last message becomes your comment on the task: write it for the people who read the task, and do not say that it is posted.\n\n{thread}"
    )
}

#[cfg(test)]
mod tests;
