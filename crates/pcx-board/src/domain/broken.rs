// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The rules of the board a call breaks.

use std::fmt;

/// A rule of the board a call breaks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Broken {
    /// A board needs a name and at least one column, each once.
    Columns(String),
    /// The column is not one of the board's.
    UnknownColumn {
        /// The column asked for.
        column: String,
        /// The board's.
        columns: Vec<String>,
    },
    /// A decision with a risk waits for approval; the task does not move.
    Held {
        /// The decision.
        decision: String,
    },
    /// The decision needs no approval or has it already.
    NotPending,
    /// The decision is not approved: there is nothing to revoke.
    NotApproved,
    /// Only a decision's author withdraws it.
    NotTheDecider {
        /// Its author.
        decider: String,
    },
    /// A field that must say something is empty.
    Empty(&'static str),
    /// A task names itself as its plan, a task it waits on, or the task it
    /// goes before.
    SelfLink,
    /// The task named is not on the task's board.
    NotOnBoard {
        /// The task named.
        task: String,
    },
    /// The plan named has a plan of its own: plans are one level deep.
    Nested {
        /// The plan named.
        plan: String,
    },
    /// The task is a plan (others name it as theirs): it has no plan.
    IsPlan {
        /// The task.
        task: String,
    },
    /// Waiting on these tasks makes a cycle.
    Cycle {
        /// The chain of waits, from the task back to it.
        chain: Vec<String>,
    },
    /// A task's key is longer than a key may be.
    LongKey {
        /// The characters a key may have.
        most: usize,
    },
    /// The task to go before is not in the column.
    NotInColumn {
        /// The task named.
        task: String,
        /// The column.
        column: String,
    },
    /// A short id that begins more than one task's id.
    Ambiguous {
        /// The short id.
        short: String,
        /// The tasks it begins.
        tasks: Vec<String>,
    },
}

impl fmt::Display for Broken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Columns(reason) => write!(f, "a board {reason}"),
            Self::UnknownColumn { column, columns } => {
                write!(
                    f,
                    "{column} is not a column of the board ({})",
                    columns.join(", ")
                )
            }
            Self::Held { decision } => write!(
                f,
                "the task waits for decision {decision} to be approved; it moves once it is"
            ),
            Self::NotPending => f.write_str("the decision waits for no approval"),
            Self::NotApproved => f.write_str("the decision is not approved"),
            Self::NotTheDecider { decider } => {
                write!(f, "only {decider}, who made the decision, withdraws it")
            }
            Self::Empty(field) => write!(f, "{field} says nothing"),
            Self::SelfLink => f.write_str("a task does not name itself"),
            Self::NotOnBoard { task } => write!(f, "task {task} is not on the board"),
            Self::Nested { plan } => {
                write!(f, "{plan} is in a plan itself; a plan is one level deep")
            }
            Self::IsPlan { task } => {
                write!(
                    f,
                    "{task} is a plan: other tasks name it, so it has no plan"
                )
            }
            Self::Cycle { chain } => {
                write!(
                    f,
                    "waiting on it makes a cycle: {}",
                    chain.join(" waits on ")
                )
            }
            Self::NotInColumn { task, column } => {
                write!(f, "task {task} is not in column {column}")
            }
            Self::LongKey { most } => write!(f, "a task's key is at most {most} characters"),
            Self::Ambiguous { short, tasks } => write!(
                f,
                "{short} begins more than one task's id ({}): name more of it",
                tasks.join(", ")
            ),
        }
    }
}

impl std::error::Error for Broken {}
