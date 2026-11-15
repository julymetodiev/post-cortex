// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The trust a person gives a team (ARCHITECTURE section 8, autonomy): its
//! `team/autonomy` document and whether it approves a decision at once.
//! Pure; the board applies it.

use std::fmt;

use crate::domain::Agent;

/// What a decision risks (its class), how much a policy approves by
/// itself (its mode), the limits of each class (operations' too, W31), one
/// level of trust, a member's or role's exception, a team's autonomy and
/// what a decision puts at stake: the messages of `pcx.team.v1`.
pub use post_cortex_proto::team::{
    Autonomy, Class, CodeLimit, DataLimit, Exception, Mode, MoneyLimit, Stakes, Trust,
};

/// The supervised policy of `team`, the one a team starts with: every
/// decision with a risk waits.
#[must_use]
pub fn supervised(team: &str) -> Autonomy {
    Autonomy {
        team: team.to_string(),
        paused: false,
        trust: Some(Trust {
            mode: Mode::Supervised.into(),
            ..Trust::default()
        }),
        exceptions: Vec::new(),
    }
}

/// The trust of `member` under `autonomy`: its own exception, else its
/// role's, else the team's (none set: supervised).
#[must_use]
pub fn trust_of<'a>(autonomy: &'a Autonomy, member: &Agent) -> Option<&'a Trust> {
    let named = |of: &str| autonomy.exceptions.iter().find(|e| e.of == of);
    named(&member.name)
        .or_else(|| named(&member.role))
        .map_or(autonomy.trust.as_ref(), |e| e.trust.as_ref())
}

/// Why a team's policy does not approve a decision at once: it waits for
/// a person.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// A person paused the team.
    Paused,
    /// The member's trust is supervised: every decision with a risk waits.
    Supervised,
    /// The member's trust approves other classes only.
    Untrusted(Class),
    /// The trust sets no limits for the class.
    NoLimit(Class),
    /// A decision on code names no branch.
    NoBranch,
    /// The branch is protected.
    Protected(String),
    /// The branch starts with none of the trusted prefixes.
    Branch {
        /// The decision's branch.
        branch: String,
        /// The prefixes the trust approves.
        prefixes: Vec<String>,
    },
    /// A decision on money names no amount, or another currency.
    Currency {
        /// The limit's currency.
        currency: String,
    },
    /// Over one decision's limit.
    PerDecision {
        /// The limit.
        limit: u64,
        /// Its currency.
        currency: String,
    },
    /// Over the day's limit, with what the policy approved today.
    PerDay {
        /// The limit.
        limit: u64,
        /// Approved by the policy today.
        spent: u64,
        /// Its currency.
        currency: String,
    },
    /// A decision on data does not say whether it can be undone.
    Reversible,
    /// The change cannot be undone and the trust approves no deletions.
    Deletion,
    /// The decision is one only a person approves.
    Person,
    /// A decision on operations names no action or no environment.
    NoAction,
    /// The trust does not approve this action by itself.
    Action {
        /// The decision's action.
        action: String,
        /// The actions the trust approves.
        actions: Vec<String>,
    },
    /// The trust does not approve actions in this environment by itself.
    Environment {
        /// The decision's environment.
        environment: String,
        /// The environments the trust approves.
        environments: Vec<String>,
    },
}

/// The word of a class, as a person reads it.
fn word(class: Class) -> &'static str {
    match class {
        Class::Unspecified => "unknown",
        Class::Code => "code",
        Class::Money => "money",
        Class::Data => "data",
        Class::Operations => "operations",
    }
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Paused => f.write_str("the team is paused"),
            Self::Supervised => f.write_str("the team is supervised"),
            Self::Untrusted(class) => {
                write!(f, "the trust does not approve {} by itself", word(*class))
            }
            Self::NoLimit(class) => write!(f, "the trust sets no limits for {}", word(*class)),
            Self::NoBranch => f.write_str("the decision names no branch"),
            Self::Protected(branch) => write!(f, "{branch} is protected"),
            Self::Branch { branch, prefixes } => write!(
                f,
                "code approves itself only on {}; this changes {branch}",
                prefixes.join(", ")
            ),
            Self::Currency { currency } => {
                write!(
                    f,
                    "the money limit is in {currency}; the decision names no such amount"
                )
            }
            Self::PerDecision { limit, currency } => {
                write!(f, "over the money limit: {limit} {currency} a decision")
            }
            Self::PerDay {
                limit,
                spent,
                currency,
            } => write!(
                f,
                "over the money limit: {limit} {currency} a day, {spent} approved today"
            ),
            Self::Reversible => f.write_str("the decision does not say whether it can be undone"),
            Self::Deletion => {
                f.write_str("data approves itself only for changes that can be undone")
            }
            Self::Person => f.write_str("only a person approves this decision"),
            Self::NoAction => f.write_str("the decision names no action or no environment"),
            Self::Action { action, actions } => write!(
                f,
                "operations approve themselves only for {}; this is {action}",
                list(actions)
            ),
            Self::Environment {
                environment,
                environments,
            } => write!(
                f,
                "operations approve themselves only in {}; this is in {environment}",
                list(environments)
            ),
        }
    }
}

impl std::error::Error for Refusal {}

/// `names` for a person: joined, or "none".
fn list(names: &[String]) -> String {
    if names.is_empty() {
        "none".to_string()
    } else {
        names.join(", ")
    }
}

/// Whether `autonomy` approves at once `member`'s decision of `class` with
/// `stakes`, the team's policy-approved money of the day being
/// `spent_today` (in the limit's currency).
///
/// # Errors
/// Why it does not: the decision waits for a person.
pub fn verdict(
    autonomy: &Autonomy,
    member: &Agent,
    class: Class,
    stakes: &Stakes,
    spent_today: u64,
) -> Result<(), Refusal> {
    if autonomy.paused {
        return Err(Refusal::Paused);
    }
    if stakes.person == Some(true) {
        return Err(Refusal::Person);
    }
    let Some(trust) = trust_of(autonomy, member) else {
        return Err(Refusal::Supervised);
    };
    match trust.mode() {
        Mode::Unspecified | Mode::Supervised => return Err(Refusal::Supervised),
        Mode::Classes if !trust.classes().any(|c| c == class) => {
            return Err(Refusal::Untrusted(class));
        }
        Mode::Classes | Mode::Full => {}
    }
    match class {
        Class::Unspecified => Err(Refusal::Untrusted(class)),
        Class::Code => {
            let limit = trust.code.as_ref().ok_or(Refusal::NoLimit(class))?;
            let branch = stakes.branch.as_deref().ok_or(Refusal::NoBranch)?;
            if limit.protected.iter().any(|p| p == branch) {
                return Err(Refusal::Protected(branch.to_string()));
            }
            if !limit
                .branches
                .iter()
                .any(|p| branch.starts_with(p.as_str()))
            {
                return Err(Refusal::Branch {
                    branch: branch.to_string(),
                    prefixes: limit.branches.clone(),
                });
            }
            Ok(())
        }
        Class::Money => {
            let limit = trust.money.as_ref().ok_or(Refusal::NoLimit(class))?;
            let amount = stakes
                .amount
                .filter(|_| stakes.currency.as_deref() == Some(limit.currency.as_str()))
                .ok_or_else(|| Refusal::Currency {
                    currency: limit.currency.clone(),
                })?;
            if amount > limit.per_decision {
                return Err(Refusal::PerDecision {
                    limit: limit.per_decision,
                    currency: limit.currency.clone(),
                });
            }
            if spent_today.saturating_add(amount) > limit.per_day {
                return Err(Refusal::PerDay {
                    limit: limit.per_day,
                    spent: spent_today,
                    currency: limit.currency.clone(),
                });
            }
            Ok(())
        }
        Class::Data => {
            let limit = trust.data.as_ref().ok_or(Refusal::NoLimit(class))?;
            match stakes.reversible {
                Some(true) => Ok(()),
                Some(false) if limit.deletions => Ok(()),
                Some(false) => Err(Refusal::Deletion),
                None => Err(Refusal::Reversible),
            }
        }
        Class::Operations => {
            let limit = trust.operations.as_ref().ok_or(Refusal::NoLimit(class))?;
            let (Some(action), Some(environment)) = (&stakes.action, &stakes.environment) else {
                return Err(Refusal::NoAction);
            };
            if !limit.actions.contains(action) {
                return Err(Refusal::Action {
                    action: action.clone(),
                    actions: limit.actions.clone(),
                });
            }
            if !limit.environments.contains(environment) {
                return Err(Refusal::Environment {
                    environment: environment.clone(),
                    environments: limit.environments.clone(),
                });
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests;
