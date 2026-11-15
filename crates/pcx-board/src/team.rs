// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The team's rules as the board applies them (ARCHITECTURE section 8):
//! who may decide or close, and whether a decision is approved at once by
//! the trust a person gave the author's team. Read from the team's
//! documents in the board's workspace; without a team the board works as
//! alone.

use pcx_documents::{Documents, Item, Typed};
use pcx_module_api::{ModuleError, ModuleErrorKind, PersonRole};
use pcx_team::{Act, Agent, Autonomy, Team, TeamKind};

use crate::domain::{self, Decision, DecisionRules, Hold, Kind, Risk};

/// Seconds in a day.
const DAY: i64 = 24 * 60 * 60;

/// The team's rules over the workspace's documents.
pub struct TeamRules<'a> {
    docs: &'a dyn Documents,
}

impl<'a> TeamRules<'a> {
    /// The rules over `docs`.
    pub fn new(docs: &'a dyn Documents) -> Self {
        Self { docs }
    }

    /// Whether `author` may do `act`: anyone when the workspace has no
    /// team; an owner or admin of the organisation (`role`) always; a
    /// module asking a decision in its own name (the call from `module`);
    /// else a member whose authority covers it.
    ///
    /// # Errors
    /// `Denied` when it may not; memory failed.
    pub async fn authorize(
        &self,
        author: Option<&str>,
        role: Option<PersonRole>,
        module: Option<&str>,
        act: Act,
    ) -> Result<(), ModuleError> {
        let (teams, agents) = self.roster().await?;
        pcx_team::may(author, role, module, act, &teams, &agents)
            .map_err(|refused| ModuleError::new(ModuleErrorKind::Denied, refused.to_string()))
    }

    /// The approver of `written` (a new decision of `author` with a risk)
    /// when its team's policy approves it at once (`policy:<version>`),
    /// else why it waits for a person.
    ///
    /// # Errors
    /// Memory failed.
    pub async fn weigh(
        &self,
        author: &str,
        written: &Item<Decision>,
    ) -> Result<Result<String, Hold>, ModuleError> {
        let alone = || Hold::NoTeam(author.to_string());
        let Some(class) = written.data.class() else {
            return Ok(Err(alone()));
        };
        let (teams, agents) = self.roster().await?;
        let (Some(team), Some(member)) = (
            teams.iter().find(|t| t.members.iter().any(|m| m == author)),
            agents.iter().find(|a| a.name == author),
        ) else {
            return Ok(Err(alone()));
        };
        let of = Autonomy {
            team: team.name.clone(),
            ..Autonomy::default()
        };
        let autonomies: Vec<Item<Autonomy>> = Typed::new(self.docs, TeamKind::Autonomy.document())
            .all(Some((&of, &["team"])))
            .await?;
        let Some(autonomy) = autonomies.into_iter().next() else {
            // A team starts supervised.
            return Ok(Err(Hold::Policy(pcx_team::Refusal::Supervised)));
        };
        let spent = self
            .spent(&written.data, written.document.updated_at.as_ref())
            .await?;
        Ok(pcx_team::verdict(
            &autonomy.data,
            member,
            class,
            &written.data.stakes_of(),
            spent,
        )
        .map(|()| domain::policy(autonomy.document.version))
        .map_err(Hold::Policy))
    }

    /// What the policy approved of money in `decision`'s currency on the
    /// UTC day of `at`, across the workspace's boards.
    async fn spent(
        &self,
        decision: &Decision,
        at: Option<&prost_types::Timestamp>,
    ) -> Result<u64, ModuleError> {
        let Some(currency) = decision.stakes_of().currency else {
            return Ok(0);
        };
        let day = at
            .ok_or_else(|| {
                ModuleError::new(ModuleErrorKind::Internal, "a decision written at no time")
            })?
            .seconds
            .div_euclid(DAY);
        let money = Decision {
            risk: Risk::Money.into(),
            ..Decision::default()
        };
        let decisions: Vec<Item<Decision>> = Typed::new(self.docs, Kind::Decision.document())
            .all(Some((&money, &["risk"])))
            .await?;
        Ok(decisions
            .iter()
            .filter(|d| {
                let stakes = d.data.stakes_of();
                d.data.by_policy()
                    && stakes.currency.as_ref() == Some(&currency)
                    && d.document
                        .updated_at
                        .as_ref()
                        .is_some_and(|t| t.seconds.div_euclid(DAY) == day)
            })
            .filter_map(|d| d.data.stakes_of().amount)
            .sum())
    }

    /// The workspace's teams and members.
    async fn roster(&self) -> Result<(Vec<Team>, Vec<Agent>), ModuleError> {
        let teams: Vec<Item<Team>> = Typed::new(self.docs, TeamKind::Team.document())
            .all(None)
            .await?;
        if teams.is_empty() {
            return Ok((Vec::new(), Vec::new()));
        }
        let agents: Vec<Item<Agent>> = Typed::new(self.docs, TeamKind::Agent.document())
            .all(None)
            .await?;
        Ok((
            teams.into_iter().map(|t| t.data).collect(),
            agents.into_iter().map(|a| a.data).collect(),
        ))
    }
}
